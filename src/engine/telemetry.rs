use crate::emu::EmulationMode;
use crate::protocol::input::TouchPoint;
use crate::protocol::{BatteryStatus, DPad, DualSenseButtons, Transport};

#[derive(Debug, Clone, Copy)]
pub struct TelemetrySnapshot {
    pub active: bool,
    pub is_edge: bool,
    pub transport: Transport,
    pub emulation: EmulationMode,
    pub raw_left_stick: [u8; 2],
    pub raw_right_stick: [u8; 2],
    pub left_stick: [u8; 2],
    pub right_stick: [u8; 2],
    pub l2_analog: u8,
    pub r2_analog: u8,
    pub dpad: DPad,
    pub buttons: DualSenseButtons,
    pub battery_percent: u8,
    pub battery_status: BatteryStatus,
    pub polling_hz: f32,
    pub proc_us_ema: f32,
    pub proc_us_max: u32,
    pub packet_count: u64,
    pub rumble_large: u8,
    pub rumble_small: u8,
    pub touch: [TouchPoint; 2],
    pub active_lightbar_rgb: Option<[u8; 3]>,
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            active: false,
            is_edge: false,
            transport: Transport::Usb,
            emulation: EmulationMode::None,
            raw_left_stick: [128, 128],
            raw_right_stick: [128, 128],
            left_stick: [128, 128],
            right_stick: [128, 128],
            l2_analog: 0,
            r2_analog: 0,
            dpad: DPad::Center,
            buttons: DualSenseButtons::empty(),
            battery_percent: 0,
            battery_status: BatteryStatus::Unknown,
            polling_hz: 0.0,
            proc_us_ema: 0.0,
            proc_us_max: 0,
            packet_count: 0,
            rumble_large: 0,
            rumble_small: 0,
            touch: [TouchPoint::default(); 2],
            active_lightbar_rgb: None,
        }
    }
}
