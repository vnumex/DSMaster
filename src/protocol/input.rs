use bitflags::bitflags;

use super::constants::{
    DS_INPUT_REPORT_BT, DS_INPUT_REPORT_BT_SIZE, DS_INPUT_REPORT_USB, DS_INPUT_REPORT_USB_SIZE,
    PS_INPUT_CRC32_SEED, ProtocolError, Transport, crc32_with_seed,
};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct DualSenseButtons: u32 {
        const SQUARE       = 1 << 0;
        const CROSS        = 1 << 1;
        const CIRCLE       = 1 << 2;
        const TRIANGLE     = 1 << 3;
        const L1           = 1 << 4;
        const R1           = 1 << 5;
        const L2_DIGITAL   = 1 << 6;
        const R2_DIGITAL   = 1 << 7;
        const CREATE       = 1 << 8;
        const OPTIONS      = 1 << 9;
        const L3           = 1 << 10;
        const R3           = 1 << 11;
        const PS_HOME      = 1 << 12;
        const TOUCHPAD     = 1 << 13;
        const MIC_MUTE     = 1 << 14;
        const EDGE_FN1     = 1 << 15;
        const EDGE_FN2     = 1 << 16;
        const EDGE_PADDLE_L = 1 << 17;
        const EDGE_PADDLE_R = 1 << 18;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DPad {
    North = 0,
    NorthEast = 1,
    East = 2,
    SouthEast = 3,
    South = 4,
    SouthWest = 5,
    West = 6,
    NorthWest = 7,
    #[default]
    Center = 8,
}

impl DPad {
    #[inline]
    pub const fn from_hat(value: u8) -> Self {
        match value & 0x0F {
            0 => Self::North,
            1 => Self::NorthEast,
            2 => Self::East,
            3 => Self::SouthEast,
            4 => Self::South,
            5 => Self::SouthWest,
            6 => Self::West,
            7 => Self::NorthWest,
            _ => Self::Center,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TouchPoint {
    pub active: bool,
    pub id: u8,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatteryStatus {
    Discharging,
    Charging,
    Full,
    TempOrVoltageError,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RawInputState {
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
    pub sensor_timestamp: u32,
    pub touch: [TouchPoint; 2],
    pub battery_percent: u8,
    pub battery_status: BatteryStatus,
    pub headphone_connected: bool,
    pub mic_connected: bool,
    pub mic_muted_hw: bool,
}

#[inline]
fn parse_touch_point(tp: &[u8]) -> TouchPoint {
    let contact = tp[0];
    let active = (contact & 0x80) == 0;
    let id = contact & 0x7F;
    let x = (tp[1] as u16) | (((tp[2] & 0x0F) as u16) << 8);
    let y = ((tp[2] >> 4) as u16) | ((tp[3] as u16) << 4);
    TouchPoint { active, id, x, y }
}

/// Parses a raw DualSense USB (`0x01`) or Bluetooth (`0x31`) HID input report with zero allocations.
#[inline]
pub fn parse_input_report(
    data: &[u8],
    transport: Transport,
    is_edge: bool,
) -> Result<RawInputState, ProtocolError> {
    if data.is_empty() {
        return Err(ProtocolError::ReportTooShort {
            expected: 1,
            actual: 0,
        });
    }

    let report_id = data[0];
    let payload: &[u8] = match transport {
        Transport::Usb => {
            if report_id != DS_INPUT_REPORT_USB {
                return Err(ProtocolError::UnexpectedReportId(report_id));
            }
            if data.len() < DS_INPUT_REPORT_USB_SIZE {
                return Err(ProtocolError::ReportTooShort {
                    expected: DS_INPUT_REPORT_USB_SIZE,
                    actual: data.len(),
                });
            }
            &data[1..DS_INPUT_REPORT_USB_SIZE]
        }
        Transport::Bluetooth => {
            if report_id == DS_INPUT_REPORT_USB {
                return Err(ProtocolError::MinimalBluetoothReport);
            }
            if report_id != DS_INPUT_REPORT_BT {
                return Err(ProtocolError::UnexpectedReportId(report_id));
            }
            if data.len() < DS_INPUT_REPORT_BT_SIZE {
                return Err(ProtocolError::ReportTooShort {
                    expected: DS_INPUT_REPORT_BT_SIZE,
                    actual: data.len(),
                });
            }
            let expected = crc32_with_seed(PS_INPUT_CRC32_SEED, &data[0..74]);
            let actual = u32::from_le_bytes([data[74], data[75], data[76], data[77]]);
            if expected != actual {
                return Err(ProtocolError::InvalidCrc { expected, actual });
            }
            &data[2..65]
        }
    };

    let b0 = payload[7];
    let b1 = payload[8];
    let b2 = payload[9];

    let mut buttons = DualSenseButtons::empty();
    if b0 & (1 << 4) != 0 {
        buttons |= DualSenseButtons::SQUARE;
    }
    if b0 & (1 << 5) != 0 {
        buttons |= DualSenseButtons::CROSS;
    }
    if b0 & (1 << 6) != 0 {
        buttons |= DualSenseButtons::CIRCLE;
    }
    if b0 & (1 << 7) != 0 {
        buttons |= DualSenseButtons::TRIANGLE;
    }
    if b1 & (1 << 0) != 0 {
        buttons |= DualSenseButtons::L1;
    }
    if b1 & (1 << 1) != 0 {
        buttons |= DualSenseButtons::R1;
    }
    if b1 & (1 << 2) != 0 {
        buttons |= DualSenseButtons::L2_DIGITAL;
    }
    if b1 & (1 << 3) != 0 {
        buttons |= DualSenseButtons::R2_DIGITAL;
    }
    if b1 & (1 << 4) != 0 {
        buttons |= DualSenseButtons::CREATE;
    }
    if b1 & (1 << 5) != 0 {
        buttons |= DualSenseButtons::OPTIONS;
    }
    if b1 & (1 << 6) != 0 {
        buttons |= DualSenseButtons::L3;
    }
    if b1 & (1 << 7) != 0 {
        buttons |= DualSenseButtons::R3;
    }
    if b2 & (1 << 0) != 0 {
        buttons |= DualSenseButtons::PS_HOME;
    }
    if b2 & (1 << 1) != 0 {
        buttons |= DualSenseButtons::TOUCHPAD;
    }
    if b2 & (1 << 2) != 0 {
        buttons |= DualSenseButtons::MIC_MUTE;
    }
    if is_edge {
        if b2 & (1 << 4) != 0 {
            buttons |= DualSenseButtons::EDGE_FN1;
        }
        if b2 & (1 << 5) != 0 {
            buttons |= DualSenseButtons::EDGE_FN2;
        }
        if b2 & (1 << 6) != 0 {
            buttons |= DualSenseButtons::EDGE_PADDLE_L;
        }
        if b2 & (1 << 7) != 0 {
            buttons |= DualSenseButtons::EDGE_PADDLE_R;
        }
    }

    let status0 = payload[52];
    let status1 = payload[53];
    let battery_data = status0 & 0x0F;
    let charging_raw = (status0 >> 4) & 0x0F;
    let (battery_percent, battery_status) = match charging_raw {
        0x0 => (
            (battery_data.saturating_mul(10).saturating_add(5)).min(100),
            BatteryStatus::Discharging,
        ),
        0x1 => (
            (battery_data.saturating_mul(10).saturating_add(5)).min(100),
            BatteryStatus::Charging,
        ),
        0x2 => (100, BatteryStatus::Full),
        0xA | 0xB => (0, BatteryStatus::TempOrVoltageError),
        _ => (0, BatteryStatus::Unknown),
    };

    Ok(RawInputState {
        left_x: payload[0],
        left_y: payload[1],
        right_x: payload[2],
        right_y: payload[3],
        l2_analog: payload[4],
        r2_analog: payload[5],
        seq_number: payload[6],
        dpad: DPad::from_hat(b0),
        buttons,
        gyro: [
            i16::from_le_bytes([payload[15], payload[16]]),
            i16::from_le_bytes([payload[17], payload[18]]),
            i16::from_le_bytes([payload[19], payload[20]]),
        ],
        accel: [
            i16::from_le_bytes([payload[21], payload[22]]),
            i16::from_le_bytes([payload[23], payload[24]]),
            i16::from_le_bytes([payload[25], payload[26]]),
        ],
        sensor_timestamp: u32::from_le_bytes([payload[27], payload[28], payload[29], payload[30]]),
        touch: [
            parse_touch_point(&payload[32..36]),
            parse_touch_point(&payload[36..40]),
        ],
        battery_percent,
        battery_status,
        headphone_connected: (status1 & 0x01) != 0,
        mic_connected: (status1 & 0x02) != 0,
        mic_muted_hw: (status1 & 0x04) != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usb_input_report_basic() {
        let mut pkt = [0u8; DS_INPUT_REPORT_USB_SIZE];
        pkt[0] = DS_INPUT_REPORT_USB;
        pkt[1] = 128; // lx
        pkt[2] = 255; // ly
        pkt[3] = 0; // rx
        pkt[4] = 64; // ry
        pkt[5] = 200; // l2
        pkt[6] = 100; // r2
        pkt[7] = 42; // seq
        pkt[8] = 0x20 | 0x02; // Cross + East DPad
        pkt[9] = 0x01 | 0x20; // L1 + Options
        pkt[10] = 0x01 | 0x80; // PS_HOME + Edge right paddle
        // Touch 0 active at (1919, 1079)
        pkt[33] = 0x01; // active, id=1
        pkt[34] = (1919 & 0xFF) as u8;
        pkt[35] = (((1919 >> 8) & 0x0F) as u8) | (((1079 & 0x0F) as u8) << 4);
        pkt[36] = (1079 >> 4) as u8;
        // Touch 1 inactive
        pkt[37] = 0x80;
        // Status0: charging, level 7 -> 75%
        pkt[53] = (0x1 << 4) | 7;
        pkt[54] = 0x05; // headphone + hw mic mute

        let state = parse_input_report(&pkt, Transport::Usb, true).unwrap();
        assert_eq!(state.left_x, 128);
        assert_eq!(state.left_y, 255);
        assert_eq!(state.right_x, 0);
        assert_eq!(state.right_y, 64);
        assert_eq!(state.l2_analog, 200);
        assert_eq!(state.r2_analog, 100);
        assert_eq!(state.seq_number, 42);
        assert_eq!(state.dpad, DPad::East);
        assert!(state.buttons.contains(DualSenseButtons::CROSS));
        assert!(state.buttons.contains(DualSenseButtons::L1));
        assert!(state.buttons.contains(DualSenseButtons::OPTIONS));
        assert!(state.buttons.contains(DualSenseButtons::PS_HOME));
        assert!(state.buttons.contains(DualSenseButtons::EDGE_PADDLE_R));
        assert!(state.touch[0].active);
        assert_eq!(state.touch[0].id, 1);
        assert_eq!(state.touch[0].x, 1919);
        assert_eq!(state.touch[0].y, 1079);
        assert!(!state.touch[1].active);
        assert_eq!(state.battery_percent, 75);
        assert_eq!(state.battery_status, BatteryStatus::Charging);
        assert!(state.headphone_connected);
        assert!(!state.mic_connected);
        assert!(state.mic_muted_hw);
    }

    #[test]
    fn parse_bt_minimal_detected() {
        let mut pkt = [0u8; 10];
        pkt[0] = DS_INPUT_REPORT_USB;
        let err = parse_input_report(&pkt, Transport::Bluetooth, false).unwrap_err();
        assert_eq!(err, ProtocolError::MinimalBluetoothReport);
    }

    #[test]
    fn parse_bt_full_with_crc() {
        let mut pkt = [0u8; DS_INPUT_REPORT_BT_SIZE];
        pkt[0] = DS_INPUT_REPORT_BT;
        pkt[1] = 0x00;
        pkt[2] = 64; // lx
        pkt[3] = 192; // ly
        pkt[9] = 0x18; // Square + center dpad (8)
        pkt[54] = (0x2 << 4) | 10; // Full battery
        let crc = crc32_with_seed(PS_INPUT_CRC32_SEED, &pkt[0..74]);
        pkt[74..78].copy_from_slice(&crc.to_le_bytes());

        let state = parse_input_report(&pkt, Transport::Bluetooth, false).unwrap();
        assert_eq!(state.left_x, 64);
        assert_eq!(state.left_y, 192);
        assert_eq!(state.dpad, DPad::Center);
        assert!(state.buttons.contains(DualSenseButtons::SQUARE));
        assert_eq!(state.battery_percent, 100);
        assert_eq!(state.battery_status, BatteryStatus::Full);

        // Corrupt 1 byte and verify CRC rejection
        pkt[10] ^= 0xFF;
        assert!(matches!(
            parse_input_report(&pkt, Transport::Bluetooth, false),
            Err(ProtocolError::InvalidCrc { .. })
        ));
    }
}
