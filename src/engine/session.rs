use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::w;

use super::bt_audio::{BtAudioTestSignal, BtAudioWorkerHandle};
use super::mouse::MouseInjector;
use super::processing::{ProcessedInput, TurboState, apply_turbo_state, process_input};
use super::profile::{ControllerProfile, LightbarMode, MicMuteLedOverride};
use super::seqlock::SeqLock;
use super::telemetry::TelemetrySnapshot;
use crate::emu::audio_device::{SharedAudioBridge, VirtualAudioDevice};
use crate::emu::dualsense::VirtualDualSenseConfig;
use crate::emu::vigem::ds4::DualShock4Target;
use crate::emu::vigem::x360::Xbox360Target;
use crate::emu::{EmuBackends, EmulationMode, VirtualController};
use crate::hid::{CancelHandle, DeviceInfo, DualSenseDevice, ReadOutcome};
use crate::protocol::{
    AdaptiveTriggerEffect, BatteryStatus, BtAudioRoute, DS_FEATURE_REPORT_CALIBRATION_SIZE,
    DS_GYRO_RES_PER_DEG_S, DualSenseButtons, ImuCalibration, MicMuteLed, OutputState,
    ProtocolError, Transport, VIRTUAL_SERIAL_PREFIX,
    constants::{DS_ACC_RES_PER_G, DS_BT_MIC_VOLUME_MAX, DS_INPUT_REPORT_BT},
    encode_output_report, extract_bt_mic_opus, parse_input_report, scale_headphone_volume,
    scale_speaker_volume,
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

#[derive(Default)]
pub struct TestRumbleSignal {
    /// `(large as u32) | ((small as u32) << 8) | ((seq as u32) << 16)`
    packed: AtomicU32,
    duration_ms: AtomicU16,
}

impl TestRumbleSignal {
    pub fn trigger(&self, large: u8, small: u8, duration_ms: u16) {
        self.duration_ms.store(duration_ms, Ordering::Release);
        let prev = self.packed.load(Ordering::Acquire);
        let next_seq = ((prev >> 16) as u16).wrapping_add(1).max(1);
        let next = (large as u32) | ((small as u32) << 8) | ((next_seq as u32) << 16);
        self.packed.store(next, Ordering::Release);
    }

    fn snapshot(&self) -> (u16, u8, u8, u16) {
        let p = self.packed.load(Ordering::Acquire);
        let d = self.duration_ms.load(Ordering::Acquire);
        ((p >> 16) as u16, (p & 0xFF) as u8, ((p >> 8) & 0xFF) as u8, d)
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
        backends: EmuBackends,
        profile: Arc<ArcSwap<ControllerProfile>>,
        topology_version: Arc<AtomicU64>,
        test_rumble: Arc<TestRumbleSignal>,
        bt_audio_test: Arc<BtAudioTestSignal>,
        cloak_enabled: Arc<AtomicBool>,
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
            .spawn(move || {
                session_loop(
                    device,
                    backends,
                    profile,
                    tel_c,
                    stop_c,
                    topology_version,
                    test_rumble,
                    bt_audio_test,
                    cloak_enabled,
                )
            })?;

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
    backends: EmuBackends,
    profile_swap: Arc<ArcSwap<ControllerProfile>>,
    telemetry: Arc<SeqLock<TelemetrySnapshot>>,
    stop_flag: Arc<AtomicBool>,
    topology_version: Arc<AtomicU64>,
    test_rumble: Arc<TestRumbleSignal>,
    bt_audio_test: Arc<BtAudioTestSignal>,
    cloak_enabled: Arc<AtomicBool>,
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
    let mac_address = device.read_feature_pairing_mac().unwrap_or([0; 6]);
    let fw_info = device.read_feature_firmware_info();
    let firmware_version = fw_info.map(|f| f.main_fw_version).unwrap_or(0);
    let hardware_version = fw_info.map(|f| f.hw_info).unwrap_or(0);
    let mut firmware_date = [0u8; 20];
    if let Some(fw) = fw_info {
        firmware_date[..11].copy_from_slice(&fw.build_date);
        firmware_date[11] = b' ';
        firmware_date[12..20].copy_from_slice(&fw.build_time);
    }
    let mut last_bt_activate = Instant::now();

    let mut active_mode = EmulationMode::None;
    let mut target: Option<Box<dyn VirtualController>> = None;
    let initial_prof = *profile_swap.load_full();
    let mut last_requested_mode = initial_prof.emulation;
    reconcile_target(
        initial_prof.emulation,
        &backends,
        &info,
        device.raw_calibration_05(),
        &mut active_mode,
        &mut target,
    );

    let mut mouse = MouseInjector::default();
    let mut turbo_state = TurboState::default();
    let mut last_base_proc: Option<ProcessedInput> = None;
    let mut bt_seq: u8 = 0;
    let session_start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_anim_eval = Instant::now();
    let mut prev_buttons = DualSenseButtons::empty();
    let mut prev_dpad = crate::protocol::DPad::Center;
    let mut prev_axes: [u8; 6] = [128, 128, 128, 128, 0, 0];

    let mut last_override_lightbar = initial_prof.override_lightbar;
    let mut last_override_triggers = initial_prof.override_triggers;
    let mut last_rgb = compute_animated_lightbar(&initial_prof, 0.0, 100, BatteryStatus::Unknown, 0, 0);
    let mut game_rgb: Option<[u8; 3]> = None;
    let mut last_leds = initial_prof.player_leds;
    let mut game_leds: Option<u8> = None;
    let mut last_led_brightness = initial_prof.player_led_brightness;
    let mut last_mic_mute_led = initial_prof.mic_mute_led;
    let mut last_battery_pct: u8 = 100;
    let mut last_battery_status = BatteryStatus::Unknown;
    let mut last_max_trigger: u8;
    let mut last_left_trigger = initial_prof.left_trigger_effect();
    let mut last_right_trigger = initial_prof.right_trigger_effect();
    let mut game_left_trigger: Option<[u8; 11]> = None;
    let mut game_right_trigger: Option<[u8; 11]> = None;
    let mut game_rumble_large: u8 = 0;
    let mut game_rumble_small: u8 = 0;
    let mut rumble_large: u8 = 0;
    let mut rumble_small: u8 = 0;
    let mut last_test_seq: u16 = test_rumble.snapshot().0;
    let mut active_test_rumble: Option<(Instant, u8, u8)> = None;
    let mut triggers_touched = initial_prof.override_triggers;
    let mut dirty_rumble = false;
    let mut dirty_lightbar = false;
    let mut dirty_player_leds = false;
    let mut dirty_led_brightness = false;
    let mut dirty_mic_mute_led = false;
    let mut dirty_left_trigger = false;
    let mut dirty_right_trigger = false;
    let mut release_triggers = false;
    let mut dirty_bt_audio = false;
    let mut last_bt_audio_hw: Option<(bool, u8, u8, BtAudioRoute, bool, bool)> = None;

    let (bt_audio_bridge, mut bt_audio_worker) = if info.transport == Transport::Bluetooth {
        let bridge = Arc::new(SharedAudioBridge::new());
        let worker = BtAudioWorkerHandle::spawn(
            info.device_path.clone(),
            info.output_report_len,
            Arc::clone(&profile_swap),
            bt_audio_test,
            Arc::clone(&bridge),
            Arc::clone(&cloak_enabled),
        );
        (Some(bridge), Some(worker))
    } else {
        (None, None)
    };
    let mut virtual_audio: Option<VirtualAudioDevice> = None;
    let mut virtual_audio_attempted = false;

    if initial_prof.override_lightbar
        || initial_prof.override_triggers
        || initial_prof.mic_mute_led != MicMuteLedOverride::Native
    {
        flush_output(
            &mut device,
            &OutputState {
                update_lightbar: initial_prof.override_lightbar,
                lightbar_rgb: last_rgb,
                update_player_leds: initial_prof.override_lightbar,
                player_leds: last_leds,
                update_led_brightness: initial_prof.override_lightbar,
                led_brightness: last_led_brightness,
                update_mic_mute_led: initial_prof.mic_mute_led.to_protocol().is_some(),
                mic_mute_led: initial_prof.mic_mute_led.to_protocol().unwrap_or(MicMuteLed::Off),
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
        let read_timeout = if turbo_state.is_active() { 10 } else { 250 };
        let outcome = match device.read_report(read_timeout) {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "HID read error; ending session");
                break;
            }
        };

        let processed = match outcome {
            ReadOutcome::Report(bytes) => {
                if info.transport == Transport::Bluetooth
                    && bytes.len() >= 78
                    && bytes[0] == DS_INPUT_REPORT_BT
                    && (bytes[1] & 0x02) != 0
                {
                    if let (Some(br), Some(pkt)) =
                        (bt_audio_bridge.as_ref(), extract_bt_mic_opus(bytes))
                    {
                        let _ = br.mic_opus_tx.try_send(pkt);
                    }
                    continue;
                }
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
                let mut proc_in = process_input(&raw, &calibration, &prof);
                last_base_proc = Some(proc_in);
                apply_turbo_state(&mut proc_in, &prof.turbo, &mut turbo_state, Instant::now());
                mouse.update(&proc_in, &prof);

                let cur_axes = [
                    raw.left_x,
                    raw.left_y,
                    raw.right_x,
                    raw.right_y,
                    raw.l2_analog,
                    raw.r2_analog,
                ];
                let axis_moved = cur_axes
                    .iter()
                    .zip(prev_axes.iter())
                    .any(|(&a, &b)| a.abs_diff(b) > 4);
                if raw.buttons != prev_buttons
                    || raw.dpad != prev_dpad
                    || raw.touch[0].active
                    || raw.touch[1].active
                    || axis_moved
                {
                    last_activity = Instant::now();
                    prev_buttons = raw.buttons;
                    prev_dpad = raw.dpad;
                    prev_axes = cur_axes;
                }
                last_battery_pct = proc_in.battery_percent;
                last_battery_status = proc_in.battery_status;
                last_max_trigger = proc_in.l2_analog.max(proc_in.r2_analog);

                if prof.emulation != last_requested_mode {
                    last_requested_mode = prof.emulation;
                    game_rgb = None;
                    game_leds = None;
                    let had_game_triggers =
                        game_left_trigger.take().is_some() || game_right_trigger.take().is_some();
                    if had_game_triggers && !prof.override_triggers && triggers_touched {
                        release_triggers = true;
                        dirty_left_trigger = false;
                        dirty_right_trigger = false;
                    }
                    if rumble_large != 0 || rumble_small != 0 {
                        rumble_large = 0;
                        rumble_small = 0;
                        dirty_rumble = true;
                    }
                    reconcile_target(
                        prof.emulation,
                        &backends,
                        &info,
                        device.raw_calibration_05(),
                        &mut active_mode,
                        &mut target,
                    );
                }

                if let Some(vc) = target.as_deref_mut() {
                    let _ = vc.submit(&proc_in);
                    if let Some(fb) = vc.poll_feedback() {
                        if let Some((large, small)) = fb.rumble {
                            game_rumble_large = large;
                            game_rumble_small = small;
                        }
                        if let Some(rgb) = fb.lightbar_rgb {
                            if game_rgb != Some(rgb) {
                                game_rgb = Some(rgb);
                                if !prof.override_lightbar {
                                    dirty_lightbar = true;
                                }
                            }
                        }
                        if let Some(leds) = fb.player_leds {
                            let masked = leds & 0x1F;
                            if game_leds != Some(masked) {
                                game_leds = Some(masked);
                                if !prof.override_lightbar {
                                    dirty_player_leds = true;
                                }
                            }
                        }
                        if let Some(raw_l) = fb.left_trigger_raw {
                            if game_left_trigger != Some(raw_l) {
                                game_left_trigger = Some(raw_l);
                                if !prof.override_triggers {
                                    dirty_left_trigger = true;
                                    release_triggers = false;
                                    triggers_touched = true;
                                }
                            }
                        }
                        if let Some(raw_r) = fb.right_trigger_raw {
                            if game_right_trigger != Some(raw_r) {
                                game_right_trigger = Some(raw_r);
                                if !prof.override_triggers {
                                    dirty_right_trigger = true;
                                    release_triggers = false;
                                    triggers_touched = true;
                                }
                            }
                        }
                    }
                }

                let (test_seq, t_large, t_small, t_dur) = test_rumble.snapshot();
                if test_seq != last_test_seq {
                    last_test_seq = test_seq;
                    let until = Instant::now() + Duration::from_millis(t_dur.max(50) as u64);
                    active_test_rumble = Some((until, t_large, t_small));
                }
                if let Some((until, _, _)) = active_test_rumble {
                    if Instant::now() >= until {
                        active_test_rumble = None;
                    }
                }

                let bt_spk_test_active = bt_audio_worker
                    .as_ref()
                    .is_some_and(|w| w.telemetry.speaker_test_active.load(Ordering::Acquire));
                let bt_hap_test_active = bt_audio_worker
                    .as_ref()
                    .is_some_and(|w| w.telemetry.haptics_test_active.load(Ordering::Acquire));
                let bt_spk_active = info.transport == Transport::Bluetooth
                    && (prof.virtual_audio_device || bt_spk_test_active);
                let bt_hap_active = info.transport == Transport::Bluetooth
                    && (prof.bt_haptics_enabled || bt_hap_test_active);
                let bt_mic_active = info.transport == Transport::Bluetooth
                    && prof.virtual_audio_device
                    && cloak_enabled.load(Ordering::Acquire)
                    && bt_audio_bridge
                        .as_ref()
                        .is_some_and(|b| b.mic_endpoint_open.load(Ordering::Acquire));
                let bt_any_audio = bt_spk_active || bt_hap_active || bt_mic_active;
                if let Some(br) = bt_audio_bridge.as_ref() {
                    if prof.virtual_audio_device {
                        if virtual_audio.is_none() && !virtual_audio_attempted {
                            virtual_audio_attempted = true;
                            match VirtualAudioDevice::spawn(Arc::clone(br)) {
                                Ok(dev) => virtual_audio = Some(dev),
                                Err(e) => warn!(error = %e, "Failed to attach Virtual DualSense Audio"),
                            }
                        }
                    } else {
                        if virtual_audio.is_some() {
                            virtual_audio = None;
                        }
                        virtual_audio_attempted = false;
                    }
                }
                let bt_vol_norm = if prof.virtual_audio_device {
                    prof.bt_speaker_volume
                } else {
                    0.85
                };
                let bt_spk_vol_byte = scale_speaker_volume(bt_vol_norm);
                let bt_hp_vol_byte = scale_headphone_volume(bt_vol_norm);
                let virtual_speaker_open = bt_audio_bridge
                    .as_ref()
                    .is_some_and(|b| b.speaker_endpoint_open.load(Ordering::Acquire));
                let bt_hw_key = (
                    bt_any_audio,
                    bt_spk_vol_byte,
                    bt_hp_vol_byte,
                    prof.bt_audio_route,
                    bt_hap_active,
                    virtual_speaker_open,
                );
                if bt_any_audio && last_bt_audio_hw != Some(bt_hw_key) {
                    last_bt_audio_hw = Some(bt_hw_key);
                    dirty_bt_audio = true;
                } else if !bt_any_audio {
                    last_bt_audio_hw = Some(bt_hw_key);
                }

                let (desired_large, desired_small) = if bt_hap_active {
                    (0, 0)
                } else if let Some((_, tl, ts)) = active_test_rumble {
                    let scale = prof.rumble_strength.clamp(0.15, 1.0);
                    (
                        ((tl as f32) * scale).round().clamp(0.0, 255.0) as u8,
                        ((ts as f32) * scale).round().clamp(0.0, 255.0) as u8,
                    )
                } else if prof.enable_rumble {
                    let scale = prof.rumble_strength.clamp(0.0, 1.0);
                    (
                        ((game_rumble_large as f32) * scale).round().clamp(0.0, 255.0) as u8,
                        ((game_rumble_small as f32) * scale).round().clamp(0.0, 255.0) as u8,
                    )
                } else {
                    (0, 0)
                };

                if desired_large != rumble_large || desired_small != rumble_small {
                    rumble_large = desired_large;
                    rumble_small = desired_small;
                    dirty_rumble = !bt_hap_active;
                }
                if bt_hap_active {
                    dirty_rumble = false;
                }

                let low_batt_active = is_low_battery_alert(&prof, last_battery_pct, last_battery_status);
                let lightbar_controlled = prof.override_lightbar || low_batt_active;

                if prof.override_lightbar != last_override_lightbar {
                    last_override_lightbar = prof.override_lightbar;
                    if prof.override_lightbar {
                        last_rgb = compute_animated_lightbar(
                            &prof,
                            session_start.elapsed().as_secs_f32(),
                            last_battery_pct,
                            last_battery_status,
                            last_max_trigger,
                            last_activity.elapsed().as_secs() as u32,
                        );
                        last_leds = prof.player_leds;
                        last_led_brightness = prof.player_led_brightness;
                        dirty_lightbar = true;
                        dirty_player_leds = true;
                        dirty_led_brightness = true;
                    } else {
                        dirty_lightbar = game_rgb.is_some();
                        dirty_player_leds = game_leds.is_some();
                    }
                } else if lightbar_controlled {
                    if prof.override_lightbar {
                        if prof.player_leds != last_leds {
                            last_leds = prof.player_leds;
                            dirty_player_leds = true;
                        }
                        if prof.player_led_brightness != last_led_brightness {
                            last_led_brightness = prof.player_led_brightness;
                            dirty_led_brightness = true;
                        }
                    }
                    let anim_due = prof.lightbar_mode == LightbarMode::Static && !low_batt_active
                        || last_anim_eval.elapsed() >= Duration::from_millis(33);
                    if anim_due {
                        last_anim_eval = Instant::now();
                        let desired_rgb = compute_animated_lightbar(
                            &prof,
                            session_start.elapsed().as_secs_f32(),
                            last_battery_pct,
                            last_battery_status,
                            last_max_trigger,
                            last_activity.elapsed().as_secs() as u32,
                        );
                        if desired_rgb != last_rgb {
                            last_rgb = desired_rgb;
                            dirty_lightbar = true;
                        }
                    }
                }

                if prof.mic_mute_led != last_mic_mute_led {
                    last_mic_mute_led = prof.mic_mute_led;
                    dirty_mic_mute_led = true;
                }

                let left_eff = prof.left_trigger_effect();
                let right_eff = prof.right_trigger_effect();
                if prof.override_triggers != last_override_triggers {
                    last_override_triggers = prof.override_triggers;
                    if prof.override_triggers {
                        last_left_trigger = left_eff;
                        last_right_trigger = right_eff;
                        dirty_left_trigger = true;
                        dirty_right_trigger = true;
                        release_triggers = false;
                        triggers_touched = true;
                    } else if game_left_trigger.is_some() || game_right_trigger.is_some() {
                        dirty_left_trigger = true;
                        dirty_right_trigger = true;
                        release_triggers = false;
                        triggers_touched = true;
                    } else {
                        dirty_left_trigger = false;
                        dirty_right_trigger = false;
                        if triggers_touched {
                            release_triggers = true;
                        }
                    }
                } else if prof.override_triggers {
                    if left_eff != last_left_trigger {
                        last_left_trigger = left_eff;
                        dirty_left_trigger = true;
                        triggers_touched = true;
                    }
                    if right_eff != last_right_trigger {
                        last_right_trigger = right_eff;
                        dirty_right_trigger = true;
                        triggers_touched = true;
                    }
                }

                if dirty_rumble
                    || dirty_lightbar
                    || dirty_player_leds
                    || dirty_led_brightness
                    || dirty_mic_mute_led
                    || dirty_left_trigger
                    || dirty_right_trigger
                    || release_triggers
                    || dirty_bt_audio
                {
                    let can_write = info.transport == Transport::Usb
                        || last_output_write.elapsed() >= bt_min_interval
                        || (dirty_rumble && rumble_large == 0 && rumble_small == 0)
                        || release_triggers;
                    if can_write {
                        let update_lb = dirty_lightbar && (lightbar_controlled || game_rgb.is_some());
                        let effective_rgb = if lightbar_controlled {
                            last_rgb
                        } else {
                            game_rgb.unwrap_or(last_rgb)
                        };
                        let update_leds =
                            dirty_player_leds && (prof.override_lightbar || game_leds.is_some());
                        let effective_leds = if prof.override_lightbar {
                            last_leds
                        } else {
                            game_leds.unwrap_or(last_leds)
                        };
                        let update_bright = dirty_led_brightness && prof.override_lightbar;
                        let update_mute = dirty_mic_mute_led && prof.mic_mute_led.to_protocol().is_some();
                        let mute_val = prof.mic_mute_led.to_protocol().unwrap_or(MicMuteLed::Off);
                        let update_left_trig = release_triggers || dirty_left_trigger;
                        let update_right_trig = release_triggers || dirty_right_trigger;
                        let effective_left_trig = if release_triggers {
                            AdaptiveTriggerEffect::Off
                        } else if prof.override_triggers {
                            last_left_trigger
                        } else {
                            game_left_trigger
                                .map(AdaptiveTriggerEffect::Raw)
                                .unwrap_or(AdaptiveTriggerEffect::Off)
                        };
                        let effective_right_trig = if release_triggers {
                            AdaptiveTriggerEffect::Off
                        } else if prof.override_triggers {
                            last_right_trigger
                        } else {
                            game_right_trigger
                                .map(AdaptiveTriggerEffect::Raw)
                                .unwrap_or(AdaptiveTriggerEffect::Off)
                        };

                        flush_output(
                            &mut device,
                            &OutputState {
                                update_rumble: dirty_rumble && !bt_hap_active,
                                motor_left: rumble_large,
                                motor_right: rumble_small,
                                update_headphone_volume: dirty_bt_audio,
                                headphone_volume: bt_hp_vol_byte,
                                update_speaker_volume: dirty_bt_audio,
                                speaker_volume: bt_spk_vol_byte,
                                update_mic_volume: dirty_bt_audio,
                                mic_volume: DS_BT_MIC_VOLUME_MAX,
                                update_audio_control: dirty_bt_audio,
                                audio_control: prof.bt_audio_route.audio_control_byte(),
                                unmute_audio: dirty_bt_audio,
                                update_mic_mute_led: update_mute,
                                mic_mute_led: mute_val,
                                update_lightbar: update_lb,
                                lightbar_rgb: effective_rgb,
                                update_player_leds: update_leds,
                                player_leds: effective_leds,
                                update_led_brightness: update_bright,
                                led_brightness: last_led_brightness,
                                update_left_trigger: update_left_trig,
                                left_trigger: effective_left_trig,
                                update_right_trigger: update_right_trig,
                                right_trigger: effective_right_trig,
                                ..Default::default()
                            },
                            &mut bt_seq,
                        );
                        if release_triggers {
                            triggers_touched = false;
                        }
                        dirty_rumble = false;
                        dirty_lightbar = false;
                        dirty_player_leds = false;
                        dirty_led_brightness = false;
                        dirty_mic_mute_led = false;
                        dirty_left_trigger = false;
                        dirty_right_trigger = false;
                        release_triggers = false;
                        dirty_bt_audio = false;
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
            ReadOutcome::Timeout => {
                let (test_seq, t_large, t_small, t_dur) = test_rumble.snapshot();
                if test_seq != last_test_seq {
                    last_test_seq = test_seq;
                    let until = Instant::now() + Duration::from_millis(t_dur.max(50) as u64);
                    active_test_rumble = Some((until, t_large, t_small));
                }
                if let Some((until, _, _)) = active_test_rumble {
                    if Instant::now() >= until {
                        active_test_rumble = None;
                    }
                }
                if active_test_rumble.is_none() && game_rumble_large == 0 && game_rumble_small == 0 && (rumble_large != 0 || rumble_small != 0) {
                    rumble_large = 0;
                    rumble_small = 0;
                    flush_output(
                        &mut device,
                        &OutputState {
                            update_rumble: true,
                            motor_left: 0,
                            motor_right: 0,
                            ..Default::default()
                        },
                        &mut bt_seq,
                    );
                }
                let prof = profile_swap.load();
                let bt_spk_test_active = bt_audio_worker
                    .as_ref()
                    .is_some_and(|w| w.telemetry.speaker_test_active.load(Ordering::Acquire));
                let bt_hap_test_active = bt_audio_worker
                    .as_ref()
                    .is_some_and(|w| w.telemetry.haptics_test_active.load(Ordering::Acquire));
                let bt_any_audio = info.transport == Transport::Bluetooth
                    && (prof.bt_haptics_enabled
                        || prof.virtual_audio_device
                        || bt_spk_test_active
                        || bt_hap_test_active);
                if let Some(br) = bt_audio_bridge.as_ref() {
                    if prof.virtual_audio_device {
                        if virtual_audio.is_none() && !virtual_audio_attempted {
                            virtual_audio_attempted = true;
                            match VirtualAudioDevice::spawn(Arc::clone(br)) {
                                Ok(dev) => virtual_audio = Some(dev),
                                Err(e) => warn!(error = %e, "Failed to attach Virtual DualSense Audio"),
                            }
                        }
                    } else {
                        if virtual_audio.is_some() {
                            virtual_audio = None;
                        }
                        virtual_audio_attempted = false;
                    }
                }
                if bt_any_audio {
                    let bt_vol_norm = if prof.virtual_audio_device {
                        prof.bt_speaker_volume
                    } else {
                        0.85
                    };
                    let bt_spk_vol_byte = scale_speaker_volume(bt_vol_norm);
                    let bt_hp_vol_byte = scale_headphone_volume(bt_vol_norm);
                    let bt_hap_active = prof.bt_haptics_enabled || bt_hap_test_active;
                    let virtual_speaker_open = bt_audio_bridge
                        .as_ref()
                        .is_some_and(|b| b.speaker_endpoint_open.load(Ordering::Acquire));
                    let bt_hw_key = (
                        true,
                        bt_spk_vol_byte,
                        bt_hp_vol_byte,
                        prof.bt_audio_route,
                        bt_hap_active,
                        virtual_speaker_open,
                    );
                    if last_bt_audio_hw != Some(bt_hw_key) {
                        last_bt_audio_hw = Some(bt_hw_key);
                        flush_output(
                            &mut device,
                            &OutputState {
                                update_headphone_volume: true,
                                headphone_volume: bt_hp_vol_byte,
                                update_speaker_volume: true,
                                speaker_volume: bt_spk_vol_byte,
                                update_mic_volume: true,
                                mic_volume: DS_BT_MIC_VOLUME_MAX,
                                update_audio_control: true,
                                audio_control: prof.bt_audio_route.audio_control_byte(),
                                unmute_audio: true,
                                ..Default::default()
                            },
                            &mut bt_seq,
                        );
                    }
                }
                if turbo_state.is_active() {
                    if let Some(mut base) = last_base_proc {
                        apply_turbo_state(&mut base, &prof.turbo, &mut turbo_state, Instant::now());
                        if let Some(vc) = target.as_deref_mut() {
                            let _ = vc.submit(&base);
                        }
                        base
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            ReadOutcome::Cancelled | ReadOutcome::Disconnected => break,
        };

        let (
            bt_audio_streaming,
            bt_haptics_streaming,
            bt_speaker_level,
            bt_haptics_level,
            bt_mic_streaming,
            bt_mic_level,
        ) = if let Some(w) = bt_audio_worker.as_ref() {
            (
                w.telemetry.speaker_streaming.load(Ordering::Acquire),
                w.telemetry.haptics_streaming.load(Ordering::Acquire),
                w.telemetry.speaker_level.load(Ordering::Acquire),
                w.telemetry.haptics_level.load(Ordering::Acquire),
                w.telemetry.mic_streaming.load(Ordering::Acquire),
                w.telemetry.mic_level.load(Ordering::Acquire),
            )
        } else {
            (false, false, 0, 0, false, 0)
        };
        let (virtual_audio_attached, virtual_speaker_active, virtual_mic_active) =
            if let Some(br) = bt_audio_bridge.as_ref() {
                (
                    br.virtual_audio_attached.load(Ordering::Acquire),
                    br.speaker_endpoint_open.load(Ordering::Acquire),
                    br.mic_endpoint_open.load(Ordering::Acquire),
                )
            } else {
                (false, false, false)
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
            touch: processed.touch,
            active_lightbar_rgb: if last_override_lightbar || is_low_battery_alert(&profile_swap.load(), last_battery_pct, last_battery_status) {
                Some(last_rgb)
            } else {
                game_rgb
            },
            product_id: info.product_id,
            mac_address,
            firmware_version,
            hardware_version,
            firmware_date,
            connected_secs: session_start.elapsed().as_secs() as u32,
            idle_secs: last_activity.elapsed().as_secs() as u32,
            headphone_connected: processed.headphone_connected,
            mic_connected: processed.mic_connected,
            mic_muted_hw: processed.mic_muted_hw,
            gyro_dps: [
                processed.calibrated_gyro[0] as f32 / (DS_GYRO_RES_PER_DEG_S as f32),
                processed.calibrated_gyro[1] as f32 / (DS_GYRO_RES_PER_DEG_S as f32),
                processed.calibrated_gyro[2] as f32 / (DS_GYRO_RES_PER_DEG_S as f32),
            ],
            accel_g: [
                processed.accel[0] as f32 / (DS_ACC_RES_PER_G as f32),
                processed.accel[1] as f32 / (DS_ACC_RES_PER_G as f32),
                processed.accel[2] as f32 / (DS_ACC_RES_PER_G as f32),
            ],
            bt_audio_streaming,
            bt_haptics_streaming,
            bt_speaker_level,
            bt_haptics_level,
            bt_mic_streaming,
            bt_mic_level,
            virtual_audio_attached,
            virtual_speaker_active,
            virtual_mic_active,
        });
    }

    drop(virtual_audio);
    if let Some(mut w) = bt_audio_worker.take() {
        w.stop();
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

fn build_virtual_dualsense_config(
    info: &DeviceInfo,
    raw_calibration_05: Option<[u8; DS_FEATURE_REPORT_CALIBRATION_SIZE]>,
) -> VirtualDualSenseConfig {
    let seed = if !info.instance_id.is_empty() {
        info.instance_id.as_bytes()
    } else {
        info.device_path.as_bytes()
    };
    let mut h: u32 = 0x811C_9DC5;
    for &b in seed {
        h ^= b.to_ascii_uppercase() as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let tag = ((h ^ (h >> 16)) & 0xFFFF) as u16;
    let tag = if tag == 0 { 1 } else { tag };
    let hi = (tag >> 8) as u8;
    let lo = (tag & 0xFF) as u8;

    VirtualDualSenseConfig {
        raw_calibration_05,
        // Locally-administered unicast MAC ("DSM") + deterministic 16-bit controller tag.
        virtual_mac: [0x02, 0x44, 0x53, 0x4D, hi, lo],
        serial_tag: format!("{VIRTUAL_SERIAL_PREFIX}-{tag:04X}"),
    }
}

fn reconcile_target(
    desired: EmulationMode,
    backends: &EmuBackends,
    info: &DeviceInfo,
    raw_calibration_05: Option<[u8; DS_FEATURE_REPORT_CALIBRATION_SIZE]>,
    active: &mut EmulationMode,
    target: &mut Option<Box<dyn VirtualController>>,
) {
    *target = None;
    *active = EmulationMode::None;
    match desired {
        EmulationMode::None => {}
        EmulationMode::Xbox360 => {
            if let Some(client) = &backends.vigem {
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
            if let Some(client) = &backends.vigem {
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
        EmulationMode::DualSense => {
            let cfg = build_virtual_dualsense_config(info, raw_calibration_05);
            match backends.spawn_dualsense(cfg) {
                Ok(t) => {
                    *target = Some(t);
                    *active = EmulationMode::DualSense;
                }
                Err(e) => warn!(error = %e, "Failed to plug in Virtual DualSense target"),
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

#[inline]
pub fn is_low_battery_alert(prof: &ControllerProfile, battery_pct: u8, status: BatteryStatus) -> bool {
    prof.low_battery_alert
        && status == BatteryStatus::Discharging
        && battery_pct <= prof.low_battery_threshold.clamp(5, 50)
}

fn hue_to_rgb(hue_deg: f32) -> [u8; 3] {
    let h = hue_deg.rem_euclid(360.0) / 60.0;
    let x = (1.0 - ((h % 2.0) - 1.0).abs()) * 255.0;
    let (r, g, b) = if h < 1.0 {
        (255.0, x, 0.0)
    } else if h < 2.0 {
        (x, 255.0, 0.0)
    } else if h < 3.0 {
        (0.0, 255.0, x)
    } else if h < 4.0 {
        (0.0, x, 255.0)
    } else if h < 5.0 {
        (x, 0.0, 255.0)
    } else {
        (255.0, 0.0, x)
    };
    [r.round() as u8, g.round() as u8, b.round() as u8]
}

pub fn compute_animated_lightbar(
    prof: &ControllerProfile,
    elapsed_secs: f32,
    battery_pct: u8,
    battery_status: BatteryStatus,
    max_trigger: u8,
    idle_secs: u32,
) -> [u8; 3] {
    let bright = prof.lightbar_brightness.clamp(0.0, 1.0);
    let speed = prof.lightbar_speed.clamp(0.25, 3.0);

    if is_low_battery_alert(prof, battery_pct, battery_status) {
        let pulse = 0.25 + 0.75 * (0.5 + 0.5 * (elapsed_secs * 6.0).sin());
        return [(240.0 * pulse).round() as u8, 12, 12];
    }

    let base = match prof.lightbar_mode {
        LightbarMode::Static => prof.lightbar_rgb,
        LightbarMode::Breathing => {
            let phase = 0.15 + 0.85 * (0.5 + 0.5 * (elapsed_secs * speed * 2.1).sin());
            [
                ((prof.lightbar_rgb[0] as f32) * phase).round() as u8,
                ((prof.lightbar_rgb[1] as f32) * phase).round() as u8,
                ((prof.lightbar_rgb[2] as f32) * phase).round() as u8,
            ]
        }
        LightbarMode::Rainbow => hue_to_rgb(elapsed_secs * speed * 90.0),
        LightbarMode::BatteryIndicator => {
            if battery_status == BatteryStatus::Full || battery_pct >= 75 {
                [32, 210, 96]
            } else if battery_pct >= 40 {
                [235, 175, 42]
            } else {
                [235, 56, 56]
            }
        }
        LightbarMode::TriggerReactive => {
            let t = (max_trigger as f32 / 255.0).clamp(0.0, 1.0);
            let boost = 0.20 + 0.80 * t;
            let [r, g, b] = prof.lightbar_rgb;
            [
                ((r as f32) * boost).round().min(255.0) as u8,
                ((g as f32) * boost).round().min(255.0) as u8,
                ((b as f32) * boost).round().min(255.0) as u8,
            ]
        }
    };

    let idle_factor = if prof.idle_dim_secs > 0 && idle_secs >= prof.idle_dim_secs as u32 {
        0.15
    } else {
        1.0
    };
    let scale = bright * idle_factor;
    [
        ((base[0] as f32) * scale).round().clamp(0.0, 255.0) as u8,
        ((base[1] as f32) * scale).round().clamp(0.0, 255.0) as u8,
        ((base[2] as f32) * scale).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_lightbar_modes_low_battery_and_idle_dim() {
        let mut prof = ControllerProfile {
            override_lightbar: true,
            lightbar_rgb: [100, 200, 50],
            lightbar_mode: LightbarMode::Static,
            lightbar_brightness: 0.5,
            low_battery_alert: true,
            low_battery_threshold: 20,
            idle_dim_secs: 60,
            ..Default::default()
        };
        assert_eq!(
            compute_animated_lightbar(&prof, 0.0, 80, BatteryStatus::Discharging, 0, 0),
            [50, 100, 25]
        );
        // Idle dim scales by 0.15
        assert_eq!(
            compute_animated_lightbar(&prof, 0.0, 80, BatteryStatus::Discharging, 0, 65),
            [8, 15, 4]
        );
        // Low battery alert overrides to pulsing red
        let lb = compute_animated_lightbar(&prof, 0.25, 15, BatteryStatus::Discharging, 0, 0);
        assert!(lb[0] > lb[1] && lb[0] > lb[2]);
        // Rainbow progresses hues
        prof.lightbar_mode = LightbarMode::Rainbow;
        prof.lightbar_brightness = 1.0;
        let r0 = compute_animated_lightbar(&prof, 0.0, 80, BatteryStatus::Discharging, 0, 0);
        let r1 = compute_animated_lightbar(&prof, 1.5, 80, BatteryStatus::Discharging, 0, 0);
        assert_ne!(r0, r1);
        // Battery indicator turns green when >=75%
        prof.lightbar_mode = LightbarMode::BatteryIndicator;
        assert_eq!(
            compute_animated_lightbar(&prof, 0.0, 90, BatteryStatus::Discharging, 0, 0),
            [32, 210, 96]
        );
    }
}
