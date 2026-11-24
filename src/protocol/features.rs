#![allow(dead_code)]

use super::calibration::parse_calibration_report;
use super::constants::{
    DS_FEATURE_REPORT_CALIBRATION, DS_FEATURE_REPORT_CALIBRATION_SIZE,
    FEATURE_REPORT_FIRMWARE_ID, FEATURE_REPORT_FIRMWARE_SIZE, FEATURE_REPORT_PAIRING_ID,
    FEATURE_REPORT_PAIRING_SIZE,
};

#[inline]
fn le_i16(buf: &[u8], offset: usize) -> i32 {
    i16::from_le_bytes([buf[offset], buf[offset + 1]]) as i32
}

#[inline]
fn write_le_i16(buf: &mut [u8], offset: usize, val: i16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
}

/// Validates that a raw 41-byte `0x05` calibration report has non-zero speed and axis ranges
/// so external consumers (SDL, Steam Input, games) will never encounter division by zero or NaN.
fn is_valid_calibration_raw(raw: &[u8; DS_FEATURE_REPORT_CALIBRATION_SIZE]) -> bool {
    if raw[0] != DS_FEATURE_REPORT_CALIBRATION {
        return false;
    }
    if parse_calibration_report(raw).is_err() {
        return false;
    }

    let speed_2x = le_i16(raw, 19) + le_i16(raw, 21);
    if speed_2x == 0 {
        return false;
    }

    // Gyro axes: (bias_off, plus_off, minus_off)
    for (bias_off, plus_off, minus_off) in [(1, 7, 9), (3, 11, 13), (5, 15, 17)] {
        let bias = le_i16(raw, bias_off);
        let plus = le_i16(raw, plus_off);
        let minus = le_i16(raw, minus_off);
        let denom = (plus - bias).abs() + (minus - bias).abs();
        if denom == 0 || (plus - minus) == 0 {
            return false;
        }
    }

    // Accel axes: (plus_off, minus_off)
    for (plus_off, minus_off) in [(23, 25), (27, 29), (31, 33)] {
        if le_i16(raw, plus_off) - le_i16(raw, minus_off) == 0 {
            return false;
        }
    }

    true
}

/// Builds a 41-byte USB DualSense IMU Calibration Feature Report (`0x05`).
///
/// If `raw_override` is provided and passes non-zero denominator validation, it is returned
/// verbatim (forwarding the physical controller's factory calibration). Otherwise, emits a
/// clean-room neutral 1:1 identity calibration with zero biases and safe non-zero ranges.
pub fn build_calibration_feature_05(
    raw_override: Option<&[u8; DS_FEATURE_REPORT_CALIBRATION_SIZE]>,
) -> [u8; DS_FEATURE_REPORT_CALIBRATION_SIZE] {
    if let Some(raw) = raw_override {
        if is_valid_calibration_raw(raw) {
            return *raw;
        }
    }

    let mut out = [0u8; DS_FEATURE_REPORT_CALIBRATION_SIZE];
    out[0] = DS_FEATURE_REPORT_CALIBRATION;

    // Neutral zero gyro biases (offsets 1..7 already 0).
    // Gyro +/- reference counts (+/-10800 at +/-540 deg/s => 20 LSB/(deg/s), non-zero denom 21600).
    for (plus_off, minus_off) in [(7, 9), (11, 13), (15, 17)] {
        write_le_i16(&mut out, plus_off, 10_800);
        write_le_i16(&mut out, minus_off, -10_800);
    }
    write_le_i16(&mut out, 19, 540);
    write_le_i16(&mut out, 21, 540);

    // Accel +/-1g reference counts (+/-8192 => exact 1:1 scaling at 8192 LSB/g).
    for (plus_off, minus_off) in [(23, 25), (27, 29), (31, 33)] {
        write_le_i16(&mut out, plus_off, 8192);
        write_le_i16(&mut out, minus_off, -8192);
    }

    out
}

/// Builds a 20-byte USB DualSense Pairing / MAC Address Feature Report (`0x09`).
///
/// Places `virtual_mac` (supplied in standard MSB-first order, e.g. `[0x02, 0x44, 0x53, 0x4D, 0x00, 0x01]`)
/// into bytes `1..7` in little-endian (reversed) wire order, with remaining bytes zeroed.
pub fn build_pairing_feature_09(virtual_mac: [u8; 6]) -> [u8; FEATURE_REPORT_PAIRING_SIZE] {
    let mut out = [0u8; FEATURE_REPORT_PAIRING_SIZE];
    out[0] = FEATURE_REPORT_PAIRING_ID;
    for i in 0..6 {
        out[1 + i] = virtual_mac[5 - i];
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FirmwareInfo {
    pub build_date: [u8; 11],
    pub build_time: [u8; 8],
    pub fw_type: u16,
    pub hw_info: u32,
    pub main_fw_version: u32,
    pub update_version: u16,
}

/// Parses a 20-byte DualSense Pairing / MAC Address Feature Report (`0x09`).
/// Returns `None` if the report ID mismatches, buffer is too short, or MAC is all zeros.
pub fn parse_pairing_feature_09(buf: &[u8]) -> Option<[u8; 6]> {
    if buf.len() < 7 || buf[0] != FEATURE_REPORT_PAIRING_ID {
        return None;
    }
    let mac = [buf[6], buf[5], buf[4], buf[3], buf[2], buf[1]];
    if mac == [0; 6] {
        return None;
    }
    Some(mac)
}

/// Parses a 64-byte DualSense Firmware & Hardware Info Feature Report (`0x20`).
pub fn parse_firmware_feature_20(buf: &[u8]) -> Option<FirmwareInfo> {
    if buf.len() < 46 || buf[0] != FEATURE_REPORT_FIRMWARE_ID {
        return None;
    }
    let mut build_date = [0u8; 11];
    build_date.copy_from_slice(&buf[1..12]);
    let mut build_time = [0u8; 8];
    build_time.copy_from_slice(&buf[12..20]);
    Some(FirmwareInfo {
        build_date,
        build_time,
        fw_type: u16::from_le_bytes([buf[20], buf[21]]),
        hw_info: u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]),
        main_fw_version: u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]),
        update_version: u16::from_le_bytes([buf[44], buf[45]]),
    })
}

/// Builds a 64-byte clean-room USB DualSense Firmware & Hardware Info Feature Report (`0x20`).
///
/// Reports modern DualSense firmware metadata so host libraries (SDL, Steam Input, games)
/// enable full adaptive trigger and enhanced rumble support without cloning real firmware images.
pub fn build_firmware_feature_20() -> [u8; FEATURE_REPORT_FIRMWARE_SIZE] {
    let mut out = [0u8; FEATURE_REPORT_FIRMWARE_SIZE];
    out[0] = FEATURE_REPORT_FIRMWARE_ID;

    // Clean-room ASCII build date (11 bytes, [1..12]) and build time (8 bytes, [12..20]).
    out[1..12].copy_from_slice(b"Jan 01 2026");
    out[12..20].copy_from_slice(b"00:00:00");

    // fw_type (u16 LE) = 0x0002
    out[20..22].copy_from_slice(&0x0002u16.to_le_bytes());
    // sw_series (u16 LE) = 0x0000
    out[22..24].copy_from_slice(&0x0000u16.to_le_bytes());
    // hw_info (u32 LE) = 0x0000_0414
    out[24..28].copy_from_slice(&0x0000_0414u32.to_le_bytes());
    // main_fw_version (u32 LE) = 0x0009_0000
    out[28..32].copy_from_slice(&0x0009_0000u32.to_le_bytes());
    // update_version (u16 LE) = 0x0900 (prevents Steam from prompting to update virtual firmware)
    out[44..46].copy_from_slice(&0x0900u16.to_le_bytes());

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::constants::DS_ACC_RES_PER_G;

    #[test]
    fn default_calibration_05_parses_cleanly_without_zero_denominators() {
        let rep = build_calibration_feature_05(None);
        assert_eq!(rep.len(), 41);
        assert_eq!(rep[0], 0x05);
        assert!(is_valid_calibration_raw(&rep));

        let cal = parse_calibration_report(&rep).unwrap();
        for axis in cal.gyro.iter().chain(cal.accel.iter()) {
            assert_ne!(axis.sens_denom, 0);
            assert_ne!(axis.sens_numer, 0);
        }
        assert_eq!(cal.accel[0].apply(8192), DS_ACC_RES_PER_G);
        assert_eq!(cal.accel[1].apply(-8192), -DS_ACC_RES_PER_G);
        assert_eq!(cal.accel[2].apply(0), 0);
        assert_ne!(cal.gyro[0].apply(100), 0);
    }

    #[test]
    fn calibration_05_accepts_valid_override_and_rejects_zeroed_override() {
        let mut custom = build_calibration_feature_05(None);
        write_le_i16(&mut custom, 1, 12); // non-zero pitch bias
        assert_eq!(build_calibration_feature_05(Some(&custom)), custom);

        let mut bad = [0u8; 41];
        bad[0] = 0x05;
        let fallback = build_calibration_feature_05(Some(&bad));
        assert!(is_valid_calibration_raw(&fallback));
        assert_ne!(fallback, bad);
    }

    #[test]
    fn pairing_09_encodes_mac_in_little_endian_order() {
        let mac = [0x02, 0x44, 0x53, 0x4D, 0x00, 0x01];
        let rep = build_pairing_feature_09(mac);
        assert_eq!(rep.len(), 20);
        assert_eq!(rep[0], 0x09);
        assert_eq!(&rep[1..7], &[0x01, 0x00, 0x4D, 0x53, 0x44, 0x02]);
        assert!(rep[7..].iter().all(|&b| b == 0));
    }

    #[test]
    fn firmware_20_fields_and_length() {
        let rep = build_firmware_feature_20();
        assert_eq!(rep.len(), 64);
        assert_eq!(rep[0], 0x20);
        assert_eq!(&rep[1..12], b"Jan 01 2026");
        assert_eq!(&rep[12..20], b"00:00:00");
        assert_eq!(u16::from_le_bytes([rep[20], rep[21]]), 0x0002);
        assert_eq!(u32::from_le_bytes([rep[24], rep[25], rep[26], rep[27]]), 0x0000_0414);
        assert_eq!(u32::from_le_bytes([rep[28], rep[29], rep[30], rep[31]]), 0x0009_0000);
        assert_eq!(u16::from_le_bytes([rep[44], rep[45]]), 0x0900);

        let parsed_fw = parse_firmware_feature_20(&rep).unwrap();
        assert_eq!(&parsed_fw.build_date, b"Jan 01 2026");
        assert_eq!(&parsed_fw.build_time, b"00:00:00");
        assert_eq!(parsed_fw.fw_type, 0x0002);
        assert_eq!(parsed_fw.hw_info, 0x0000_0414);
        assert_eq!(parsed_fw.main_fw_version, 0x0009_0000);
        assert_eq!(parsed_fw.update_version, 0x0900);
    }

    #[test]
    fn pairing_09_round_trip_and_zero_mac_rejection() {
        let mac = [0xE8, 0x47, 0x3A, 0x3F, 0x84, 0x3C];
        let rep = build_pairing_feature_09(mac);
        assert_eq!(parse_pairing_feature_09(&rep), Some(mac));
        let zero_rep = build_pairing_feature_09([0; 6]);
        assert_eq!(parse_pairing_feature_09(&zero_rep), None);
    }
}
