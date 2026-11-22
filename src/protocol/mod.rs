pub mod calibration;
pub mod constants;
pub mod input;
pub mod output;
pub mod triggers;

pub use calibration::{ImuCalibration, parse_calibration_report};
pub use constants::{
    DS_FEATURE_REPORT_CALIBRATION, DS_FEATURE_REPORT_CALIBRATION_SIZE, DUALSENSE_EDGE_PID,
    DUALSENSE_PID, ProtocolError, SONY_VID, Transport,
};
pub use input::{BatteryStatus, DPad, DualSenseButtons, RawInputState, parse_input_report};
pub use output::{OutputState, encode_output_report};
pub use triggers::AdaptiveTriggerEffect;
