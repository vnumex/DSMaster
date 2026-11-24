use thiserror::Error;

pub const SONY_VID: u16 = 0x054C;
pub const DUALSENSE_PID: u16 = 0x0CE6;
pub const DUALSENSE_EDGE_PID: u16 = 0x0DF2;

/// Prefix embedded in virtual DualSense USB serial strings for self-exclusion & HidHide safety.
#[allow(dead_code)]
pub const VIRTUAL_SERIAL_PREFIX: &str = "DSMASTER-VDS";

pub const DS_INPUT_REPORT_USB: u8 = 0x01;
pub const DS_INPUT_REPORT_USB_SIZE: usize = 64;
pub const DS_INPUT_REPORT_BT: u8 = 0x31;
pub const DS_INPUT_REPORT_BT_SIZE: usize = 78;

pub const DS_OUTPUT_REPORT_USB: u8 = 0x02;
pub const DS_OUTPUT_REPORT_USB_MIN_SIZE: usize = 48;
pub const DS_OUTPUT_REPORT_BT: u8 = 0x31;
pub const DS_OUTPUT_REPORT_BT_SIZE: usize = 78;
pub const DS_OUTPUT_REPORT_BT_HAPTICS: u8 = 0x32;
pub const DS_OUTPUT_REPORT_BT_HAPTICS_SIZE: usize = 142;
pub const DS_OUTPUT_REPORT_BT_AUDIO: u8 = 0x35;
pub const DS_OUTPUT_REPORT_BT_AUDIO_SIZE: usize = 334;

pub const DS_BT_AUDIO_SUBPKT_SESSION: u8 = 0x11;
pub const DS_BT_AUDIO_SUBPKT_HAPTICS: u8 = 0x12;
pub const DS_BT_AUDIO_SUBPKT_SPEAKER: u8 = 0x13;
pub const DS_BT_AUDIO_SUBPKT_HEADSET: u8 = 0x16;
pub const DS_BT_AUDIO_OPUS_BYTES: usize = 200;
pub const DS_BT_MIC_OPUS_BYTES: usize = 71;
pub const DS_BT_MIC_VOLUME_MAX: u8 = 0x40;
pub const DS_BT_HAPTICS_PCM_BYTES: usize = 64;
pub const DS_BT_AUDIO_BUFFER_LEN_DEFAULT: u8 = 48;
pub const DS_BT_AUDIO_COMBINED_LANE_OFFSET: usize = 77;

pub const DS_FEATURE_REPORT_CALIBRATION: u8 = 0x05;
pub const DS_FEATURE_REPORT_CALIBRATION_SIZE: usize = 41;
#[allow(dead_code)]
pub const FEATURE_REPORT_PAIRING_ID: u8 = 0x09;
#[allow(dead_code)]
pub const FEATURE_REPORT_PAIRING_SIZE: usize = 20;
#[allow(dead_code)]
pub const FEATURE_REPORT_FIRMWARE_ID: u8 = 0x20;
#[allow(dead_code)]
pub const FEATURE_REPORT_FIRMWARE_SIZE: usize = 64;

pub const PS_INPUT_CRC32_SEED: u8 = 0xA1;
pub const PS_OUTPUT_CRC32_SEED: u8 = 0xA2;

pub const DS_OUTPUT_TAG: u8 = 0x10;

pub const DS_ACC_RES_PER_G: i32 = 8192;
pub const DS_ACC_RANGE: i32 = 4 * DS_ACC_RES_PER_G;
pub const DS_GYRO_RES_PER_DEG_S: i32 = 1024;
pub const DS_GYRO_RANGE: i32 = 2048 * DS_GYRO_RES_PER_DEG_S;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Bluetooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProtocolError {
    #[error("input report too short: expected at least {expected} bytes, got {actual}")]
    ReportTooShort { expected: usize, actual: usize },
    #[error("output buffer too small: expected at least {expected} bytes, got {actual}")]
    BufferTooSmall { expected: usize, actual: usize },
    #[error("unexpected report id 0x{0:02X}")]
    UnexpectedReportId(u8),
    #[error(
        "bluetooth input report is in minimal 0x01 mode; query feature report 0x05 to activate 0x31"
    )]
    MinimalBluetoothReport,
    #[error("bluetooth 0x31 report carries microphone Opus audio rather than controller state")]
    BluetoothMicAudioPacket,
    #[error("invalid bluetooth CRC-32: expected 0x{expected:08X}, got 0x{actual:08X}")]
    InvalidCrc { expected: u32, actual: u32 },
}

/// Computes Sony's seeded little-endian IEEE 802.3 CRC-32.
#[inline]
pub fn crc32_with_seed(seed: u8, data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[seed]);
    hasher.update(data);
    hasher.finalize()
}
