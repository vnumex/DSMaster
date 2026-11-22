use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::w;

use super::mouse::MouseInjector;
use super::processing::process_input;
use super::profile::ControllerProfile;
use super::seqlock::SeqLock;
use super::telemetry::TelemetrySnapshot;
use crate::emu::vigem::VigemClient;
use crate::emu::vigem::ds4::DualShock4Target;
use crate::emu::vigem::x360::Xbox360Target;
use crate::emu::{EmulationMode, VirtualController};
use crate::hid::{CancelHandle, DualSenseDevice, ReadOutcome};
use crate::protocol::{
    AdaptiveTriggerEffect, ImuCalibration, OutputState, ProtocolError, Transport,
    encode_output_report, parse_input_report,
};

struct MmcssGuard(HANDLE);
impl MmcssGuard {
    fn enter_games() -> Option<Self> {
        let mut task_index = 0u32;
        unsafe { AvSetMmThreadCharacteristicsW(w!("Games"), &mut task_index) }
            .ok()
            .map(Self)
    }
}
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

pub struct ControllerSessionHandle {
    pub instance_id: String,
    pub telemetry: Arc<SeqLock<TelemetrySnapshot>>,
    stop_flag: Arc<AtomicBool>,
    cancel_handle: CancelHandle,
    join_handle: Option<JoinHandle<()>>,
}

impl ControllerSessionHandle {
    pub fn spawn(
        device: DualSenseDevice,
        vigem: Option<VigemClient>,
        profile: Arc<ArcSwap<ControllerProfile>>,
        topology_version: Arc<AtomicU64>,
    ) -> std::io::Result<Self> {
        let info = device.info().clone();
        let cancel_handle = device.cancel_handle();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(SeqLock::new(TelemetrySnapshot {
            active: true,
            is_edge: info.is_edge,
            transport: info.transport,
            emulation: profile.load().emulation,
            ..Default::default()
        }));

        let stop_c = Arc::clone(&stop_flag);
        let tel_c = Arc::clone(&telemetry);

        let join_handle = thread::Builder::new()
            .name(format!("ds-io-{}", info.product_id))
            .spawn(move || session_loop(device, vigem, profile, tel_c, stop_c, topology_version))?;

        Ok(Self {
            instance_id: info.instance_id,
            telemetry,
            stop_flag,
            cancel_handle,
            join_handle: Some(join_handle),
        })
    }

    pub fn is_finished(&self) -> bool {
        self.join_handle
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        self.cancel_handle.cancel();
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ControllerSessionHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn session_loop(
    mut device: DualSenseDevice,
    vigem: Option<VigemClient>,
    profile_swap: Arc<ArcSwap<ControllerProfile>>,
    telemetry: Arc<SeqLock<TelemetrySnapshot>>,
    stop_flag: Arc<AtomicBool>,
    topology_version: Arc<AtomicU64>,
) {
    let _mmcss = MmcssGuard::enter_games();
    let info = device.info().clone();

    let calibration = match device.read_feature_calibration() {
        Ok(Some(cal)) => {
            debug!(path = %info.device_path, "Loaded DualSense calibration");
            cal
        }
        Ok(None) => {
            warn!(path = %info.device_path, "Calibration report invalid; using fallback");
            ImuCalibration::default()
        }
        Err(e) => {
            warn!(path = %info.device_path, error = %e, "Failed to query calibration 0x05");
            ImuCalibration::default()
        }
    };
    let mut last_bt_activate = Instant::now();

    let mut active_mode = EmulationMode::None;
    let mut target: Option<Box<dyn VirtualController>> = None;
    let initial_prof = *profile_swap.load_full();
    reconcile_target(
        initial_prof.emulation,
        &vigem,
        &mut active_mode,
        &mut target,
    );

    let mut mouse = MouseInjector::default();
    let mut bt_seq: u8 = 0;
    let mut last_override_lightbar = initial_prof.override_lightbar;
    let mut last_override_triggers = initial_prof.override_triggers;
    let mut last_rgb = initial_prof.lightbar_rgb;
    let mut ds4_rgb: Option<[u8; 3]> = None;
    let mut last_leds = initial_prof.player_leds;
    let mut last_left_trigger = initial_prof.left_trigger_preset.effect();
    let mut last_right_trigger = initial_prof.right_trigger_preset.effect();
    let mut rumble_large: u8 = 0;
    let mut rumble_small: u8 = 0;
    let mut triggers_touched = initial_prof.override_triggers;
    let mut dirty_rumble = false;
    let mut dirty_lightbar = false;
    let mut dirty_triggers = false;
    let mut release_triggers = false;

    if initial_prof.override_lightbar || initial_prof.override_triggers {
        flush_output(
            &mut device,
            &OutputState {
                update_lightbar: initial_prof.override_lightbar,
                lightbar_rgb: last_rgb,
                update_player_leds: initial_prof.override_lightbar,
                player_leds: last_leds,
                update_left_trigger: initial_prof.override_triggers,
                left_trigger: last_left_trigger,
                update_right_trigger: initial_prof.override_triggers,
                right_trigger: last_right_trigger,
                ..Default::default()
            },
            &mut bt_seq,
        );
    }
    let mut last_output_write = Instant::now();

    let mut rate_window_start = Instant::now();
    let mut rate_window_packets = 0u32;
    let mut polling_hz = 0.0f32;
    let mut proc_us_ema = 0.0f32;
    let mut proc_us_max = 0u32;
    let mut packet_count = 0u64;
    let bt_min_interval = Duration::from_millis(8);

    info!(
        transport = ?info.transport,
        is_edge = info.is_edge,
        mode = ?active_mode,
        "Controller session started"
    );

    while !stop_flag.load(Ordering::Acquire) {
        let outcome = match device.read_report(250) {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "HID read error; ending session");
                break;
            }
        };

        let processed = match outcome {
            ReadOutcome::Report(bytes) => {
                let t0 = Instant::now();
                let raw = match parse_input_report(bytes, info.transport, info.is_edge) {
                    Ok(p) => p,
                    Err(ProtocolError::MinimalBluetoothReport) => {
                        if last_bt_activate.elapsed() >= Duration::from_millis(500) {
                            let _ = device.read_feature_calibration();
                            last_bt_activate = Instant::now();
                        }
                        continue;
                    }
                    Err(_) => continue,
                };

                let prof = profile_swap.load();
                let proc_in = process_input(&raw, &calibration, &prof);
                mouse.update(&proc_in, &prof);

                if prof.emulation != active_mode {
                    ds4_rgb = None;
                    reconcile_target(prof.emulation, &vigem, &mut active_mode, &mut target);
                }

                if let Some(vc) = target.as_deref_mut() {
                    let _ = vc.submit(&proc_in);
                    if let Some(fb) = vc.poll_feedback() {
                        if prof.enable_rumble {
                            if fb.large_motor != rumble_large || fb.small_motor != rumble_small {
                                rumble_large = fb.large_motor;
                                rumble_small = fb.small_motor;
                                dirty_rumble = true;
                            }
                        } else if rumble_large != 0 || rumble_small != 0 {
                            rumble_large = 0;
                            rumble_small = 0;
                            dirty_rumble = true;
                        }
                        if prof.override_lightbar {
                            if let Some(rgb) = fb.lightbar_rgb {
                                if ds4_rgb != Some(rgb) {
                                    ds4_rgb = Some(rgb);
                                    dirty_lightbar = true;
                                }
                            }
                        }
                    }
                }

                if !prof.enable_rumble && (rumble_large != 0 || rumble_small != 0) {
                    rumble_large = 0;
                    rumble_small = 0;
                    dirty_rumble = true;
                }

                if prof.override_lightbar != last_override_lightbar {
                    last_override_lightbar = prof.override_lightbar;
                    if prof.override_lightbar {
                        last_rgb = prof.lightbar_rgb;
                        last_leds = prof.player_leds;
                        dirty_lightbar = true;
                    } else {
                        ds4_rgb = None;
                        dirty_lightbar = false;
                    }
                } else if prof.override_lightbar
                    && (prof.lightbar_rgb != last_rgb || prof.player_leds != last_leds)
                {
                    last_rgb = prof.lightbar_rgb;
                    last_leds = prof.player_leds;
                    dirty_lightbar = true;
                }

                let left_eff = prof.left_trigger_preset.effect();
                let right_eff = prof.right_trigger_preset.effect();
                if prof.override_triggers != last_override_triggers {
                    last_override_triggers = prof.override_triggers;
                    if prof.override_triggers {
                        last_left_trigger = left_eff;
                        last_right_trigger = right_eff;
                        dirty_triggers = true;
                        release_triggers = false;
                        triggers_touched = true;
                    } else {
                        dirty_triggers = false;
                        if triggers_touched {
                            release_triggers = true;
                        }
                    }
                } else if prof.override_triggers
                    && (left_eff != last_left_trigger || right_eff != last_right_trigger)
                {
                    last_left_trigger = left_eff;
                    last_right_trigger = right_eff;
                    dirty_triggers = true;
                    triggers_touched = true;
                }

                if dirty_rumble || dirty_lightbar || dirty_triggers || release_triggers {
                    let can_write = info.transport == Transport::Usb
                        || last_output_write.elapsed() >= bt_min_interval
                        || (dirty_rumble && rumble_large == 0 && rumble_small == 0)
                        || release_triggers;
                    if can_write {
                        let effective_rgb = ds4_rgb.unwrap_or(last_rgb);
                        let update_lb = dirty_lightbar && prof.override_lightbar;
                        let update_trig =
                            (dirty_triggers && prof.override_triggers) || release_triggers;
                        flush_output(
                            &mut device,
                            &OutputState {
                                update_rumble: dirty_rumble,
                                motor_left: rumble_large,
                                motor_right: rumble_small,
                                update_lightbar: update_lb,
                                lightbar_rgb: effective_rgb,
                                update_player_leds: update_lb,
                                player_leds: last_leds,
                                update_left_trigger: update_trig,
                                left_trigger: if release_triggers {
                                    AdaptiveTriggerEffect::Off
                                } else {
                                    last_left_trigger
                                },
                                update_right_trigger: update_trig,
                                right_trigger: if release_triggers {
                                    AdaptiveTriggerEffect::Off
                                } else {
                                    last_right_trigger
                                },
                                ..Default::default()
                            },
                            &mut bt_seq,
                        );
                        if release_triggers {
                            triggers_touched = false;
                        }
                        dirty_rumble = false;
                        dirty_lightbar = false;
                        dirty_triggers = false;
                        release_triggers = false;
                        last_output_write = Instant::now();
                    }
                }

                let proc_us = t0.elapsed().as_micros() as u32;
                proc_us_ema = if packet_count == 0 {
                    proc_us as f32
                } else {
                    proc_us_ema * 0.95 + (proc_us as f32) * 0.05
                };
                if proc_us > proc_us_max {
                    proc_us_max = proc_us;
                }
                packet_count += 1;
                rate_window_packets += 1;
                let elapsed = rate_window_start.elapsed();
                if elapsed >= Duration::from_millis(500) {
                    polling_hz = rate_window_packets as f32 / elapsed.as_secs_f32();
                    rate_window_packets = 0;
                    rate_window_start = Instant::now();
                }

                proc_in
            }
            ReadOutcome::Timeout => continue,
            ReadOutcome::Cancelled | ReadOutcome::Disconnected => break,
        };

        telemetry.write(TelemetrySnapshot {
            active: true,
            is_edge: info.is_edge,
            transport: info.transport,
            emulation: active_mode,
            raw_left_stick: [processed.raw_left_x, processed.raw_left_y],
            raw_right_stick: [processed.raw_right_x, processed.raw_right_y],
            left_stick: [processed.left_x, processed.left_y],
            right_stick: [processed.right_x, processed.right_y],
            l2_analog: processed.l2_analog,
            r2_analog: processed.r2_analog,
            dpad: processed.dpad,
            buttons: processed.buttons,
            battery_percent: processed.battery_percent,
            battery_status: processed.battery_status,
            polling_hz,
            proc_us_ema,
            proc_us_max,
            packet_count,
            rumble_large,
            rumble_small,
        });
    }

    let stop_rumble = rumble_large > 0 || rumble_small > 0;
    if stop_rumble || triggers_touched {
        flush_output(
            &mut device,
            &OutputState {
                update_rumble: stop_rumble,
                motor_left: 0,
                motor_right: 0,
                update_left_trigger: triggers_touched,
                left_trigger: AdaptiveTriggerEffect::Off,
                update_right_trigger: triggers_touched,
                right_trigger: AdaptiveTriggerEffect::Off,
                ..Default::default()
            },
            &mut bt_seq,
        );
    }
    drop(target);
    let mut final_snap = telemetry.read();
    final_snap.active = false;
    telemetry.write(final_snap);
    topology_version.fetch_add(1, Ordering::Release);
    info!(path = %info.device_path, "Controller session stopped");
}

fn reconcile_target(
    desired: EmulationMode,
    vigem: &Option<VigemClient>,
    active: &mut EmulationMode,
    target: &mut Option<Box<dyn VirtualController>>,
) {
    *target = None;
    *active = EmulationMode::None;
    match desired {
        EmulationMode::None => {}
        EmulationMode::Xbox360 => {
            if let Some(client) = vigem {
                match Xbox360Target::new(client) {
                    Ok(t) => {
                        *target = Some(Box::new(t));
                        *active = EmulationMode::Xbox360;
                    }
                    Err(e) => warn!(error = %e, "Failed to plug in Xbox 360 virtual target"),
                }
            } else {
                warn!("Xbox 360 emulation requested, but ViGEmBus is unavailable");
            }
        }
        EmulationMode::DualShock4 => {
            if let Some(client) = vigem {
                match DualShock4Target::new(client) {
                    Ok(t) => {
                        *target = Some(Box::new(t));
                        *active = EmulationMode::DualShock4;
                    }
                    Err(e) => warn!(error = %e, "Failed to plug in DualShock 4 virtual target"),
                }
            } else {
                warn!("DualShock 4 emulation requested, but ViGEmBus is unavailable");
            }
        }
    }
}

#[inline]
fn flush_output(device: &mut DualSenseDevice, state: &OutputState, bt_seq: &mut u8) {
    let transport = device.info().transport;
    let seq = *bt_seq;
    if encode_output_report(state, transport, seq, device.write_buf_mut()).is_ok() {
        if transport == Transport::Bluetooth {
            *bt_seq = bt_seq.wrapping_add(1) & 0x0F;
        }
        let _ = device.write_report(50);
    }
}
