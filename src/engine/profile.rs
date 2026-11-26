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
