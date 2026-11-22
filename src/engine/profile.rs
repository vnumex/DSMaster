use crate::emu::EmulationMode;
use crate::protocol::AdaptiveTriggerEffect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerProfile {
    pub emulation: EmulationMode,
    pub lightbar_rgb: [u8; 3],
    pub player_leds: u8,
    pub enable_rumble: bool,
    pub left_trigger: AdaptiveTriggerEffect,
    pub right_trigger: AdaptiveTriggerEffect,
}

impl Default for ControllerProfile {
    fn default() -> Self {
        Self {
            emulation: EmulationMode::Xbox360,
            lightbar_rgb: [0, 96, 255],
            player_leds: 0x04,
            enable_rumble: true,
            left_trigger: AdaptiveTriggerEffect::Off,
            right_trigger: AdaptiveTriggerEffect::Off,
        }
    }
}
