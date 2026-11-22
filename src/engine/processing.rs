use super::profile::{ButtonPreset, ControllerProfile, StickTuning, TriggerTuning};
use crate::protocol::calibration::ImuCalibration;
use crate::protocol::input::{DPad, TouchPoint};
use crate::protocol::{BatteryStatus, DualSenseButtons, RawInputState};

#[derive(Debug, Clone, Copy)]
pub struct ProcessedInput {
    pub left_x: u8,
    pub left_y: u8,
    pub right_x: u8,
    pub right_y: u8,
    pub l2_analog: u8,
    pub r2_analog: u8,
    pub dpad: DPad,
    pub buttons: DualSenseButtons,
    pub gyro: [i16; 3],
    pub accel: [i16; 3],
    pub calibrated_gyro: [i32; 3],
    pub sensor_timestamp: u32,
    pub touch: [TouchPoint; 2],
    pub battery_percent: u8,
    pub battery_status: BatteryStatus,
}

#[inline]
fn process_stick(raw_x: u8, raw_y: u8, tuning: &StickTuning) -> [u8; 2] {
    let nx = (raw_x as f32 - 128.0) / 127.5;
    let ny = (raw_y as f32 - 128.0) / 127.5;
    let mag = (nx * nx + ny * ny).sqrt();
    let dz = tuning.deadzone.clamp(0.0, 0.49);
    if mag <= dz || mag < 1e-6 {
        return [128, 128];
    }

    let norm_mag = ((mag - dz) / (1.0 - dz)).min(1.0);
    let curved = tuning.curve.apply(norm_mag);
    let anti = tuning.anti_deadzone.clamp(0.0, 0.45);
    let out_mag = if curved > 0.0 {
        (anti + curved * (1.0 - anti)).min(1.0)
    } else {
        0.0
    };

    let scale = out_mag / mag;
    let ox = (nx * scale * 127.5 + 128.0).round().clamp(0.0, 255.0) as u8;
    let oy = (ny * scale * 127.5 + 128.0).round().clamp(0.0, 255.0) as u8;
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
fn swap_bits(
    buttons: &mut DualSenseButtons,
    a: DualSenseButtons,
    b: DualSenseButtons,
    raw: DualSenseButtons,
) {
    buttons.set(a, raw.contains(b));
    buttons.set(b, raw.contains(a));
}

#[inline]
fn remap_buttons(raw: DualSenseButtons, preset: ButtonPreset) -> DualSenseButtons {
    let mut out = raw;
    match preset {
        ButtonPreset::Standard => {}
        ButtonPreset::SwapFace => {
            swap_bits(
                &mut out,
                DualSenseButtons::CROSS,
                DualSenseButtons::CIRCLE,
                raw,
            );
            swap_bits(
                &mut out,
                DualSenseButtons::SQUARE,
                DualSenseButtons::TRIANGLE,
                raw,
            );
        }
        ButtonPreset::Tactical => {
            swap_bits(
                &mut out,
                DualSenseButtons::CIRCLE,
                DualSenseButtons::R3,
                raw,
            );
        }
        ButtonPreset::BumperJumper => {
            swap_bits(&mut out, DualSenseButtons::CROSS, DualSenseButtons::L1, raw);
        }
    }
    out
}

#[inline]
pub fn process_input(
    raw: &RawInputState,
    cal: &ImuCalibration,
    profile: &ControllerProfile,
) -> ProcessedInput {
    let left = process_stick(raw.left_x, raw.left_y, &profile.left_stick);
    let right = process_stick(raw.right_x, raw.right_y, &profile.right_stick);
    let l2_analog = process_trigger(raw.l2_analog, &profile.triggers);
    let r2_analog = process_trigger(raw.r2_analog, &profile.triggers);
    let mut buttons = remap_buttons(raw.buttons, profile.button_preset);
    buttons.set(DualSenseButtons::L2_DIGITAL, l2_analog >= 32);
    buttons.set(DualSenseButtons::R2_DIGITAL, r2_analog >= 32);

    ProcessedInput {
        left_x: left[0],
        left_y: left[1],
        right_x: right[0],
        right_y: right[1],
        l2_analog,
        r2_analog,
        dpad: raw.dpad,
        buttons,
        gyro: raw.gyro,
        accel: raw.accel,
        calibrated_gyro: [
            cal.gyro[0].apply(raw.gyro[0]),
            cal.gyro[1].apply(raw.gyro[1]),
            cal.gyro[2].apply(raw.gyro[2]),
        ],
        sensor_timestamp: raw.sensor_timestamp,
        touch: raw.touch,
        battery_percent: raw.battery_percent,
        battery_status: raw.battery_status,
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
        };
        let [ox, _] = process_stick(142, 128, &tuning);
        assert!(ox >= 160);
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

    #[test]
    fn button_presets_swap_expected_buttons() {
        let raw = DualSenseButtons::CROSS | DualSenseButtons::R3;
        let swap = remap_buttons(raw, ButtonPreset::SwapFace);
        assert!(swap.contains(DualSenseButtons::CIRCLE));
        assert!(!swap.contains(DualSenseButtons::CROSS));

        let tac = remap_buttons(raw, ButtonPreset::Tactical);
        assert!(tac.contains(DualSenseButtons::CIRCLE));
        assert!(!tac.contains(DualSenseButtons::R3));
        assert!(tac.contains(DualSenseButtons::CROSS));

        let bj = remap_buttons(raw, ButtonPreset::BumperJumper);
        assert!(bj.contains(DualSenseButtons::L1));
        assert!(!bj.contains(DualSenseButtons::CROSS));
    }
}
