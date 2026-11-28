use serde::{Deserialize, Serialize};

use crate::emu::EmulationMode;
use crate::protocol::{AdaptiveTriggerEffect, BtAudioRoute, BtMicRoute, LedBrightness, MicMuteLed};

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

fn default_low_battery_threshold() -> u8 {
    20
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LightbarMode {
    #[default]
    Static,
    Breathing,
    Rainbow,
    BatteryIndicator,
    TriggerReactive,
}

impl LightbarMode {
    pub const ALL: [Self; 5] = [
        Self::Static,
        Self::Breathing,
        Self::Rainbow,
        Self::BatteryIndicator,
        Self::TriggerReactive,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Static => "Static",
            Self::Breathing => "Breathing",
            Self::Rainbow => "Rainbow",
            Self::BatteryIndicator => "Battery",
            Self::TriggerReactive => "Trigger",
        }
    }

    #[inline]
    pub fn uses_custom_color(self) -> bool {
        matches!(self, Self::Static | Self::Breathing | Self::TriggerReactive)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MicMuteLedOverride {
    #[default]
    Native,
    Off,
    Solid,
    Pulse,
}

impl MicMuteLedOverride {
    pub const ALL: [Self; 4] = [Self::Native, Self::Off, Self::Solid, Self::Pulse];

    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "Native",
            Self::Off => "Off",
            Self::Solid => "Solid",
            Self::Pulse => "Pulse",
        }
    }

    pub fn to_protocol(self) -> Option<MicMuteLed> {
        match self {
            Self::Native => None,
            Self::Off => Some(MicMuteLed::Off),
            Self::Solid => Some(MicMuteLed::Solid),
            Self::Pulse => Some(MicMuteLed::Pulse),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CustomTriggerMode {
    #[default]
    Feedback,
    Weapon,
    Bow,
    Slope,
    Vibration,
    Machine,
    Galloping,
}

impl CustomTriggerMode {
    pub const ALL: [Self; 7] = [
        Self::Feedback,
        Self::Weapon,
        Self::Bow,
        Self::Slope,
        Self::Vibration,
        Self::Machine,
        Self::Galloping,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Feedback => "Feedback",
            Self::Weapon => "Weapon",
            Self::Bow => "Bow",
            Self::Slope => "Slope",
            Self::Vibration => "Vibration",
            Self::Machine => "Machine",
            Self::Galloping => "Galloping",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomTriggerConfig {
    pub mode: CustomTriggerMode,
    pub start_position: u8,
    pub end_position: u8,
    pub strength: u8,
    pub secondary_strength: u8,
    pub frequency: u8,
}

impl Default for CustomTriggerConfig {
    fn default() -> Self {
        Self {
            mode: CustomTriggerMode::Feedback,
            start_position: 1,
            end_position: 6,
            strength: 5,
            secondary_strength: 7,
            frequency: 30,
        }
    }
}

impl CustomTriggerConfig {
    #[allow(dead_code)]
    pub fn effect(self) -> AdaptiveTriggerEffect {
        match self.mode {
            CustomTriggerMode::Feedback => AdaptiveTriggerEffect::Feedback {
                start_position: self.start_position.min(9),
                strength: self.strength.clamp(1, 8),
            },
            CustomTriggerMode::Weapon => AdaptiveTriggerEffect::Weapon {
                start_position: self.start_position.clamp(2, 7),
                end_position: self.end_position.clamp(self.start_position.clamp(2, 7) + 1, 8),
                strength: self.strength.clamp(1, 8),
            },
            CustomTriggerMode::Bow => AdaptiveTriggerEffect::Bow {
                start_position: self.start_position.clamp(0, 7),
                end_position: self.end_position.clamp(self.start_position.clamp(0, 7) + 1, 8),
                strength: self.strength.clamp(1, 8),
                snap_force: self.secondary_strength.clamp(1, 8),
            },
            CustomTriggerMode::Slope => AdaptiveTriggerEffect::SlopeFeedback {
                start_position: self.start_position.min(8),
                end_position: self.end_position.clamp(self.start_position.min(8) + 1, 9),
                start_strength: self.strength.clamp(1, 8),
                end_strength: self.secondary_strength.clamp(1, 8),
            },
            CustomTriggerMode::Vibration => AdaptiveTriggerEffect::Vibration {
                position: self.start_position.min(9),
                amplitude: self.strength.clamp(1, 8),
                frequency: self.frequency.clamp(1, 100),
            },
            CustomTriggerMode::Machine => AdaptiveTriggerEffect::Machine {
                start_position: self.start_position.clamp(1, 8),
                end_position: self.end_position.clamp(self.start_position.clamp(1, 8) + 1, 9),
                strength_a: self.strength.saturating_sub(1).min(7),
                strength_b: self.secondary_strength.saturating_sub(1).min(7),
                frequency: self.frequency.clamp(1, 100),
                period: 2,
            },
            CustomTriggerMode::Galloping => AdaptiveTriggerEffect::Galloping {
                start_position: self.start_position.min(8),
                end_position: self.end_position.clamp(self.start_position.min(8) + 1, 9),
                first_foot: 1,
                second_foot: 4,
                frequency: self.frequency.clamp(1, 100),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerPreset {
    #[default]
    Off,
    Soft,
    Rigid,
    Weapon,
    Bow,
    Slope,
    Pulse,
    Machine,
    Galloping,
    Custom,
}

impl TriggerPreset {
    pub const ALL: [Self; 10] = [
        Self::Off,
        Self::Soft,
        Self::Rigid,
        Self::Weapon,
        Self::Bow,
        Self::Slope,
        Self::Pulse,
        Self::Machine,
        Self::Galloping,
        Self::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Soft => "Soft",
            Self::Rigid => "Rigid",
            Self::Weapon => "Weapon",
            Self::Bow => "Bow",
            Self::Slope => "Slope",
            Self::Pulse => "Pulse",
            Self::Machine => "Machine",
            Self::Galloping => "Galloping",
            Self::Custom => "Custom",
        }
    }

    #[allow(dead_code)]
    pub fn effect(self) -> AdaptiveTriggerEffect {
        self.effect_with_custom(&CustomTriggerConfig::default())
    }

    pub fn effect_with_custom(self, custom: &CustomTriggerConfig) -> AdaptiveTriggerEffect {
        match self {
            Self::Off => AdaptiveTriggerEffect::Off,
            Self::Soft => AdaptiveTriggerEffect::Feedback {
                start_position: 2,
                strength: 3,
            },
            Self::Rigid => AdaptiveTriggerEffect::Feedback {
                start_position: 1,
                strength: 6,
            },
            Self::Weapon => AdaptiveTriggerEffect::Weapon {
                start_position: 2,
                end_position: 6,
                strength: 7,
            },
            Self::Bow => AdaptiveTriggerEffect::Bow {
                start_position: 1,
                end_position: 7,
                strength: 6,
                snap_force: 7,
            },
            Self::Slope => AdaptiveTriggerEffect::SlopeFeedback {
                start_position: 1,
                end_position: 8,
                start_strength: 2,
                end_strength: 7,
            },
            Self::Pulse => AdaptiveTriggerEffect::Vibration {
                position: 1,
                amplitude: 5,
                frequency: 35,
            },
            Self::Machine => AdaptiveTriggerEffect::Machine {
                start_position: 2,
                end_position: 8,
                strength_a: 2,
                strength_b: 6,
                frequency: 28,
                period: 2,
            },
            Self::Galloping => AdaptiveTriggerEffect::Galloping {
                start_position: 1,
                end_position: 8,
                first_foot: 1,
                second_foot: 5,
                frequency: 8,
            },
            Self::Custom => custom.effect(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeadzoneShape {
    #[default]
    Radial,
    Axial,
    Passthrough,
}

impl DeadzoneShape {
    pub const ALL: [Self; 3] = [Self::Radial, Self::Axial, Self::Passthrough];

    pub fn label(self) -> &'static str {
        match self {
            Self::Radial => "Radial",
            Self::Axial => "Axial",
            Self::Passthrough => "Passthrough",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickTuning {
    pub deadzone: f32,
    pub anti_deadzone: f32,
    pub outer_deadzone: f32,
    pub shape: DeadzoneShape,
    pub invert_x: bool,
    pub invert_y: bool,
    pub curve: ResponseCurve,
}

impl Default for StickTuning {
    fn default() -> Self {
        Self {
            deadzone: 0.05,
            anti_deadzone: 0.0,
            outer_deadzone: 0.0,
            shape: DeadzoneShape::Radial,
            invert_x: false,
            invert_y: false,
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

fn default_gyro_deadzone() -> f32 {
    1.0
}

fn default_turbo_rate() -> u8 {
    10
}

fn default_bt_speaker_volume() -> f32 {
    0.8
}

fn default_bt_haptics_gain() -> f32 {
    0.75
}

fn default_flick_pixels_per_turn() -> f32 {
    8000.0
}

fn default_flick_threshold() -> f32 {
    0.80
}

fn default_flick_ms() -> u16 {
    80
}

pub const MOD_CTRL: u8 = 1 << 0;
pub const MOD_SHIFT: u8 = 1 << 1;
pub const MOD_ALT: u8 = 1 << 2;
pub const MOD_WIN: u8 = 1 << 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKey {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    PlayPause,
    NextTrack,
    PrevTrack,
}

impl MediaKey {
    pub const ALL: [Self; 6] = [
        Self::VolumeMute,
        Self::VolumeDown,
        Self::VolumeUp,
        Self::PlayPause,
        Self::PrevTrack,
        Self::NextTrack,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::VolumeUp => "Vol +",
            Self::VolumeDown => "Vol -",
            Self::VolumeMute => "Mute",
            Self::PlayPause => "Play/Pause",
            Self::NextTrack => "Next",
            Self::PrevTrack => "Prev",
        }
    }

    #[inline]
    pub fn vk(self) -> u16 {
        match self {
            Self::VolumeMute => 0xAD,
            Self::VolumeDown => 0xAE,
            Self::VolumeUp => 0xAF,
            Self::NextTrack => 0xB0,
            Self::PrevTrack => 0xB1,
            Self::PlayPause => 0xB3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MacroStep {
    pub vk: u16,
    pub modifiers: u8,
    pub delay_ms: u16,
}

pub const MAX_MACRO_STEPS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ButtonAction {
    #[default]
    None,
    Key {
        vk: u16,
        #[serde(default)]
        modifiers: u8,
        #[serde(default)]
        hold: bool,
    },
    Media(MediaKey),
    Macro {
        steps: [MacroStep; MAX_MACRO_STEPS],
        len: u8,
    },
}

impl ButtonAction {
    #[inline]
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn format_label(&self) -> String {
        match *self {
            Self::None => "None".to_owned(),
            Self::Key {
                vk,
                modifiers,
                hold,
            } => {
                let combo = format_key_combo(vk, modifiers);
                if hold {
                    format!("{combo} (Hold)")
                } else {
                    combo
                }
            }
            Self::Media(m) => m.label().to_owned(),
            Self::Macro { steps, len } => {
                let n = (len as usize).min(MAX_MACRO_STEPS);
                if n == 0 {
                    "Macro (empty)".to_owned()
                } else if n == 1 {
                    format!("Macro: {}", format_key_combo(steps[0].vk, steps[0].modifiers))
                } else {
                    format!("Macro ({n} steps)")
                }
            }
        }
    }
}

pub fn format_key_combo(vk: u16, modifiers: u8) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(5);
    if (modifiers & MOD_CTRL) != 0 {
        parts.push("Ctrl");
    }
    if (modifiers & MOD_SHIFT) != 0 {
        parts.push("Shift");
    }
    if (modifiers & MOD_ALT) != 0 {
        parts.push("Alt");
    }
    if (modifiers & MOD_WIN) != 0 {
        parts.push("Win");
    }
    let key = vk_label(vk);
    if parts.is_empty() {
        key
    } else {
        format!("{}+{key}", parts.join("+"))
    }
}

pub fn vk_label(vk: u16) -> String {
    match vk {
        0x08 => "Backspace".to_owned(),
        0x09 => "Tab".to_owned(),
        0x0D => "Enter".to_owned(),
        0x13 => "Pause".to_owned(),
        0x14 => "CapsLock".to_owned(),
        0x1B => "Esc".to_owned(),
        0x20 => "Space".to_owned(),
        0x21 => "PgUp".to_owned(),
        0x22 => "PgDn".to_owned(),
        0x23 => "End".to_owned(),
        0x24 => "Home".to_owned(),
        0x25 => "Left".to_owned(),
        0x26 => "Up".to_owned(),
        0x27 => "Right".to_owned(),
        0x28 => "Down".to_owned(),
        0x2C => "PrintScr".to_owned(),
        0x2D => "Insert".to_owned(),
        0x2E => "Delete".to_owned(),
        0x30..=0x39 => ((vk as u8) as char).to_string(),
        0x41..=0x5A => ((vk as u8) as char).to_string(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x6A => "Num*".to_owned(),
        0x6B => "Num+".to_owned(),
        0x6D => "Num-".to_owned(),
        0x6E => "Num.".to_owned(),
        0x6F => "Num/".to_owned(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x90 => "NumLock".to_owned(),
        0x91 => "ScrLock".to_owned(),
        0xAD => "Vol Mute".to_owned(),
        0xAE => "Vol -".to_owned(),
        0xAF => "Vol +".to_owned(),
        0xB0 => "Next".to_owned(),
        0xB1 => "Prev".to_owned(),
        0xB2 => "Stop".to_owned(),
        0xB3 => "Play/Pause".to_owned(),
        0xBA => ";".to_owned(),
        0xBB => "=".to_owned(),
        0xBC => ",".to_owned(),
        0xBD => "-".to_owned(),
        0xBE => ".".to_owned(),
        0xBF => "/".to_owned(),
        0xC0 => "`".to_owned(),
        0xDB => "[".to_owned(),
        0xDC => "\\".to_owned(),
        0xDD => "]".to_owned(),
        0xDE => "'".to_owned(),
        other => format!("VK 0x{other:02X}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionMap {
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub cross: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub circle: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub square: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub triangle: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub l1: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub r1: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub l2: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub r2: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub l3: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub r3: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub options: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub create: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub touchpad: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub mic_mute: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub dpad_up: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub dpad_down: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub dpad_left: ButtonAction,
    #[serde(skip_serializing_if = "ButtonAction::is_none")]
    pub dpad_right: ButtonAction,
}

impl ActionMap {
    #[inline]
    pub fn get(&self, src: RemappableButton) -> ButtonAction {
        match src {
            RemappableButton::None => ButtonAction::None,
            RemappableButton::Cross => self.cross,
            RemappableButton::Circle => self.circle,
            RemappableButton::Square => self.square,
            RemappableButton::Triangle => self.triangle,
            RemappableButton::L1 => self.l1,
            RemappableButton::R1 => self.r1,
            RemappableButton::L2 => self.l2,
            RemappableButton::R2 => self.r2,
            RemappableButton::L3 => self.l3,
            RemappableButton::R3 => self.r3,
            RemappableButton::Options => self.options,
            RemappableButton::Create => self.create,
            RemappableButton::Touchpad => self.touchpad,
            RemappableButton::MicMute => self.mic_mute,
            RemappableButton::DpadUp => self.dpad_up,
            RemappableButton::DpadDown => self.dpad_down,
            RemappableButton::DpadLeft => self.dpad_left,
            RemappableButton::DpadRight => self.dpad_right,
        }
    }

    #[inline]
    pub fn set(&mut self, src: RemappableButton, action: ButtonAction) {
        match src {
            RemappableButton::None => {}
            RemappableButton::Cross => self.cross = action,
            RemappableButton::Circle => self.circle = action,
            RemappableButton::Square => self.square = action,
            RemappableButton::Triangle => self.triangle = action,
            RemappableButton::L1 => self.l1 = action,
            RemappableButton::R1 => self.r1 = action,
            RemappableButton::L2 => self.l2 = action,
            RemappableButton::R2 => self.r2 = action,
            RemappableButton::L3 => self.l3 = action,
            RemappableButton::R3 => self.r3 = action,
            RemappableButton::Options => self.options = action,
            RemappableButton::Create => self.create = action,
            RemappableButton::Touchpad => self.touchpad = action,
            RemappableButton::MicMute => self.mic_mute = action,
            RemappableButton::DpadUp => self.dpad_up = action,
            RemappableButton::DpadDown => self.dpad_down = action,
            RemappableButton::DpadLeft => self.dpad_left = action,
            RemappableButton::DpadRight => self.dpad_right = action,
        }
    }

    pub fn custom_count(&self) -> usize {
        RemappableButton::SOURCES
            .iter()
            .filter(|&&s| !self.get(s).is_none())
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RemappableButton {
    #[default]
    None,
    Cross,
    Circle,
    Square,
    Triangle,
    L1,
    R1,
    L2,
    R2,
    L3,
    R3,
    Options,
    Create,
    Touchpad,
    MicMute,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

impl RemappableButton {
    pub const SOURCES: [Self; 18] = [
        Self::Cross,
        Self::Circle,
        Self::Square,
        Self::Triangle,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
        Self::L3,
        Self::R3,
        Self::Options,
        Self::Create,
        Self::Touchpad,
        Self::MicMute,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
    ];

    #[allow(dead_code)]
    pub const ALL: [Self; 19] = [
        Self::None,
        Self::Cross,
        Self::Circle,
        Self::Square,
        Self::Triangle,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
        Self::L3,
        Self::R3,
        Self::Options,
        Self::Create,
        Self::Touchpad,
        Self::MicMute,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
    ];

    pub const TURBO_TARGETS: [Self; 14] = [
        Self::Cross,
        Self::Circle,
        Self::Square,
        Self::Triangle,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
        Self::L3,
        Self::R3,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Cross => "Cross",
            Self::Circle => "Circle",
            Self::Square => "Square",
            Self::Triangle => "Triangle",
            Self::L1 => "L1",
            Self::R1 => "R1",
            Self::L2 => "L2",
            Self::R2 => "R2",
            Self::L3 => "L3",
            Self::R3 => "R3",
            Self::Options => "Options",
            Self::Create => "Create",
            Self::Touchpad => "Touchpad",
            Self::MicMute => "Mic Mute",
            Self::DpadUp => "D-Up",
            Self::DpadDown => "D-Down",
            Self::DpadLeft => "D-Left",
            Self::DpadRight => "D-Right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonMap {
    pub cross: RemappableButton,
    pub circle: RemappableButton,
    pub square: RemappableButton,
    pub triangle: RemappableButton,
    pub l1: RemappableButton,
    pub r1: RemappableButton,
    pub l2: RemappableButton,
    pub r2: RemappableButton,
    pub l3: RemappableButton,
    pub r3: RemappableButton,
    pub options: RemappableButton,
    pub create: RemappableButton,
    pub touchpad: RemappableButton,
    pub mic_mute: RemappableButton,
    pub dpad_up: RemappableButton,
    pub dpad_down: RemappableButton,
    pub dpad_left: RemappableButton,
    pub dpad_right: RemappableButton,
}

impl Default for ButtonMap {
    fn default() -> Self {
        Self {
            cross: RemappableButton::Cross,
            circle: RemappableButton::Circle,
            square: RemappableButton::Square,
            triangle: RemappableButton::Triangle,
            l1: RemappableButton::L1,
            r1: RemappableButton::R1,
            l2: RemappableButton::L2,
            r2: RemappableButton::R2,
            l3: RemappableButton::L3,
            r3: RemappableButton::R3,
            options: RemappableButton::Options,
            create: RemappableButton::Create,
            touchpad: RemappableButton::Touchpad,
            mic_mute: RemappableButton::MicMute,
            dpad_up: RemappableButton::DpadUp,
            dpad_down: RemappableButton::DpadDown,
            dpad_left: RemappableButton::DpadLeft,
            dpad_right: RemappableButton::DpadRight,
        }
    }
}

impl ButtonMap {
    #[inline]
    pub fn get(&self, src: RemappableButton) -> RemappableButton {
        match src {
            RemappableButton::None => RemappableButton::None,
            RemappableButton::Cross => self.cross,
            RemappableButton::Circle => self.circle,
            RemappableButton::Square => self.square,
            RemappableButton::Triangle => self.triangle,
            RemappableButton::L1 => self.l1,
            RemappableButton::R1 => self.r1,
            RemappableButton::L2 => self.l2,
            RemappableButton::R2 => self.r2,
            RemappableButton::L3 => self.l3,
            RemappableButton::R3 => self.r3,
            RemappableButton::Options => self.options,
            RemappableButton::Create => self.create,
            RemappableButton::Touchpad => self.touchpad,
            RemappableButton::MicMute => self.mic_mute,
            RemappableButton::DpadUp => self.dpad_up,
            RemappableButton::DpadDown => self.dpad_down,
            RemappableButton::DpadLeft => self.dpad_left,
            RemappableButton::DpadRight => self.dpad_right,
        }
    }

    #[inline]
    pub fn set(&mut self, src: RemappableButton, dst: RemappableButton) {
        match src {
            RemappableButton::None => {}
            RemappableButton::Cross => self.cross = dst,
            RemappableButton::Circle => self.circle = dst,
            RemappableButton::Square => self.square = dst,
            RemappableButton::Triangle => self.triangle = dst,
            RemappableButton::L1 => self.l1 = dst,
            RemappableButton::R1 => self.r1 = dst,
            RemappableButton::L2 => self.l2 = dst,
            RemappableButton::R2 => self.r2 = dst,
            RemappableButton::L3 => self.l3 = dst,
            RemappableButton::R3 => self.r3 = dst,
            RemappableButton::Options => self.options = dst,
            RemappableButton::Create => self.create = dst,
            RemappableButton::Touchpad => self.touchpad = dst,
            RemappableButton::MicMute => self.mic_mute = dst,
            RemappableButton::DpadUp => self.dpad_up = dst,
            RemappableButton::DpadDown => self.dpad_down = dst,
            RemappableButton::DpadLeft => self.dpad_left = dst,
            RemappableButton::DpadRight => self.dpad_right = dst,
        }
    }

    pub fn swap(&mut self, a: RemappableButton, b: RemappableButton) {
        if a == RemappableButton::None || b == RemappableButton::None || a == b {
            return;
        }
        let va = self.get(a);
        let vb = self.get(b);
        self.set(a, vb);
        self.set(b, va);
    }

    pub fn custom_count(&self) -> usize {
        RemappableButton::SOURCES
            .iter()
            .filter(|&&s| self.get(s) != s)
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TurboConfig {
    pub enabled: bool,
    pub target: RemappableButton,
    #[serde(default = "default_turbo_rate")]
    pub rate_hz: u8,
}

impl Default for TurboConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target: RemappableButton::Cross,
            rate_hz: 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GyroAimMode {
    #[default]
    Off,
    HoldL2,
    HoldL1,
    HoldR2,
    HoldR1,
    TouchpadTouch,
    AlwaysOn,
}

impl GyroAimMode {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::HoldL2,
        Self::HoldL1,
        Self::HoldR2,
        Self::HoldR1,
        Self::TouchpadTouch,
        Self::AlwaysOn,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::HoldL2 => "Hold L2",
            Self::HoldL1 => "Hold L1",
            Self::HoldR2 => "Hold R2",
            Self::HoldR1 => "Hold R1",
            Self::TouchpadTouch => "Touchpad",
            Self::AlwaysOn => "Always On",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GyroOutputMode {
    #[default]
    Mouse,
    RightStick,
}

impl GyroOutputMode {
    pub const ALL: [Self; 2] = [Self::Mouse, Self::RightStick];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mouse => "Mouse",
            Self::RightStick => "Right Stick",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerProfile {
    pub emulation: EmulationMode,
    pub override_lightbar: bool,
    pub lightbar_rgb: [u8; 3],
    pub lightbar_mode: LightbarMode,
    #[serde(default = "default_one")]
    pub lightbar_brightness: f32,
    #[serde(default = "default_one")]
    pub lightbar_speed: f32,
    pub player_leds: u8,
    pub player_led_brightness: LedBrightness,
    pub mic_mute_led: MicMuteLedOverride,
    #[serde(default = "default_true")]
    pub low_battery_alert: bool,
    #[serde(default = "default_low_battery_threshold")]
    pub low_battery_threshold: u8,
    pub idle_dim_secs: u16,
    pub enable_rumble: bool,
    #[serde(default = "default_one")]
    pub rumble_strength: f32,
    pub override_triggers: bool,
    pub left_trigger_preset: TriggerPreset,
    pub right_trigger_preset: TriggerPreset,
    pub left_custom_trigger: CustomTriggerConfig,
    pub right_custom_trigger: CustomTriggerConfig,
    #[serde(default = "default_true")]
    pub link_triggers: bool,
    pub left_stick: StickTuning,
    pub right_stick: StickTuning,
    pub swap_sticks: bool,
    pub triggers: TriggerTuning,
    pub right_trigger_tuning: TriggerTuning,
    pub button_map: ButtonMap,
    #[serde(default)]
    pub action_map: ActionMap,
    #[serde(default)]
    pub mic_button_controls_windows_mic: bool,
    pub touchpad_mouse: bool,
    pub touchpad_sensitivity: f32,
    #[serde(default = "default_true")]
    pub touchpad_scroll: bool,
    pub touchpad_tap_to_click: bool,
    #[serde(default = "default_true")]
    pub touchpad_split_click: bool,
    pub gyro_mode: GyroAimMode,
    pub gyro_output: GyroOutputMode,
    pub gyro_sensitivity: f32,
    #[serde(default = "default_gyro_deadzone")]
    pub gyro_deadzone: f32,
    #[serde(default = "default_true")]
    pub gyro_use_yaw: bool,
    pub gyro_use_roll: bool,
    pub gyro_invert_x: bool,
    pub gyro_invert_y: bool,
    #[serde(default)]
    pub flick_stick_enabled: bool,
    #[serde(default = "default_flick_pixels_per_turn")]
    pub flick_stick_pixels_per_turn: f32,
    #[serde(default = "default_flick_threshold")]
    pub flick_stick_threshold: f32,
    #[serde(default = "default_flick_ms")]
    pub flick_stick_flick_ms: u16,
    pub turbo: TurboConfig,
    pub bt_speaker_enabled: bool,
    #[serde(default = "default_bt_speaker_volume")]
    pub bt_speaker_volume: f32,
    pub bt_audio_route: BtAudioRoute,
    pub bt_haptics_enabled: bool,
    #[serde(default = "default_bt_haptics_gain")]
    pub bt_haptics_gain: f32,
    pub virtual_audio_device: bool,
    #[serde(default)]
    pub virtual_speaker_device: bool,
    #[serde(default)]
    pub virtual_mic_device: bool,
    #[serde(default = "default_true")]
    pub bt_mic_enabled: bool,
    #[serde(default = "default_one")]
    pub bt_mic_gain: f32,
    #[serde(default)]
    pub bt_mic_route: BtMicRoute,
}

impl Default for ControllerProfile {
    fn default() -> Self {
        Self {
            emulation: EmulationMode::Xbox360,
            override_lightbar: false,
            lightbar_rgb: [0, 96, 255],
            lightbar_mode: LightbarMode::Static,
            lightbar_brightness: 1.0,
            lightbar_speed: 1.0,
            player_leds: 0x04,
            player_led_brightness: LedBrightness::High,
            mic_mute_led: MicMuteLedOverride::Native,
            low_battery_alert: true,
            low_battery_threshold: 20,
            idle_dim_secs: 0,
            enable_rumble: true,
            rumble_strength: 1.0,
            override_triggers: false,
            left_trigger_preset: TriggerPreset::Off,
            right_trigger_preset: TriggerPreset::Off,
            left_custom_trigger: CustomTriggerConfig::default(),
            right_custom_trigger: CustomTriggerConfig::default(),
            link_triggers: true,
            left_stick: StickTuning::default(),
            right_stick: StickTuning::default(),
            swap_sticks: false,
            triggers: TriggerTuning::default(),
            right_trigger_tuning: TriggerTuning::default(),
            button_map: ButtonMap::default(),
            action_map: ActionMap::default(),
            mic_button_controls_windows_mic: false,
            touchpad_mouse: false,
            touchpad_sensitivity: 1.0,
            touchpad_scroll: true,
            touchpad_tap_to_click: false,
            touchpad_split_click: true,
            gyro_mode: GyroAimMode::Off,
            gyro_output: GyroOutputMode::Mouse,
            gyro_sensitivity: 1.0,
            gyro_deadzone: 1.0,
            gyro_use_yaw: true,
            gyro_use_roll: false,
            gyro_invert_x: false,
            gyro_invert_y: false,
            flick_stick_enabled: false,
            flick_stick_pixels_per_turn: default_flick_pixels_per_turn(),
            flick_stick_threshold: default_flick_threshold(),
            flick_stick_flick_ms: default_flick_ms(),
            turbo: TurboConfig::default(),
            bt_speaker_enabled: false,
            bt_speaker_volume: 0.8,
            bt_audio_route: BtAudioRoute::Auto,
            bt_haptics_enabled: false,
            bt_haptics_gain: 0.75,
            virtual_audio_device: false,
            virtual_speaker_device: false,
            virtual_mic_device: false,
            bt_mic_enabled: true,
            bt_mic_gain: 1.0,
            bt_mic_route: BtMicRoute::Auto,
        }
    }
}

impl ControllerProfile {
    #[inline]
    pub fn virtual_audio_any(&self) -> bool {
        self.virtual_speaker_device || self.virtual_mic_device
    }

    #[inline]
    pub fn left_trigger_tuning(&self) -> TriggerTuning {
        self.triggers
    }

    #[inline]
    pub fn right_trigger_tuning(&self) -> TriggerTuning {
        if self.link_triggers {
            self.triggers
        } else {
            self.right_trigger_tuning
        }
    }

    #[inline]
    pub fn left_trigger_effect(&self) -> AdaptiveTriggerEffect {
        self.left_trigger_preset
            .effect_with_custom(&self.left_custom_trigger)
    }

    #[inline]
    pub fn right_trigger_effect(&self) -> AdaptiveTriggerEffect {
        self.right_trigger_preset
            .effect_with_custom(&self.right_custom_trigger)
    }

    #[inline]
    pub fn set_custom_lightbar_rgb(&mut self, rgb: [u8; 3]) {
        self.override_lightbar = true;
        if !self.lightbar_mode.uses_custom_color() {
            self.lightbar_mode = LightbarMode::Static;
        }
        self.lightbar_rgb = rgb;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_lightbar_rgb_switches_away_from_color_ignoring_modes() {
        let mut prof = ControllerProfile {
            override_lightbar: false,
            lightbar_mode: LightbarMode::BatteryIndicator,
            lightbar_rgb: [0, 96, 255],
            ..Default::default()
        };
        prof.set_custom_lightbar_rgb([235, 56, 56]);
        assert!(prof.override_lightbar);
        assert_eq!(prof.lightbar_mode, LightbarMode::Static);
        assert_eq!(prof.lightbar_rgb, [235, 56, 56]);

        prof.lightbar_mode = LightbarMode::Rainbow;
        prof.set_custom_lightbar_rgb([32, 200, 96]);
        assert_eq!(prof.lightbar_mode, LightbarMode::Static);
        assert_eq!(prof.lightbar_rgb, [32, 200, 96]);

        prof.lightbar_mode = LightbarMode::Breathing;
        prof.set_custom_lightbar_rgb([148, 86, 245]);
        assert_eq!(prof.lightbar_mode, LightbarMode::Breathing);
        assert_eq!(prof.lightbar_rgb, [148, 86, 245]);

        prof.lightbar_mode = LightbarMode::TriggerReactive;
        prof.set_custom_lightbar_rgb([242, 208, 107]);
        assert_eq!(prof.lightbar_mode, LightbarMode::TriggerReactive);
        assert_eq!(prof.lightbar_rgb, [242, 208, 107]);
    }
}
