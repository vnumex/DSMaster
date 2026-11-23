pub mod calibration;
pub mod constants;
pub mod descriptors;
pub mod features;
pub mod input;
pub mod output;
pub mod triggers;

pub use calibration::{ImuCalibration, parse_calibration_report};
pub use constants::{
    DS_FEATURE_REPORT_CALIBRATION, DS_FEATURE_REPORT_CALIBRATION_SIZE, DS_GYRO_RES_PER_DEG_S,
    DUALSENSE_EDGE_PID, DUALSENSE_PID, ProtocolError, SONY_VID, Transport,
};
#[allow(unused_imports)]
pub use constants::{
    FEATURE_REPORT_FIRMWARE_ID, FEATURE_REPORT_FIRMWARE_SIZE, FEATURE_REPORT_PAIRING_ID,
    FEATURE_REPORT_PAIRING_SIZE, VIRTUAL_SERIAL_PREFIX,
};
#[allow(unused_imports)]
pub use descriptors::{
    CONFIG_DESCRIPTOR, DEVICE_DESCRIPTOR, HID_DESCRIPTOR, HID_REPORT_DESCRIPTOR,
    build_string_descriptor,
};
#[allow(unused_imports)]
pub use features::{
    build_calibration_feature_05, build_firmware_feature_20, build_pairing_feature_09,
};
pub use input::{BatteryStatus, DPad, DualSenseButtons, RawInputState, parse_input_report};
#[allow(unused_imports)]
pub use input::encode_usb_input_report;
pub use output::{OutputState, encode_output_report};
#[allow(unused_imports)]
pub use output::{ParsedOutputReport, parse_usb_output_report};
pub use triggers::AdaptiveTriggerEffect;
