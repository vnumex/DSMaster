pub mod dualsense;
pub mod mapping;
pub mod vigem;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use self::dualsense::{UsbipDualSenseTarget, VirtualDualSenseConfig};
use self::vigem::VigemClient;
use crate::engine::processing::ProcessedInput;
use crate::protocol::constants::DS_INPUT_REPORT_USB_SIZE;
use crate::protocol::{ParsedOutputReport, encode_usb_input_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EmulationMode {
    None,
    #[default]
    Xbox360,
    DualShock4,
    DualSense,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeedbackState {
    /// Rumble motor intensities as `(large_motor, small_motor)`.
    pub rumble: Option<(u8, u8)>,
    pub lightbar_rgb: Option<[u8; 3]>,
    pub player_leds: Option<u8>,
    pub left_trigger_raw: Option<[u8; 11]>,
    pub right_trigger_raw: Option<[u8; 11]>,
}

impl From<ParsedOutputReport> for FeedbackState {
    #[inline]
    fn from(r: ParsedOutputReport) -> Self {
        Self {
            rumble: r.rumble,
            lightbar_rgb: r.lightbar_rgb,
            player_leds: r.player_leds,
            left_trigger_raw: r.left_trigger_raw,
            right_trigger_raw: r.right_trigger_raw,
        }
    }
}

#[derive(Debug, Error)]
pub enum EmuError {
    #[error("ViGEmBus driver not found; install Nefarius ViGEmBus")]
    DriverNotFound,
    #[error("ViGEmBus version mismatch or rejected")]
    IncompatibleVersion,
    #[error("no free ViGEmBus target serial slot")]
    NoFreeSlot,
    #[error("virtual DualSense backend error: {0}")]
    VirtualDualSense(String),
    #[error("Win32 API {0} failed with code 0x{1:08X}")]
    Win32(&'static str, u32),
    #[error("OS error in {0}: code {1}")]
    Os(&'static str, u32),
}

pub trait VirtualController: Send {
    fn submit(&mut self, input: &ProcessedInput) -> Result<(), EmuError>;
    fn poll_feedback(&mut self) -> Option<FeedbackState>;
}

/// Transport-agnostic container of available virtual controller backends shared across sessions.
#[derive(Clone, Default)]
pub struct EmuBackends {
    pub vigem: Option<VigemClient>,
}

impl EmuBackends {
    /// Spawns a Virtual DualSense target via the configured USB/IP (`usbip-win2`) backend.
    pub fn spawn_dualsense(
        &self,
        config: VirtualDualSenseConfig,
    ) -> Result<Box<dyn VirtualController>, EmuError> {
        let target = UsbipDualSenseTarget::spawn(config).map_err(EmuError::VirtualDualSense)?;
        Ok(Box::new(target))
    }
}

impl VirtualController for UsbipDualSenseTarget {
    #[inline]
    fn submit(&mut self, input: &ProcessedInput) -> Result<(), EmuError> {
        let mut report = [0u8; DS_INPUT_REPORT_USB_SIZE];
        let raw = input.to_raw_input_state();
        encode_usb_input_report(&raw, &mut report);
        self.submit_report(&report);
        Ok(())
    }

    #[inline]
    fn poll_feedback(&mut self) -> Option<FeedbackState> {
        UsbipDualSenseTarget::poll_feedback(self).map(FeedbackState::from)
    }
}
