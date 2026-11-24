use std::time::Instant;

use super::profile::{
    ButtonMap, ControllerProfile, DeadzoneShape, GyroAimMode, GyroOutputMode, RemappableButton,
    StickTuning, TriggerTuning, TurboConfig,
};
use crate::protocol::calibration::ImuCalibration;
use crate::protocol::input::{DPad, TouchPoint};
use crate::protocol::{BatteryStatus, DS_GYRO_RES_PER_DEG_S, DualSenseButtons, RawInputState};

#[derive(Debug, Clone, Copy)]
pub struct ProcessedInput {
    pub raw_left_x: u8,
    pub raw_left_y: u8,
    pub raw_right_x: u8,
    pub raw_right_y: u8,
    pub left_x: u8,
    pub left_y: u8,
    pub right_x: u8,
    pub right_y: u8,
    pub l2_analog: u8,
    pub r2_analog: u8,
    pub seq_number: u8,
    pub dpad: DPad,
    pub buttons: DualSenseButtons,
    pub gyro: [i16; 3],
    pub accel: [i16; 3],
    pub calibrated_gyro: [i32; 3],
    pub sensor_timestamp: u32,
    pub touch: [TouchPoint; 2],
    pub battery_percent: u8,
    pub battery_status: BatteryStatus,
    pub headphone_connected: bool,
    pub mic_connected: bool,
    pub mic_muted_hw: bool,
}

impl ProcessedInput {
    /// Converts the processed controller state back into a `RawInputState` suitable for
    /// encoding into a 64-byte DualSense USB HID Input Report (`0x01`).
    #[inline]
    pub const fn to_raw_input_state(&self) -> RawInputState {
        RawInputState {
            left_x: self.left_x,
            left_y: self.left_y,
            right_x: self.right_x,
            right_y: self.right_y,
            l2_analog: self.l2_analog,
            r2_analog: self.r2_analog,
            seq_number: self.seq_number,
            dpad: self.dpad,
            buttons: self.buttons,
            gyro: self.gyro,
            accel: self.accel,
            sensor_timestamp: self.sensor_timestamp,
            touch: self.touch,
            battery_percent: self.battery_percent,
            battery_status: self.battery_status,
            headphone_connected: self.headphone_connected,
            mic_connected: self.mic_connected,
            mic_muted_hw: self.mic_muted_hw,
        }
    }
}

#[inline]
fn map_scalar_axis(val: f32, dz: f32, outer: f32, anti: f32, tuning: &StickTuning) -> f32 {
    let abs_v = val.abs();
    if abs_v <= dz || abs_v < 1e-6 {
        return 0.0;
    }
    let span = (1.0 - dz - outer).max(0.05);
    let norm = ((abs_v - dz) / span).clamp(0.0, 1.0);
    let curved = tuning.curve.apply(norm);
    let out_mag = if curved > 0.0 {
        (anti + curved * (1.0 - anti)).min(1.0)
    } else {
        0.0
    };
    val.signum() * out_mag
}

#[inline]
fn process_stick(raw_x: u8, raw_y: u8, tuning: &StickTuning) -> [u8; 2] {
    let mut nx = (raw_x as f32 - 128.0) / 127.5;
    let mut ny = (raw_y as f32 - 128.0) / 127.5;
    if tuning.invert_x {
        nx = -nx;
    }
    if tuning.invert_y {
        ny = -ny;
    }

    let dz = tuning.deadzone.clamp(0.0, 0.49);
    let outer = tuning.outer_deadzone.clamp(0.0, 0.40);
    let anti = tuning.anti_deadzone.clamp(0.0, 0.45);

    let (ox_f, oy_f) = match tuning.shape {
        DeadzoneShape::Passthrough => (nx.clamp(-1.0, 1.0), ny.clamp(-1.0, 1.0)),
        DeadzoneShape::Axial => (
            map_scalar_axis(nx, dz, outer, anti, tuning),
            map_scalar_axis(ny, dz, outer, anti, tuning),
        ),
        DeadzoneShape::Radial => {
            let mag = (nx * nx + ny * ny).sqrt();
            if mag <= dz || mag < 1e-6 {
                (0.0, 0.0)
            } else {
                let span = (1.0 - dz - outer).max(0.05);
                let norm_mag = ((mag - dz) / span).clamp(0.0, 1.0);
                let curved = tuning.curve.apply(norm_mag);
                let out_mag = if curved > 0.0 {
                    (anti + curved * (1.0 - anti)).min(1.0)
                } else {
                    0.0
                };
                let scale = out_mag / mag;
                (nx * scale, ny * scale)
            }
        }
    };

    let ox = (ox_f * 127.5 + 128.0).round().clamp(0.0, 255.0) as u8;
    let oy = (oy_f * 127.5 + 128.0).round().clamp(0.0, 255.0) as u8;
    [ox, oy]
}

#[inline]
fn process_trigger(raw: u8, tuning: &TriggerTuning) -> u8 {
    if raw <= tuning.deadzone {
        return 0;
    }
    let max_in = tuning.hair_trigger.max(tuning.deadzone.saturating_add(1));
    if raw >= max_in {
        return 255;
    }
    let span = (max_in - tuning.deadzone) as u32;
    let val = ((raw - tuning.deadzone) as u32 * 255) / span;
    val.min(255) as u8
}

#[inline]
fn dpad_to_cardinals(dpad: DPad) -> (bool, bool, bool, bool) {
    match dpad {
        DPad::North => (true, false, false, false),
        DPad::NorthEast => (true, false, false, true),
        DPad::East => (false, false, false, true),
        DPad::SouthEast => (false, true, false, true),
        DPad::South => (false, true, false, false),
        DPad::SouthWest => (false, true, true, false),
        DPad::West => (false, false, true, false),
        DPad::NorthWest => (true, false, true, false),
        DPad::Center => (false, false, false, false),
    }
}

#[inline]
fn cardinals_to_dpad(up: bool, down: bool, left: bool, right: bool) -> DPad {
    let v = match (up, down) {
        (true, false) => 1i8,
        (false, true) => -1i8,
        _ => 0i8,
    };
    let h = match (left, right) {
        (false, true) => 1i8,
        (true, false) => -1i8,
        _ => 0i8,
    };
    match (v, h) {
        (1, 0) => DPad::North,
        (1, 1) => DPad::NorthEast,
        (0, 1) => DPad::East,
        (-1, 1) => DPad::SouthEast,
        (-1, 0) => DPad::South,
        (-1, -1) => DPad::SouthWest,
        (0, -1) => DPad::West,
        (1, -1) => DPad::NorthWest,
        _ => DPad::Center,
    }
}

#[inline]
fn apply_button_map(
    raw_buttons: DualSenseButtons,
    raw_dpad: DPad,
    in_l2: u8,
    in_r2: u8,
    map: &ButtonMap,
) -> (DualSenseButtons, DPad, u8, u8) {
    let (raw_up, raw_down, raw_left, raw_right) = dpad_to_cardinals(raw_dpad);
    let mut out_buttons = raw_buttons
        & (DualSenseButtons::PS_HOME
            | DualSenseButtons::EDGE_FN1
            | DualSenseButtons::EDGE_FN2
            | DualSenseButtons::EDGE_PADDLE_L
            | DualSenseButtons::EDGE_PADDLE_R);
    let mut up = false;
    let mut down = false;
    let mut left = false;
    let mut right = false;

    let mut out_l2 = if map.l2 == RemappableButton::L2 {
        in_l2
    } else {
        0
    };
    let mut out_r2 = if map.r2 == RemappableButton::R2 {
        in_r2
    } else {
        0
    };

    let sources = [
        (RemappableButton::Cross, raw_buttons.contains(DualSenseButtons::CROSS)),
        (RemappableButton::Circle, raw_buttons.contains(DualSenseButtons::CIRCLE)),
        (RemappableButton::Square, raw_buttons.contains(DualSenseButtons::SQUARE)),
        (RemappableButton::Triangle, raw_buttons.contains(DualSenseButtons::TRIANGLE)),
        (RemappableButton::L1, raw_buttons.contains(DualSenseButtons::L1)),
        (RemappableButton::R1, raw_buttons.contains(DualSenseButtons::R1)),
        (RemappableButton::L2, in_l2 >= 32),
        (RemappableButton::R2, in_r2 >= 32),
        (RemappableButton::L3, raw_buttons.contains(DualSenseButtons::L3)),
        (RemappableButton::R3, raw_buttons.contains(DualSenseButtons::R3)),
        (RemappableButton::Options, raw_buttons.contains(DualSenseButtons::OPTIONS)),
        (RemappableButton::Create, raw_buttons.contains(DualSenseButtons::CREATE)),
        (RemappableButton::Touchpad, raw_buttons.contains(DualSenseButtons::TOUCHPAD)),
        (RemappableButton::MicMute, raw_buttons.contains(DualSenseButtons::MIC_MUTE)),
        (RemappableButton::DpadUp, raw_up),
        (RemappableButton::DpadDown, raw_down),
        (RemappableButton::DpadLeft, raw_left),
        (RemappableButton::DpadRight, raw_right),
    ];

    for (src, pressed) in sources {
        if !pressed {
            continue;
        }
        match map.get(src) {
            RemappableButton::None => {}
            RemappableButton::Cross => out_buttons |= DualSenseButtons::CROSS,
            RemappableButton::Circle => out_buttons |= DualSenseButtons::CIRCLE,
            RemappableButton::Square => out_buttons |= DualSenseButtons::SQUARE,
            RemappableButton::Triangle => out_buttons |= DualSenseButtons::TRIANGLE,
            RemappableButton::L1 => out_buttons |= DualSenseButtons::L1,
            RemappableButton::R1 => out_buttons |= DualSenseButtons::R1,
            RemappableButton::L2 => {
                if src != RemappableButton::L2 {
                    out_l2 = out_l2.max(255);
                }
            }
            RemappableButton::R2 => {
                if src != RemappableButton::R2 {
                    out_r2 = out_r2.max(255);
                }
            }
            RemappableButton::L3 => out_buttons |= DualSenseButtons::L3,
            RemappableButton::R3 => out_buttons |= DualSenseButtons::R3,
            RemappableButton::Options => out_buttons |= DualSenseButtons::OPTIONS,
            RemappableButton::Create => out_buttons |= DualSenseButtons::CREATE,
            RemappableButton::Touchpad => out_buttons |= DualSenseButtons::TOUCHPAD,
            RemappableButton::MicMute => out_buttons |= DualSenseButtons::MIC_MUTE,
            RemappableButton::DpadUp => up = true,
            RemappableButton::DpadDown => down = true,
            RemappableButton::DpadLeft => left = true,
            RemappableButton::DpadRight => right = true,
        }
    }

    out_buttons.set(DualSenseButtons::L2_DIGITAL, out_l2 >= 32);
    out_buttons.set(DualSenseButtons::R2_DIGITAL, out_r2 >= 32);
    (out_buttons, cardinals_to_dpad(up, down, left, right), out_l2, out_r2)
}

#[derive(Debug, Clone, Copy)]
pub struct TurboState {
    hold_start: Option<Instant>,
    last_target: RemappableButton,
}

impl Default for TurboState {
    fn default() -> Self {
        Self {
            hold_start: None,
            last_target: RemappableButton::None,
        }
    }
}

impl TurboState {
    #[inline]
    pub fn is_active(&self) -> bool {
        self.hold_start.is_some()
    }
}

#[inline]
pub fn is_button_pressed(proc_in: &ProcessedInput, btn: RemappableButton) -> bool {
    let (up, down, left, right) = dpad_to_cardinals(proc_in.dpad);
    match btn {
        RemappableButton::None => false,
        RemappableButton::Cross => proc_in.buttons.contains(DualSenseButtons::CROSS),
        RemappableButton::Circle => proc_in.buttons.contains(DualSenseButtons::CIRCLE),
        RemappableButton::Square => proc_in.buttons.contains(DualSenseButtons::SQUARE),
        RemappableButton::Triangle => proc_in.buttons.contains(DualSenseButtons::TRIANGLE),
        RemappableButton::L1 => proc_in.buttons.contains(DualSenseButtons::L1),
        RemappableButton::R1 => proc_in.buttons.contains(DualSenseButtons::R1),
        RemappableButton::L2 => proc_in.l2_analog >= 32,
        RemappableButton::R2 => proc_in.r2_analog >= 32,
        RemappableButton::L3 => proc_in.buttons.contains(DualSenseButtons::L3),
        RemappableButton::R3 => proc_in.buttons.contains(DualSenseButtons::R3),
        RemappableButton::Options => proc_in.buttons.contains(DualSenseButtons::OPTIONS),
        RemappableButton::Create => proc_in.buttons.contains(DualSenseButtons::CREATE),
        RemappableButton::Touchpad => proc_in.buttons.contains(DualSenseButtons::TOUCHPAD),
        RemappableButton::MicMute => proc_in.buttons.contains(DualSenseButtons::MIC_MUTE),
        RemappableButton::DpadUp => up,
        RemappableButton::DpadDown => down,
        RemappableButton::DpadLeft => left,
        RemappableButton::DpadRight => right,
    }
}

/// Applies Turbo rapid-fire gating anchored to initial hold time so the first press always fires immediately.
#[inline]
pub fn apply_turbo_state(
    proc_in: &mut ProcessedInput,
    turbo: &TurboConfig,
    state: &mut TurboState,
    now: Instant,
) -> bool {
    if !turbo.enabled || turbo.target == RemappableButton::None {
        state.hold_start = None;
        return false;
    }
    if state.last_target != turbo.target {
        state.hold_start = None;
        state.last_target = turbo.target;
    }
    if is_button_pressed(proc_in, turbo.target) {
        let start = *state.hold_start.get_or_insert(now);
        let elapsed_ms = now.saturating_duration_since(start).as_millis() as u64;
        apply_turbo(proc_in, turbo, elapsed_ms);
        true
    } else {
        state.hold_start = None;
        false
    }
}

/// Applies 50% duty-cycle rapid-fire gating to `proc_in` when `turbo.enabled` is true.
#[inline]
pub fn apply_turbo(proc_in: &mut ProcessedInput, turbo: &TurboConfig, elapsed_ms: u64) {
    if !turbo.enabled || turbo.target == RemappableButton::None {
        return;
    }
    let rate = turbo.rate_hz.clamp(2, 30) as u64;
    let phase_on = ((elapsed_ms.saturating_mul(rate).saturating_mul(2)) / 1000).is_multiple_of(2);
    if phase_on {
        return;
    }
    let (mut up, mut down, mut left, mut right) = dpad_to_cardinals(proc_in.dpad);
    match turbo.target {
        RemappableButton::None => return,
        RemappableButton::Cross => proc_in.buttons.remove(DualSenseButtons::CROSS),
        RemappableButton::Circle => proc_in.buttons.remove(DualSenseButtons::CIRCLE),
        RemappableButton::Square => proc_in.buttons.remove(DualSenseButtons::SQUARE),
        RemappableButton::Triangle => proc_in.buttons.remove(DualSenseButtons::TRIANGLE),
        RemappableButton::L1 => proc_in.buttons.remove(DualSenseButtons::L1),
        RemappableButton::R1 => proc_in.buttons.remove(DualSenseButtons::R1),
        RemappableButton::L2 => {
            proc_in.buttons.remove(DualSenseButtons::L2_DIGITAL);
            proc_in.l2_analog = 0;
        }
        RemappableButton::R2 => {
            proc_in.buttons.remove(DualSenseButtons::R2_DIGITAL);
            proc_in.r2_analog = 0;
        }
        RemappableButton::L3 => proc_in.buttons.remove(DualSenseButtons::L3),
        RemappableButton::R3 => proc_in.buttons.remove(DualSenseButtons::R3),
        RemappableButton::Options => proc_in.buttons.remove(DualSenseButtons::OPTIONS),
        RemappableButton::Create => proc_in.buttons.remove(DualSenseButtons::CREATE),
        RemappableButton::Touchpad => proc_in.buttons.remove(DualSenseButtons::TOUCHPAD),
        RemappableButton::MicMute => proc_in.buttons.remove(DualSenseButtons::MIC_MUTE),
        RemappableButton::DpadUp => up = false,
        RemappableButton::DpadDown => down = false,
        RemappableButton::DpadLeft => left = false,
        RemappableButton::DpadRight => right = false,
    }
    proc_in.dpad = cardinals_to_dpad(up, down, left, right);
}

#[inline]
pub fn is_gyro_active(
    mode: GyroAimMode,
    l2_analog: u8,
    r2_analog: u8,
    buttons: DualSenseButtons,
    touch0_active: bool,
) -> bool {
    match mode {
        GyroAimMode::Off => false,
        GyroAimMode::HoldL2 => l2_analog > 32,
        GyroAimMode::HoldL1 => buttons.contains(DualSenseButtons::L1),
        GyroAimMode::HoldR2 => r2_analog > 32,
        GyroAimMode::HoldR1 => buttons.contains(DualSenseButtons::R1),
        GyroAimMode::TouchpadTouch => touch0_active,
        GyroAimMode::AlwaysOn => true,
    }
}

/// Computes `(horizontal_deg_s, vertical_deg_s)` from calibrated IMU gyro with deadzone & inversion.
#[inline]
pub fn compute_gyro_delta(
    calibrated_gyro: [i32; 3],
    l2_analog: u8,
    r2_analog: u8,
    buttons: DualSenseButtons,
    touch0_active: bool,
    profile: &ControllerProfile,
) -> Option<(f32, f32)> {
    if !is_gyro_active(
        profile.gyro_mode,
        l2_analog,
        r2_analog,
        buttons,
        touch0_active,
    ) {
        return None;
    }

    let pitch = -(calibrated_gyro[0] as f32) / (DS_GYRO_RES_PER_DEG_S as f32);
    let yaw = -(calibrated_gyro[1] as f32) / (DS_GYRO_RES_PER_DEG_S as f32);
    let roll = -(calibrated_gyro[2] as f32) / (DS_GYRO_RES_PER_DEG_S as f32);

    let mut gx = (if profile.gyro_use_yaw { yaw } else { 0.0 })
        + (if profile.gyro_use_roll { roll } else { 0.0 });
    let mut gy = pitch;

    let mag = gx.hypot(gy);
    let dz = profile.gyro_deadzone.clamp(0.0, 15.0);
    if mag <= dz || mag < 1e-6 {
        return Some((0.0, 0.0));
    }
    let scale = (mag - dz) / mag;
    gx *= scale;
    gy *= scale;

    if profile.gyro_invert_x {
        gx = -gx;
    }
    if profile.gyro_invert_y {
        gy = -gy;
    }
    Some((gx, gy))
}

#[inline]
pub fn process_input(
    raw: &RawInputState,
    cal: &ImuCalibration,
    profile: &ControllerProfile,
) -> ProcessedInput {
    let left_raw_proc = process_stick(raw.left_x, raw.left_y, &profile.left_stick);
    let right_raw_proc = process_stick(raw.right_x, raw.right_y, &profile.right_stick);
    let (left, right) = if profile.swap_sticks {
        (right_raw_proc, left_raw_proc)
    } else {
        (left_raw_proc, right_raw_proc)
    };
    let l2_in = process_trigger(raw.l2_analog, &profile.left_trigger_tuning());
    let r2_in = process_trigger(raw.r2_analog, &profile.right_trigger_tuning());
    let (buttons, dpad, l2_analog, r2_analog) =
        apply_button_map(raw.buttons, raw.dpad, l2_in, r2_in, &profile.button_map);

    let calibrated_gyro = [
        cal.gyro[0].apply(raw.gyro[0]),
        cal.gyro[1].apply(raw.gyro[1]),
        cal.gyro[2].apply(raw.gyro[2]),
    ];

    let mut right_x = right[0];
    let mut right_y = right[1];
    if profile.gyro_output == GyroOutputMode::RightStick {
        if let Some((gx, gy)) = compute_gyro_delta(
            calibrated_gyro,
            l2_analog,
            r2_analog,
            buttons,
            raw.touch[0].active,
            profile,
        ) {
            let sens = profile.gyro_sensitivity.clamp(0.1, 5.0) * 1.4;
            right_x = ((right_x as f32) + gx * sens).round().clamp(0.0, 255.0) as u8;
            right_y = ((right_y as f32) + gy * sens).round().clamp(0.0, 255.0) as u8;
        }
    }

    ProcessedInput {
        raw_left_x: raw.left_x,
        raw_left_y: raw.left_y,
        raw_right_x: raw.right_x,
        raw_right_y: raw.right_y,
        left_x: left[0],
        left_y: left[1],
        right_x,
        right_y,
        l2_analog,
        r2_analog,
        seq_number: raw.seq_number,
        dpad,
        buttons,
        gyro: raw.gyro,
        accel: raw.accel,
        calibrated_gyro,
        sensor_timestamp: raw.sensor_timestamp,
        touch: raw.touch,
        battery_percent: raw.battery_percent,
        battery_status: raw.battery_status,
        headphone_connected: raw.headphone_connected,
        mic_connected: raw.mic_connected,
        mic_muted_hw: raw.mic_muted_hw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::profile::ResponseCurve;

    #[test]
    fn deadzone_centers_stick_and_ramps_outside() {
        let tuning = StickTuning {
            deadzone: 0.15,
            anti_deadzone: 0.0,
            curve: ResponseCurve::Linear,
            ..Default::default()
        };
        assert_eq!(process_stick(128, 128, &tuning), [128, 128]);
        assert_eq!(process_stick(138, 130, &tuning), [128, 128]);
        let [ox, oy] = process_stick(255, 128, &tuning);
        assert_eq!(ox, 255);
        assert_eq!(oy, 128);
    }

    #[test]
    fn anti_deadzone_boosts_initial_deflection() {
        let tuning = StickTuning {
            deadzone: 0.05,
            anti_deadzone: 0.25,
            curve: ResponseCurve::Linear,
            ..Default::default()
        };
        let [ox, _] = process_stick(142, 128, &tuning);
        assert!(ox >= 160);
    }

    #[test]
    fn outer_deadzone_axial_shape_invert_and_swap_sticks() {
        let outer = StickTuning {
            deadzone: 0.05,
            outer_deadzone: 0.25,
            ..Default::default()
        };
        // 0.80 deflection >= 1.0 - 0.25 = 0.75 should saturate to 255
        let [ox, _] = process_stick(230, 128, &outer);
        assert_eq!(ox, 255);

        let axial = StickTuning {
            deadzone: 0.20,
            shape: DeadzoneShape::Axial,
            invert_y: true,
            ..Default::default()
        };
        // X is within 0.20 axial deadzone (~140), Y is outside (255 -> +127 from 128, inverted -> 128 - 127 = 1)
        let [ax, ay] = process_stick(140, 255, &axial);
        assert_eq!(ax, 128);
        assert_eq!(ay, 1);

        let raw = RawInputState {
            left_x: 255,
            left_y: 128,
            right_x: 128,
            right_y: 0,
            ..Default::default()
        };
        let prof = ControllerProfile {
            swap_sticks: true,
            ..Default::default()
        };
        let out = process_input(&raw, &ImuCalibration::default(), &prof);
        assert_eq!([out.right_x, out.right_y], [255, 128]);
        assert_eq!([out.left_x, out.left_y], [128, 1]);
    }

    #[test]
    fn gyro_deadzone_axes_inversion_and_right_stick_output() {
        let mut prof = ControllerProfile {
            gyro_mode: GyroAimMode::AlwaysOn,
            gyro_output: GyroOutputMode::RightStick,
            gyro_deadzone: 2.0,
            gyro_sensitivity: 1.0,
            gyro_use_yaw: true,
            gyro_use_roll: false,
            ..Default::default()
        };

        // Below 2.0 deg/s deadzone -> zero delta
        assert_eq!(
            compute_gyro_delta([1024, 0, 0], 0, 0, DualSenseButtons::empty(), false, &prof),
            Some((0.0, 0.0))
        );

        // Above deadzone on yaw + inverted X
        prof.gyro_invert_x = true;
        let (gx, gy) = compute_gyro_delta(
            [0, -10 * 1024, 0],
            0,
            0,
            DualSenseButtons::empty(),
            false,
            &prof,
        )
        .unwrap();
        assert!(gx < -7.0 && gy.abs() < 0.01);

        // RightStick mode adds gyro onto right stick
        let raw = RawInputState {
            left_x: 128,
            left_y: 128,
            right_x: 128,
            right_y: 128,
            gyro: [0, -20 * 1024, 0],
            ..Default::default()
        };
        prof.gyro_invert_x = false;
        let out = process_input(&raw, &ImuCalibration::default(), &prof);
        assert!(out.right_x > 140);
    }

    #[test]
    fn custom_button_map_and_hardened_turbo_work() {
        let mut map = ButtonMap::default();
        map.swap(RemappableButton::Cross, RemappableButton::Circle);
        map.set(RemappableButton::L1, RemappableButton::DpadUp);
        map.set(RemappableButton::L2, RemappableButton::Cross);
        map.set(RemappableButton::Square, RemappableButton::L2);
        map.set(RemappableButton::MicMute, RemappableButton::None);

        let mut prof = ControllerProfile {
            button_map: map,
            turbo: TurboConfig {
                enabled: true,
                target: RemappableButton::L2,
                rate_hz: 10, // 50ms on, 50ms off
            },
            ..Default::default()
        };

        let raw = RawInputState {
            buttons: DualSenseButtons::CROSS
                | DualSenseButtons::L1
                | DualSenseButtons::SQUARE
                | DualSenseButtons::MIC_MUTE,
            l2_analog: 200,
            r2_analog: 180,
            dpad: DPad::Center,
            ..Default::default()
        };
        let proc_in = process_input(&raw, &ImuCalibration::default(), &prof);
        // Cross -> Circle, L2 -> Cross, L1 -> DpadUp, Square -> L2 (255), MicMute -> None, R2 preserved
        assert!(proc_in.buttons.contains(DualSenseButtons::CIRCLE));
        assert!(proc_in.buttons.contains(DualSenseButtons::CROSS));
        assert!(proc_in.buttons.contains(DualSenseButtons::L2_DIGITAL));
        assert_eq!(proc_in.l2_analog, 255);
        assert_eq!(proc_in.r2_analog, 180);
        assert!(!proc_in.buttons.contains(DualSenseButtons::SQUARE));
        assert!(!proc_in.buttons.contains(DualSenseButtons::MIC_MUTE));
        assert_eq!(proc_in.dpad, DPad::North);

        // TurboState fires immediately on first press, gates OFF at +50ms (including L2 analog=0), and resets on release
        let mut tstate = TurboState::default();
        let t0 = Instant::now();
        let mut p_on = proc_in;
        assert!(apply_turbo_state(&mut p_on, &prof.turbo, &mut tstate, t0));
        assert_eq!(p_on.l2_analog, 255);
        assert!(p_on.buttons.contains(DualSenseButtons::L2_DIGITAL));

        let mut p_off = proc_in;
        assert!(apply_turbo_state(
            &mut p_off,
            &prof.turbo,
            &mut tstate,
            t0 + std::time::Duration::from_millis(55)
        ));
        assert_eq!(p_off.l2_analog, 0);
        assert!(!p_off.buttons.contains(DualSenseButtons::L2_DIGITAL));

        // Release resets state
        prof.turbo.target = RemappableButton::R1;
        let mut p_rel = proc_in;
        assert!(!apply_turbo_state(&mut p_rel, &prof.turbo, &mut tstate, t0));
        assert!(!tstate.is_active());
    }

    #[test]
    fn hair_trigger_saturates_early() {
        let tuning = TriggerTuning {
            deadzone: 5,
            hair_trigger: 100,
        };
        assert_eq!(process_trigger(5, &tuning), 0);
        assert_eq!(process_trigger(100, &tuning), 255);
        assert_eq!(process_trigger(200, &tuning), 255);
    }
}
