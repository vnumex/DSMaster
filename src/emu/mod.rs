pub mod mapping;
pub mod vigem;

use crate::protocol::RawInputState;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmulationMode {
    None,
    #[default]
    Xbox360,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeedbackState {
    pub large_motor: u8,
    pub small_motor: u8,
    pub led_number: u8,
}

#[derive(Debug, Error)]
pub enum EmuError {
    #[error("ViGEmBus driver not found; install Nefarius ViGEmBus")]
    DriverNotFound,
    #[error("ViGEmBus version mismatch or rejected")]
    IncompatibleVersion,
    #[error("no free ViGEmBus target serial slot")]
    NoFreeSlot,
    #[error("Win32 API {0} failed with code 0x{1:08X}")]
    Win32(&'static str, u32),
    #[error("OS error in {0}: code {1}")]
    Os(&'static str, u32),
}

pub trait VirtualController: Send {
    fn submit(&mut self, input: &RawInputState) -> Result<(), EmuError>;
    fn poll_feedback(&mut self) -> Option<FeedbackState>;
}
