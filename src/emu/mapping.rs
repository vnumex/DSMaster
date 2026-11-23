use super::vigem::ioctl::XusbReport;
use crate::engine::processing::ProcessedInput;
use crate::protocol::{DPad, DualSenseButtons};

pub const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
pub const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
pub const XINPUT_GAMEPAD_DPAD_LEFT: u16 = 0x0004;
pub const XINPUT_GAMEPAD_DPAD_RIGHT: u16 = 0x0008;
pub const XINPUT_GAMEPAD_START: u16 = 0x0010;
pub const XINPUT_GAMEPAD_BACK: u16 = 0x0020;
pub const XINPUT_GAMEPAD_LEFT_THUMB: u16 = 0x0040;
pub const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
pub const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
pub const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
pub const XINPUT_GAMEPAD_GUIDE: u16 = 0x0400;
pub const XINPUT_GAMEPAD_A: u16 = 0x1000;
pub const XINPUT_GAMEPAD_B: u16 = 0x2000;
pub const XINPUT_GAMEPAD_X: u16 = 0x4000;
pub const XINPUT_GAMEPAD_Y: u16 = 0x8000;

#[inline]
pub const fn u8_to_xusb_x(v: u8) -> i16 {
    if v >= 128 {
        (((v as i32 - 128) * 32767) / 127) as i16
    } else {
        (((v as i32 - 128) * 32768) / 128) as i16
    }
}

#[inline]
pub const fn u8_to_xusb_y(v: u8) -> i16 {
    if v >= 128 {
        -(((v as i32 - 128) * 32768) / 127) as i16
    } else {
        (((128 - v as i32) * 32767) / 128) as i16
    }
}

/// Maps a processed DualSense input into an Xbox 360 `XusbReport` with zero allocations.
#[inline]
pub fn map_to_xusb(input: &ProcessedInput) -> XusbReport {
    let mut buttons: u16 = match input.dpad {
        DPad::North => XINPUT_GAMEPAD_DPAD_UP,
        DPad::NorthEast => XINPUT_GAMEPAD_DPAD_UP | XINPUT_GAMEPAD_DPAD_RIGHT,
        DPad::East => XINPUT_GAMEPAD_DPAD_RIGHT,
        DPad::SouthEast => XINPUT_GAMEPAD_DPAD_DOWN | XINPUT_GAMEPAD_DPAD_RIGHT,
        DPad::South => XINPUT_GAMEPAD_DPAD_DOWN,
        DPad::SouthWest => XINPUT_GAMEPAD_DPAD_DOWN | XINPUT_GAMEPAD_DPAD_LEFT,
        DPad::West => XINPUT_GAMEPAD_DPAD_LEFT,
        DPad::NorthWest => XINPUT_GAMEPAD_DPAD_UP | XINPUT_GAMEPAD_DPAD_LEFT,
        DPad::Center => 0,
    };

    let b = input.buttons;
    if b.contains(DualSenseButtons::CROSS) {
        buttons |= XINPUT_GAMEPAD_A;
    }
    if b.contains(DualSenseButtons::CIRCLE) {
        buttons |= XINPUT_GAMEPAD_B;
    }
    if b.contains(DualSenseButtons::SQUARE) {
        buttons |= XINPUT_GAMEPAD_X;
    }
    if b.contains(DualSenseButtons::TRIANGLE) {
        buttons |= XINPUT_GAMEPAD_Y;
    }
    if b.contains(DualSenseButtons::L1) {
        buttons |= XINPUT_GAMEPAD_LEFT_SHOULDER;
    }
    if b.contains(DualSenseButtons::R1) {
        buttons |= XINPUT_GAMEPAD_RIGHT_SHOULDER;
    }
    if b.contains(DualSenseButtons::L3) {
        buttons |= XINPUT_GAMEPAD_LEFT_THUMB;
    }
    if b.contains(DualSenseButtons::R3) {
        buttons |= XINPUT_GAMEPAD_RIGHT_THUMB;
    }
    if b.contains(DualSenseButtons::OPTIONS) {
        buttons |= XINPUT_GAMEPAD_START;
    }
    if b.contains(DualSenseButtons::CREATE) {
        buttons |= XINPUT_GAMEPAD_BACK;
    }
    if b.contains(DualSenseButtons::PS_HOME) {
        buttons |= XINPUT_GAMEPAD_GUIDE;
    }

    XusbReport {
        buttons,
        left_trigger: input.l2_analog,
        right_trigger: input.r2_analog,
        thumb_lx: u8_to_xusb_x(input.left_x),
        thumb_ly: u8_to_xusb_y(input.left_y),
        thumb_rx: u8_to_xusb_x(input.right_x),
        thumb_ry: u8_to_xusb_y(input.right_y),
    }
}

/// Maps a processed DualSense input into a 63-byte DualShock 4 v1 extended HID report.
#[inline]
pub fn map_to_ds4_ex(input: &ProcessedInput, touch_counter: u8) -> [u8; 63] {
    let mut out = [0u8; 63];
    out[0] = input.left_x;
    out[1] = input.left_y;
    out[2] = input.right_x;
    out[3] = input.right_y;

    let mut w_buttons: u16 = (input.dpad as u16) & 0x0F;
    let b = input.buttons;
    if b.contains(DualSenseButtons::SQUARE) {
        w_buttons |= 1 << 4;
    }
    if b.contains(DualSenseButtons::CROSS) {
        w_buttons |= 1 << 5;
    }
    if b.contains(DualSenseButtons::CIRCLE) {
        w_buttons |= 1 << 6;
    }
    if b.contains(DualSenseButtons::TRIANGLE) {
        w_buttons |= 1 << 7;
    }
    if b.contains(DualSenseButtons::L1) {
        w_buttons |= 1 << 8;
    }
    if b.contains(DualSenseButtons::R1) {
        w_buttons |= 1 << 9;
    }
    if b.contains(DualSenseButtons::L2_DIGITAL) {
        w_buttons |= 1 << 10;
    }
    if b.contains(DualSenseButtons::R2_DIGITAL) {
        w_buttons |= 1 << 11;
    }
    if b.contains(DualSenseButtons::CREATE) {
        w_buttons |= 1 << 12;
    }
    if b.contains(DualSenseButtons::OPTIONS) {
        w_buttons |= 1 << 13;
    }
    if b.contains(DualSenseButtons::L3) {
        w_buttons |= 1 << 14;
    }
    if b.contains(DualSenseButtons::R3) {
        w_buttons |= 1 << 15;
    }
    out[4..6].copy_from_slice(&w_buttons.to_le_bytes());

    let mut b_special: u8 = 0;
    if b.contains(DualSenseButtons::PS_HOME) {
        b_special |= 1 << 0;
    }
    if b.contains(DualSenseButtons::TOUCHPAD) {
        b_special |= 1 << 1;
    }
    out[6] = b_special;
    out[7] = input.l2_analog;
    out[8] = input.r2_analog;

    let ts = (input.sensor_timestamp & 0xFFFF) as u16;
    out[9..11].copy_from_slice(&ts.to_le_bytes());
    out[11] = (input.battery_percent / 10).min(10);

    out[12..14].copy_from_slice(&input.gyro[0].to_le_bytes());
    out[14..16].copy_from_slice(&input.gyro[1].to_le_bytes());
    out[16..18].copy_from_slice(&input.gyro[2].to_le_bytes());
    out[18..20].copy_from_slice(&input.accel[0].to_le_bytes());
    out[20..22].copy_from_slice(&input.accel[1].to_le_bytes());
    out[22..24].copy_from_slice(&input.accel[2].to_le_bytes());

    // Mark USB cable connected + battery level (0..=10)
    out[29] = 0x10 | (input.battery_percent / 10).min(10);

    // Touchpad packet
    out[32] = 1;
    out[33] = touch_counter;
    for (idx, tp) in input.touch.iter().enumerate() {
        let base = 34 + idx * 4;
        if tp.active {
            let x = tp.x.min(1919);
            // Scale DualSense Y (0..1080) to DS4 Y (0..943).
            let y = ((tp.y as u32 * 943) / 1080).min(942) as u16;
            out[base] = tp.id & 0x7F;
            out[base + 1] = (x & 0xFF) as u8;
            out[base + 2] = (((x >> 8) & 0x0F) as u8) | (((y & 0x0F) as u8) << 4);
            out[base + 3] = (y >> 4) as u8;
        } else {
            out[base] = 0x80;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::BatteryStatus;
    use crate::protocol::input::TouchPoint;

    fn sample_processed() -> ProcessedInput {
        ProcessedInput {
            raw_left_x: 255,
            raw_left_y: 0,
            raw_right_x: 128,
            raw_right_y: 128,
            left_x: 255,
            left_y: 0,
            right_x: 128,
            right_y: 128,
            l2_analog: 180,
            r2_analog: 255,
            seq_number: 1,
            dpad: DPad::NorthWest,
            buttons: DualSenseButtons::CROSS
                | DualSenseButtons::L1
                | DualSenseButtons::OPTIONS
                | DualSenseButtons::PS_HOME
                | DualSenseButtons::TOUCHPAD,
            gyro: [10, -20, 30],
            accel: [100, 200, 8192],
            calibrated_gyro: [0, 0, 0],
            sensor_timestamp: 1234,
            touch: [
                TouchPoint {
                    active: true,
                    id: 3,
                    x: 960,
                    y: 540,
                },
                TouchPoint {
                    active: false,
                    id: 0,
                    x: 0,
                    y: 0,
                },
            ],
            battery_percent: 80,
            battery_status: BatteryStatus::Discharging,
            headphone_connected: false,
            mic_connected: false,
            mic_muted_hw: false,
        }
    }

    #[test]
    fn axis_scaling_bounds() {
        assert_eq!(u8_to_xusb_x(0), -32768);
        assert_eq!(u8_to_xusb_x(128), 0);
        assert_eq!(u8_to_xusb_x(255), 32767);

        assert_eq!(u8_to_xusb_y(0), 32767);
        assert_eq!(u8_to_xusb_y(128), 0);
        assert_eq!(u8_to_xusb_y(255), -32768);
    }

    #[test]
    fn xusb_mapping_buttons_and_dpad() {
        let p = sample_processed();
        let xusb = map_to_xusb(&p);
        assert_eq!(xusb.thumb_lx, 32767);
        assert_eq!(xusb.thumb_ly, 32767);
        assert_eq!(xusb.left_trigger, 180);
        assert_eq!(xusb.right_trigger, 255);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_A, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_LEFT_SHOULDER, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_START, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_GUIDE, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_DPAD_UP, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_DPAD_LEFT, 0);
    }

    #[test]
    fn ds4_mapping_buttons_and_touch() {
        let p = sample_processed();
        let ds4 = map_to_ds4_ex(&p, 7);
        assert_eq!(ds4[0], 255);
        assert_eq!(ds4[1], 0);
        let w = u16::from_le_bytes([ds4[4], ds4[5]]);
        assert_eq!(w & 0x0F, DPad::NorthWest as u16);
        assert_ne!(w & (1 << 5), 0); // Cross
        assert_ne!(w & (1 << 8), 0); // L1
        assert_ne!(w & (1 << 13), 0); // Options
        assert_eq!(ds4[6], 0x03); // PS + Touchpad click
        assert_eq!(ds4[7], 180);
        assert_eq!(ds4[8], 255);
        assert_eq!(ds4[32], 1);
        assert_eq!(ds4[33], 7);
        assert_eq!(ds4[34], 3); // touch 0 active, id=3
        assert_eq!(ds4[38], 0x80); // touch 1 inactive
    }
}
