use serde::{Deserialize, Serialize};

use super::constants::{
    DS_BT_AUDIO_SUBPKT_HEADSET, DS_BT_AUDIO_SUBPKT_SPEAKER, DS_OUTPUT_REPORT_BT,
    DS_OUTPUT_REPORT_BT_SIZE, DS_OUTPUT_REPORT_USB, DS_OUTPUT_REPORT_USB_MIN_SIZE, DS_OUTPUT_TAG,
    PS_OUTPUT_CRC32_SEED, ProtocolError, Transport, crc32_with_seed,
};
use super::triggers::AdaptiveTriggerEffect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum MicMuteLed {
    #[default]
    Off = 0,
    Solid = 1,
    Pulse = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum LedBrightness {
    #[default]
    High = 0,
    Medium = 1,
    Low = 2,
}

impl LedBrightness {
    pub const ALL: [Self; 3] = [Self::High, Self::Medium, Self::Low];

    pub fn label(self) -> &'static str {
        match self {
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BtAudioRoute {
    #[default]
    Speaker,
    Headphones,
    Both,
}

impl BtAudioRoute {
    pub const ALL: [Self; 3] = [Self::Speaker, Self::Headphones, Self::Both];

    pub fn label(self) -> &'static str {
        match self {
            Self::Speaker => "Speaker",
            Self::Headphones => "Headphones",
            Self::Both => "Both",
        }
    }

    #[inline]
    pub fn audio_control_byte(self) -> u8 {
        match self {
            Self::Speaker => 0x30,
            Self::Headphones => 0x00,
            Self::Both => 0x20,
        }
    }

    #[inline]
    pub fn bt_lane_pid(self) -> u8 {
        match self {
            Self::Speaker => DS_BT_AUDIO_SUBPKT_SPEAKER,
            Self::Headphones | Self::Both => DS_BT_AUDIO_SUBPKT_HEADSET,
        }
    }

    #[inline]
    pub fn needs_mono_fold(self) -> bool {
        matches!(self, Self::Speaker | Self::Both)
    }
}

/// Maps normalized `[0.0, 1.0]` speaker volume to Sony's hardware speaker gain window (`0x3D..=0x64`, or `0x00` when muted).
#[inline]
pub fn scale_speaker_volume(vol: f32) -> u8 {
    let v = vol.clamp(0.0, 1.0);
    if v <= 0.001 {
        0x00
    } else {
        (0x3D as f32 + v * ((0x64 - 0x3D) as f32)).round() as u8
    }
}

/// Maps normalized `[0.0, 1.0]` headphone volume to Sony's `scePad` headset gain window (`0x40..=0x7F`, or `0x00` when muted).
#[inline]
pub fn scale_headphone_volume(vol: f32) -> u8 {
    let v = vol.clamp(0.0, 1.0);
    if v <= 0.001 {
        0x00
    } else {
        (0x40 as f32 + v * ((0x7F - 0x40) as f32)).round() as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputState {
    pub update_rumble: bool,
    pub motor_left: u8,
    pub motor_right: u8,
    pub update_headphone_volume: bool,
    pub headphone_volume: u8,
    pub update_speaker_volume: bool,
    pub speaker_volume: u8,
    pub update_mic_volume: bool,
    pub mic_volume: u8,
    pub update_audio_control: bool,
    pub audio_control: u8,
    pub unmute_audio: bool,
    pub update_mic_mute_led: bool,
    pub mic_mute_led: MicMuteLed,
    pub update_lightbar: bool,
    pub lightbar_rgb: [u8; 3],
    pub update_player_leds: bool,
    pub player_leds: u8,
    pub update_led_brightness: bool,
    pub led_brightness: LedBrightness,
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
            update_headphone_volume: false,
            headphone_volume: 0,
            update_speaker_volume: false,
            speaker_volume: 0,
            update_mic_volume: false,
            mic_volume: 0,
            update_audio_control: false,
            audio_control: 0,
            unmute_audio: false,
            update_mic_mute_led: false,
            mic_mute_led: MicMuteLed::Off,
            update_lightbar: false,
            lightbar_rgb: [0, 0, 64],
            update_player_leds: false,
            player_leds: 0x04,
            update_led_brightness: false,
            led_brightness: LedBrightness::High,
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

    if state.update_headphone_volume {
        flag0 |= 0x10;
        c[4] = state.headphone_volume.min(0x7F);
    }

    if state.update_speaker_volume {
        flag0 |= 0x20;
        c[5] = state.speaker_volume;
    }

    if state.update_mic_volume {
        flag0 |= 0x40;
        c[6] = state.mic_volume.min(0x40);
    }

    if state.update_audio_control {
        flag0 |= 0x80;
        c[7] = state.audio_control;
    }

    if state.unmute_audio {
        flag1 |= 0x02;
        c[9] = 0x00;
    }

    if state.update_right_trigger {
        flag0 |= 0x04;
        c[10..21].copy_from_slice(&state.right_trigger.encode());
    }

    if state.update_left_trigger {
        flag0 |= 0x08;
        c[21..32].copy_from_slice(&state.left_trigger.encode());
    }

    if state.update_mic_mute_led {
        flag1 |= 0x01;
        c[8] = state.mic_mute_led as u8;
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

    if state.update_led_brightness {
        flag2 |= 0x01;
        c[42] = state.led_brightness as u8;
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

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParsedOutputReport {
    /// Rumble motor intensities as `(large_left, small_right)`.
    pub rumble: Option<(u8, u8)>,
    pub mic_mute_led: Option<u8>,
    pub lightbar_rgb: Option<[u8; 3]>,
    pub player_leds: Option<u8>,
    pub led_brightness: Option<u8>,
    pub left_trigger_raw: Option<[u8; 11]>,
    pub right_trigger_raw: Option<[u8; 11]>,
}

/// Parses an incoming DualSense USB HID Output Report (`0x02`, >= 48 bytes) into
/// per-subsystem feedback fields according to `valid_flag0` (`data[1]`) and `valid_flag1` (`data[2]`).
///
/// Exact USB report layout (including Report ID `0x02` at `data[0]`, matching `write_common_payload` on `&mut out[1..48]`):
/// - `data[1]` (`c[0]`): `valid_flag0` (`0x01|0x02` rumble, `0x04` R2 trigger, `0x08` L2 trigger)
/// - `data[2]` (`c[1]`): `valid_flag1` (`0x04` lightbar RGB, `0x10` player LEDs)
/// - `data[3]` (`c[2]`): `motor_right` (small/high-frequency motor)
/// - `data[4]` (`c[3]`): `motor_left` (large/low-frequency motor)
/// - `data[11..22]` (`c[10..21]`): right adaptive trigger (11 bytes)
/// - `data[22..33]` (`c[21..32]`): left adaptive trigger (11 bytes)
/// - `data[44]` (`c[43]`): player LEDs (low 5 bits `0x1F`)
/// - `data[45..48]` (`c[44..47]`): lightbar `[R, G, B]`
#[allow(dead_code)]
#[inline]
pub fn parse_usb_output_report(data: &[u8]) -> Result<ParsedOutputReport, ProtocolError> {
    if data.is_empty() {
        return Err(ProtocolError::ReportTooShort {
            expected: DS_OUTPUT_REPORT_USB_MIN_SIZE,
            actual: 0,
        });
    }
    if data[0] != DS_OUTPUT_REPORT_USB {
        return Err(ProtocolError::UnexpectedReportId(data[0]));
    }
    if data.len() < DS_OUTPUT_REPORT_USB_MIN_SIZE {
        return Err(ProtocolError::ReportTooShort {
            expected: DS_OUTPUT_REPORT_USB_MIN_SIZE,
            actual: data.len(),
        });
    }

    let c = &data[1..DS_OUTPUT_REPORT_USB_MIN_SIZE];
    let flag0 = c[0];
    let flag1 = c[1];
    let flag2 = c[38];

    let rumble = if (flag0 & (0x01 | 0x02)) != 0 {
        Some((c[3], c[2]))
    } else {
        None
    };

    let mic_mute_led = if (flag1 & 0x01) != 0 { Some(c[8]) } else { None };

    let right_trigger_raw = if (flag0 & 0x04) != 0 {
        let mut raw = [0u8; 11];
        raw.copy_from_slice(&c[10..21]);
        Some(raw)
    } else {
        None
    };

    let left_trigger_raw = if (flag0 & 0x08) != 0 {
        let mut raw = [0u8; 11];
        raw.copy_from_slice(&c[21..32]);
        Some(raw)
    } else {
        None
    };

    let player_leds = if (flag1 & 0x10) != 0 {
        Some(c[43] & 0x1F)
    } else {
        None
    };

    let led_brightness = if (flag2 & 0x01) != 0 {
        Some(c[42])
    } else {
        None
    };

    let lightbar_rgb = if (flag1 & 0x04) != 0 {
        Some([c[44], c[45], c[46]])
    } else {
        None
    };

    Ok(ParsedOutputReport {
        rumble,
        mic_mute_led,
        lightbar_rgb,
        player_leds,
        led_brightness,
        left_trigger_raw,
        right_trigger_raw,
    })
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

    #[test]
    fn usb_output_report_round_trip_full() {
        let left_trig = AdaptiveTriggerEffect::Weapon {
            start_position: 2,
            end_position: 6,
            strength: 7,
        };
        let right_trig = AdaptiveTriggerEffect::Vibration {
            position: 1,
            amplitude: 5,
            frequency: 35,
        };
        let state = OutputState {
            update_rumble: true,
            motor_left: 210,
            motor_right: 95,
            update_mic_mute_led: true,
            mic_mute_led: MicMuteLed::Pulse,
            update_lightbar: true,
            lightbar_rgb: [12, 180, 255],
            update_player_leds: true,
            player_leds: 0x15,
            update_led_brightness: true,
            led_brightness: LedBrightness::Low,
            update_left_trigger: true,
            left_trigger: left_trig,
            update_right_trigger: true,
            right_trigger: right_trig,
            disable_led_fade: true,
            ..Default::default()
        };

        let mut usb = [0u8; 64];
        encode_output_report(&state, Transport::Usb, 0, &mut usb).unwrap();

        let parsed = parse_usb_output_report(&usb).unwrap();
        assert_eq!(parsed.rumble, Some((210, 95)));
        assert_eq!(parsed.mic_mute_led, Some(MicMuteLed::Pulse as u8));
        assert_eq!(parsed.lightbar_rgb, Some([12, 180, 255]));
        assert_eq!(parsed.player_leds, Some(0x15));
        assert_eq!(parsed.led_brightness, Some(LedBrightness::Low as u8));
        assert_eq!(parsed.left_trigger_raw, Some(left_trig.encode()));
        assert_eq!(parsed.right_trigger_raw, Some(right_trig.encode()));
    }

    #[test]
    fn usb_output_report_partial_flags_preserved() {
        // Trigger-only update must NOT clobber rumble or lighting.
        let trig_only = OutputState {
            update_right_trigger: true,
            right_trigger: AdaptiveTriggerEffect::Raw([
                0x21, 0xFC, 0x03, 0x24, 0x49, 0x92, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]),
            ..Default::default()
        };
        let mut usb = [0u8; 48];
        encode_output_report(&trig_only, Transport::Usb, 0, &mut usb).unwrap();

        let parsed = parse_usb_output_report(&usb).unwrap();
        assert_eq!(parsed.rumble, None);
        assert_eq!(parsed.lightbar_rgb, None);
        assert_eq!(parsed.player_leds, None);
        assert_eq!(parsed.left_trigger_raw, None);
        assert_eq!(
            parsed.right_trigger_raw,
            Some([0x21, 0xFC, 0x03, 0x24, 0x49, 0x92, 0x00, 0x00, 0x00, 0x00, 0x00])
        );

        // Rumble-only update must leave triggers/lightbar/LEDs as None.
        let rumble_only = OutputState {
            update_rumble: true,
            motor_left: 0,
            motor_right: 0,
            ..Default::default()
        };
        encode_output_report(&rumble_only, Transport::Usb, 0, &mut usb).unwrap();
        let parsed_r = parse_usb_output_report(&usb).unwrap();
        assert_eq!(parsed_r.rumble, Some((0, 0)));
        assert_eq!(parsed_r.lightbar_rgb, None);
        assert_eq!(parsed_r.player_leds, None);
        assert_eq!(parsed_r.left_trigger_raw, None);
        assert_eq!(parsed_r.right_trigger_raw, None);

        // Reject wrong report id and short buffers.
        let mut bad_id = [0u8; 48];
        bad_id[0] = 0x31;
        assert_eq!(
            parse_usb_output_report(&bad_id),
            Err(ProtocolError::UnexpectedReportId(0x31))
        );
        assert!(matches!(
            parse_usb_output_report(&[0x02; 16]),
            Err(ProtocolError::ReportTooShort { expected: 48, actual: 16 })
        ));
    }

    #[test]
    fn bt_0x31_audio_control_leaves_rumble_clear() {
        let state = OutputState {
            update_headphone_volume: true,
            headphone_volume: scale_headphone_volume(1.0),
            update_speaker_volume: true,
            speaker_volume: scale_speaker_volume(1.0),
            update_mic_volume: true,
            mic_volume: 0x40,
            update_audio_control: true,
            audio_control: BtAudioRoute::Speaker.audio_control_byte(),
            unmute_audio: true,
            ..Default::default()
        };

        let mut bt = [0u8; 78];
        encode_output_report(&state, Transport::Bluetooth, 2, &mut bt).unwrap();
        // valid_flag0 at bt[3]: audio bits 0x10|0x20|0x40|0x80 = 0xF0, rumble bits 0x03 must stay 0.
        assert_eq!(bt[3], 0xF0);
        // valid_flag1 at bt[4]: AllowAudioMute 0x02.
        assert_eq!(bt[4], 0x02);
        assert_eq!(bt[7], 0x7F); // headphone_volume
        assert_eq!(bt[8], 0x64); // speaker_volume
        assert_eq!(bt[9], 0x40); // mic_volume
        assert_eq!(bt[10], 0x30); // audio_control (speaker only)
        assert_eq!(bt[12], 0x00); // power_save / mute control unmuted
    }
}
