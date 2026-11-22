use super::constants::{
    DS_OUTPUT_REPORT_BT, DS_OUTPUT_REPORT_BT_SIZE, DS_OUTPUT_REPORT_USB,
    DS_OUTPUT_REPORT_USB_MIN_SIZE, DS_OUTPUT_TAG, PS_OUTPUT_CRC32_SEED, ProtocolError, Transport,
    crc32_with_seed,
};
use super::triggers::AdaptiveTriggerEffect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputState {
    pub update_rumble: bool,
    pub motor_left: u8,
    pub motor_right: u8,
    pub update_lightbar: bool,
    pub lightbar_rgb: [u8; 3],
    pub update_player_leds: bool,
    pub player_leds: u8,
    pub update_right_trigger: bool,
    pub right_trigger: AdaptiveTriggerEffect,
    pub update_left_trigger: bool,
    pub left_trigger: AdaptiveTriggerEffect,
    pub disable_led_fade: bool,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            update_rumble: false,
            motor_left: 0,
            motor_right: 0,
            update_lightbar: false,
            lightbar_rgb: [0, 0, 64],
            update_player_leds: false,
            player_leds: 0x04,
            update_right_trigger: false,
            right_trigger: AdaptiveTriggerEffect::Off,
            update_left_trigger: false,
            left_trigger: AdaptiveTriggerEffect::Off,
            disable_led_fade: true,
        }
    }
}

#[inline]
fn write_common_payload(state: &OutputState, c: &mut [u8]) {
    let mut flag0: u8 = 0;
    let mut flag1: u8 = 0;
    let mut flag2: u8 = 0;

    if state.update_rumble {
        flag0 |= 0x01 | 0x02;
        flag2 |= 0x04;
        c[2] = state.motor_right;
        c[3] = state.motor_left;
    }

    if state.update_right_trigger {
        flag0 |= 0x04;
        c[10..21].copy_from_slice(&state.right_trigger.encode());
    }

    if state.update_left_trigger {
        flag0 |= 0x08;
        c[21..32].copy_from_slice(&state.left_trigger.encode());
    }

    if state.update_lightbar {
        flag1 |= 0x04;
        if state.disable_led_fade {
            flag2 |= 0x02;
            c[41] = 0x02;
        }
        c[44] = state.lightbar_rgb[0];
        c[45] = state.lightbar_rgb[1];
        c[46] = state.lightbar_rgb[2];
    }

    if state.update_player_leds {
        flag1 |= 0x10;
        let fade_bit = if state.disable_led_fade { 0x20 } else { 0x00 };
        c[43] = (state.player_leds & 0x1F) | fade_bit;
    }

    c[0] = flag0;
    c[1] = flag1;
    c[38] = flag2;
}

/// Encodes a DualSense output report directly into `out` with zero heap allocations.
///
/// For `Transport::Usb`, `out.len()` must be at least `48` (typically `caps.OutputReportByteLength`, 48 or 63).
/// For `Transport::Bluetooth`, `out.len()` must be at least `78`.
#[inline]
pub fn encode_output_report(
    state: &OutputState,
    transport: Transport,
    bt_seq: u8,
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    match transport {
        Transport::Usb => {
            if out.len() < DS_OUTPUT_REPORT_USB_MIN_SIZE {
                return Err(ProtocolError::BufferTooSmall {
                    expected: DS_OUTPUT_REPORT_USB_MIN_SIZE,
                    actual: out.len(),
                });
            }
            out.fill(0);
            out[0] = DS_OUTPUT_REPORT_USB;
            write_common_payload(state, &mut out[1..48]);
            Ok(out.len())
        }
        Transport::Bluetooth => {
            if out.len() < DS_OUTPUT_REPORT_BT_SIZE {
                return Err(ProtocolError::BufferTooSmall {
                    expected: DS_OUTPUT_REPORT_BT_SIZE,
                    actual: out.len(),
                });
            }
            out.fill(0);
            let buf = &mut out[..DS_OUTPUT_REPORT_BT_SIZE];
            buf[0] = DS_OUTPUT_REPORT_BT;
            buf[1] = (bt_seq & 0x0F) << 4;
            buf[2] = DS_OUTPUT_TAG;
            write_common_payload(state, &mut buf[3..50]);
            let crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &buf[0..74]);
            buf[74..78].copy_from_slice(&crc.to_le_bytes());
            Ok(out.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_usb_and_bt_output_reports() {
        let state = OutputState {
            update_rumble: true,
            motor_left: 200,
            motor_right: 120,
            update_lightbar: true,
            lightbar_rgb: [10, 20, 30],
            update_player_leds: true,
            player_leds: 0x04,
            update_right_trigger: true,
            right_trigger: AdaptiveTriggerEffect::Feedback {
                start_position: 1,
                strength: 5,
            },
            ..Default::default()
        };

        let mut usb = [0u8; 48];
        let usb_len = encode_output_report(&state, Transport::Usb, 0, &mut usb).unwrap();
        assert_eq!(usb_len, 48);
        assert_eq!(usb[0], 0x02);
        assert_eq!(usb[1] & 0x07, 0x07);
        assert_eq!(usb[3], 120);
        assert_eq!(usb[4], 200);
        assert_eq!(usb[11], 0x21);
        assert_eq!(usb[45..48], [10, 20, 30]);

        let mut bt = [0u8; 78];
        let bt_len = encode_output_report(&state, Transport::Bluetooth, 3, &mut bt).unwrap();
        assert_eq!(bt_len, 78);
        assert_eq!(bt[0], 0x31);
        assert_eq!(bt[1], 0x30);
        assert_eq!(bt[2], 0x10);
        assert_eq!(bt[5], 120);
        assert_eq!(bt[6], 200);
        assert_eq!(bt[13], 0x21);
        assert_eq!(bt[47..50], [10, 20, 30]);
        let expected_crc = crc32_with_seed(PS_OUTPUT_CRC32_SEED, &bt[0..74]);
        assert_eq!(bt[74..78], expected_crc.to_le_bytes());

        // Windows Bluetooth HID requires 547-byte padded buffers with trailing bytes zeroed.
        let mut bt_547 = [0xFFu8; 547];
        let bt_547_len =
            encode_output_report(&state, Transport::Bluetooth, 3, &mut bt_547).unwrap();
        assert_eq!(bt_547_len, 547);
        assert_eq!(&bt_547[..78], &bt[..]);
        assert!(bt_547[78..].iter().all(|&b| b == 0));
    }

    #[test]
    fn rumble_only_leaves_lightbar_and_triggers_untouched() {
        let state = OutputState {
            update_rumble: true,
            motor_left: 150,
            motor_right: 80,
            ..Default::default()
        };

        let mut usb = [0u8; 48];
        encode_output_report(&state, Transport::Usb, 0, &mut usb).unwrap();
        assert_eq!(usb[1] & 0x0C, 0);
        assert_eq!(usb[2] & 0x14, 0);
        assert_eq!(usb[39] & 0x02, 0);
        assert_eq!(usb[3], 80);
        assert_eq!(usb[4], 150);

        let mut bt = [0u8; 78];
        encode_output_report(&state, Transport::Bluetooth, 1, &mut bt).unwrap();
        assert_eq!(bt[3] & 0x0C, 0);
        assert_eq!(bt[4] & 0x14, 0);
        assert_eq!(bt[41] & 0x02, 0);
        assert_eq!(bt[5], 80);
        assert_eq!(bt[6], 150);
    }
}
