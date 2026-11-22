use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::emu::EmulationMode;
use crate::emu::VirtualController;
use crate::emu::vigem::VigemClient;
use crate::emu::vigem::x360::Xbox360Target;
use crate::hid::{CancelHandle, DualSenseDevice, ReadOutcome};
use crate::protocol::{
    AdaptiveTriggerEffect, OutputState, ProtocolError, Transport, encode_output_report,
    parse_input_report,
};
use arc_swap::ArcSwap;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::w;

use super::profile::ControllerProfile;
use super::seqlock::SeqLock;
use super::telemetry::TelemetrySnapshot;

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

    // Activate BT 0x31 full mode and fetch IMU calibration.
    match device.read_feature_calibration() {
        Ok(Some(_cal)) => debug!(path = %info.device_path, "Loaded DualSense calibration"),
        Ok(None) => warn!(path = %info.device_path, "Calibration report invalid; using fallback"),
        Err(e) => warn!(path = %info.device_path, error = %e, "Failed to query calibration 0x05"),
    }

    let mut active_mode = EmulationMode::None;
    let mut target: Option<Box<dyn VirtualController>> = None;
    let initial_prof = *profile_swap.load_full();
    reconcile_target(
        initial_prof.emulation,
        &vigem,
        &mut active_mode,
        &mut target,
    );

    let mut bt_seq: u8 = 0;
    let mut last_rgb = initial_prof.lightbar_rgb;
    let mut last_leds = initial_prof.player_leds;
    let mut last_left_trigger = initial_prof.left_trigger;
    let mut last_right_trigger = initial_prof.right_trigger;
    let mut rumble_large: u8 = 0;
    let mut rumble_small: u8 = 0;

    // Send initial LED & adaptive trigger configuration.
    flush_output(
        &mut device,
        &OutputState {
            update_lightbar: true,
            lightbar_rgb: last_rgb,
            update_player_leds: true,
            player_leds: last_leds,
            update_left_trigger: true,
            left_trigger: last_left_trigger,
            update_right_trigger: true,
            right_trigger: last_right_trigger,
            ..Default::default()
        },
        &mut bt_seq,
    );
    let mut output_dirty = false;
    let mut last_output_write = Instant::now();

    let mut rate_window_start = Instant::now();
    let mut rate_window_packets = 0u32;
    let mut polling_hz = 0.0f32;
    let mut proc_us_ema = 0.0f32;
    let mut proc_us_max = 0u32;
    let mut packet_count = 0u64;
    let bt_min_interval = Duration::from_millis(8); // ~125 Hz cap for BT output reports

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

        let raw = match outcome {
            ReadOutcome::Report(bytes) => {
                let t0 = Instant::now();
                let parsed = match parse_input_report(bytes, info.transport, info.is_edge) {
                    Ok(p) => p,
                    Err(ProtocolError::MinimalBluetoothReport) => {
                        let _ = device.read_feature_calibration();
                        continue;
                    }
                    Err(_) => continue,
                };

                let prof = profile_swap.load();
                if prof.emulation != active_mode {
                    reconcile_target(prof.emulation, &vigem, &mut active_mode, &mut target);
                }

                if let Some(vc) = target.as_deref_mut() {
                    let _ = vc.submit(&parsed);
                    if let Some(fb) = vc.poll_feedback() {
                        if prof.enable_rumble
                            && (fb.large_motor != rumble_large || fb.small_motor != rumble_small)
                        {
                            rumble_large = fb.large_motor;
                            rumble_small = fb.small_motor;
                            output_dirty = true;
                        }
                    }
                }

                if prof.lightbar_rgb != last_rgb
                    || prof.player_leds != last_leds
                    || prof.left_trigger != last_left_trigger
                    || prof.right_trigger != last_right_trigger
                {
                    last_rgb = prof.lightbar_rgb;
                    last_leds = prof.player_leds;
                    last_left_trigger = prof.left_trigger;
                    last_right_trigger = prof.right_trigger;
                    output_dirty = true;
                }

                if output_dirty {
                    let can_write = info.transport == Transport::Usb
                        || last_output_write.elapsed() >= bt_min_interval
                        || (rumble_large == 0 && rumble_small == 0);
                    if can_write {
                        flush_output(
                            &mut device,
                            &OutputState {
                                update_rumble: prof.enable_rumble,
                                motor_left: if prof.enable_rumble { rumble_large } else { 0 },
                                motor_right: if prof.enable_rumble { rumble_small } else { 0 },
                                update_lightbar: true,
                                lightbar_rgb: last_rgb,
                                update_player_leds: true,
                                player_leds: last_leds,
                                update_left_trigger: true,
                                left_trigger: last_left_trigger,
                                update_right_trigger: true,
                                right_trigger: last_right_trigger,
                                ..Default::default()
                            },
                            &mut bt_seq,
                        );
                        output_dirty = false;
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

                parsed
            }
            ReadOutcome::Timeout => continue,
            ReadOutcome::Cancelled | ReadOutcome::Disconnected => break,
        };

        telemetry.write(TelemetrySnapshot {
            active: true,
            is_edge: info.is_edge,
            transport: info.transport,
            emulation: active_mode,
            left_stick: [raw.left_x, raw.left_y],
            right_stick: [raw.right_x, raw.right_y],
            l2_analog: raw.l2_analog,
            r2_analog: raw.r2_analog,
            dpad: raw.dpad,
            buttons: raw.buttons,
            battery_percent: raw.battery_percent,
            battery_status: raw.battery_status,
            polling_hz,
            proc_us_ema,
            proc_us_max,
            packet_count,
            rumble_large,
            rumble_small,
        });
    }

    // Zero rumble and withdraw adaptive trigger actuators on clean exit.
    flush_output(
        &mut device,
        &OutputState {
            update_rumble: true,
            motor_left: 0,
            motor_right: 0,
            update_left_trigger: true,
            left_trigger: AdaptiveTriggerEffect::Off,
            update_right_trigger: true,
            right_trigger: AdaptiveTriggerEffect::Off,
            ..Default::default()
        },
        &mut bt_seq,
    );
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
    if desired == EmulationMode::Xbox360 {
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
