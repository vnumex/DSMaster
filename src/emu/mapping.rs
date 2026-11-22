use crate::protocol::{DPad, DualSenseButtons, RawInputState};

use super::vigem::ioctl::XusbReport;

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

/// Maps a DualSense `RawInputState` into an Xbox 360 `XusbReport` with zero allocations.
#[inline]
pub fn map_dualsense_to_xusb(raw: &RawInputState) -> XusbReport {
    let mut buttons: u16 = match raw.dpad {
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

    let b = raw.buttons;
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
        left_trigger: raw.l2_analog,
        right_trigger: raw.r2_analog,
        thumb_lx: u8_to_xusb_x(raw.left_x),
        thumb_ly: u8_to_xusb_y(raw.left_y),
        thumb_rx: u8_to_xusb_x(raw.right_x),
        thumb_ry: u8_to_xusb_y(raw.right_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn button_and_dpad_mapping() {
        let raw = RawInputState {
            left_x: 255,
            left_y: 0,
            right_x: 128,
            right_y: 128,
            l2_analog: 180,
            r2_analog: 255,
            dpad: DPad::NorthWest,
            buttons: DualSenseButtons::CROSS | DualSenseButtons::L1 | DualSenseButtons::OPTIONS,
            ..Default::default()
        };
        let xusb = map_dualsense_to_xusb(&raw);
        assert_eq!(xusb.thumb_lx, 32767);
        assert_eq!(xusb.thumb_ly, 32767);
        assert_eq!(xusb.left_trigger, 180);
        assert_eq!(xusb.right_trigger, 255);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_A, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_LEFT_SHOULDER, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_START, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_DPAD_UP, 0);
        assert_ne!(xusb.buttons & XINPUT_GAMEPAD_DPAD_LEFT, 0);
    }
}
