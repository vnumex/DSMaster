use serde::{Deserialize, Serialize};

use crate::emu::EmulationMode;
use crate::protocol::AdaptiveTriggerEffect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerPreset {
    #[default]
    Off,
    Rigid,
    Weapon,
    Pulse,
}

impl TriggerPreset {
    pub const ALL: [Self; 4] = [Self::Off, Self::Rigid, Self::Weapon, Self::Pulse];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Rigid => "Rigid",
            Self::Weapon => "Weapon",
            Self::Pulse => "Pulse",
        }
    }

    pub fn effect(self) -> AdaptiveTriggerEffect {
        match self {
            Self::Off => AdaptiveTriggerEffect::Off,
            Self::Rigid => AdaptiveTriggerEffect::Feedback {
                start_position: 1,
                strength: 6,
            },
            Self::Weapon => AdaptiveTriggerEffect::Weapon {
                start_position: 2,
                end_position: 6,
                strength: 7,
            },
            Self::Pulse => AdaptiveTriggerEffect::Vibration {
                position: 1,
                amplitude: 5,
                frequency: 35,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ResponseCurve {
    #[default]
    Linear,
    Quadratic,
    Cubic,
    Aggressive,
}

impl ResponseCurve {
    pub const ALL: [Self; 4] = [Self::Linear, Self::Quadratic, Self::Cubic, Self::Aggressive];

    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Quadratic => "Quadratic",
            Self::Cubic => "Cubic",
            Self::Aggressive => "Aggressive",
        }
    }

    #[inline]
    pub fn apply(self, t: f32) -> f32 {
        let clamped = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => clamped,
            Self::Quadratic => clamped.powf(1.5),
            Self::Cubic => clamped.powf(2.2),
            Self::Aggressive => clamped.powf(0.65),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickTuning {
    pub deadzone: f32,
    pub anti_deadzone: f32,
    pub curve: ResponseCurve,
}

impl Default for StickTuning {
    fn default() -> Self {
        Self {
            deadzone: 0.05,
            anti_deadzone: 0.0,
            curve: ResponseCurve::Linear,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerTuning {
    pub deadzone: u8,
    pub hair_trigger: u8,
}

impl Default for TriggerTuning {
    fn default() -> Self {
        Self {
            deadzone: 0,
            hair_trigger: 255,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ButtonPreset {
    #[default]
    Standard,
    SwapFace,
    Tactical,
    BumperJumper,
}

impl ButtonPreset {
    pub const ALL: [Self; 4] = [
        Self::Standard,
        Self::SwapFace,
        Self::Tactical,
        Self::BumperJumper,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::SwapFace => "Swap Face",
            Self::Tactical => "Tactical",
            Self::BumperJumper => "Bumper Jumper",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GyroAimMode {
    #[default]
    Off,
    HoldL2,
    HoldL1,
    AlwaysOn,
}

impl GyroAimMode {
    pub const ALL: [Self; 4] = [Self::Off, Self::HoldL2, Self::HoldL1, Self::AlwaysOn];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::HoldL2 => "Hold L2",
            Self::HoldL1 => "Hold L1",
            Self::AlwaysOn => "Always On",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerProfile {
    pub emulation: EmulationMode,
    pub lightbar_rgb: [u8; 3],
    pub player_leds: u8,
    pub enable_rumble: bool,
    pub left_trigger_preset: TriggerPreset,
    pub right_trigger_preset: TriggerPreset,
    pub left_stick: StickTuning,
    pub right_stick: StickTuning,
    pub triggers: TriggerTuning,
    pub button_preset: ButtonPreset,
    pub touchpad_mouse: bool,
    pub touchpad_sensitivity: f32,
    pub gyro_mode: GyroAimMode,
    pub gyro_sensitivity: f32,
}

impl Default for ControllerProfile {
    fn default() -> Self {
        Self {
            emulation: EmulationMode::Xbox360,
            lightbar_rgb: [0, 96, 255],
            player_leds: 0x04,
            enable_rumble: true,
            left_trigger_preset: TriggerPreset::Off,
            right_trigger_preset: TriggerPreset::Off,
            left_stick: StickTuning::default(),
            right_stick: StickTuning::default(),
            triggers: TriggerTuning::default(),
            button_preset: ButtonPreset::Standard,
            touchpad_mouse: false,
            touchpad_sensitivity: 1.0,
            gyro_mode: GyroAimMode::Off,
            gyro_sensitivity: 1.0,
        }
    }
}
