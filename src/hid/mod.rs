pub mod device;
pub mod enumerate;
pub mod hotplug;

use thiserror::Error;

pub use device::{CancelHandle, DualSenseDevice, ReadOutcome};
pub use enumerate::{DeviceInfo, enumerate_dualsense_devices, inspect_hid_path};
pub use hotplug::{HotPlugEvent, HotPlugMonitor};

#[derive(Debug, Error)]
pub enum HidError {
    #[error("Win32 API {0} failed with HRESULT/code 0x{1:08X}")]
    Win32(&'static str, u32),
    #[error("OS error in {0}: code {1}")]
    Os(&'static str, u32),
    #[error("CfgMgr32 {0} failed with CONFIGRET 0x{1:08X}")]
    ConfigManager(&'static str, u32),
    #[error("HID report size out of bounds (input={input}, output={output})")]
    ReportSizeOutOfBounds { input: usize, output: usize },
}
