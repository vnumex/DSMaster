use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::profile::ControllerProfile;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedProfile {
    pub name: String,
    pub linked_exe: Option<String>,
    pub settings: ControllerProfile,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub active_profile: String,
    pub cloak_physical: bool,
    pub auto_switch_profiles: bool,
    #[serde(default = "default_true")]
    pub work_in_background: bool,
    pub start_minimized: bool,
    pub profiles: Vec<NamedProfile>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_profile: "Default".to_owned(),
            cloak_physical: false,
            auto_switch_profiles: true,
            work_in_background: true,
            start_minimized: false,
            profiles: vec![NamedProfile {
                name: "Default".to_owned(),
                linked_exe: None,
                settings: ControllerProfile::default(),
            }],
        }
    }
}

impl AppConfig {
    pub fn active_settings(&self) -> ControllerProfile {
        self.profiles
            .iter()
            .find(|p| p.name == self.active_profile)
            .or_else(|| self.profiles.first())
            .map(|p| p.settings)
            .unwrap_or_default()
    }

    pub fn active_settings_mut(&mut self) -> &mut ControllerProfile {
        if let Some(idx) = self
            .profiles
            .iter()
            .position(|p| p.name == self.active_profile)
        {
            return &mut self.profiles[idx].settings;
        }
        if self.profiles.is_empty() {
            self.profiles.push(NamedProfile {
                name: "Default".to_owned(),
                linked_exe: None,
                settings: ControllerProfile::default(),
            });
            self.active_profile = "Default".to_owned();
        }
        &mut self.profiles[0].settings
    }
}

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("DSMaster").join("profiles.toml"))
}

pub fn open_config_dir() -> Result<(), String> {
    let Some(path) = config_path() else {
        return Err("APPDATA environment variable not set".to_owned());
    };
    let Some(dir) = path.parent() else {
        return Err("Invalid config directory".to_owned());
    };
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return AppConfig::default();
    };
    let mut cfg: AppConfig = toml::from_str(&raw).unwrap_or_default();
    if cfg.profiles.is_empty() {
        cfg.profiles.push(NamedProfile {
            name: "Default".to_owned(),
            linked_exe: None,
            settings: ControllerProfile::default(),
        });
    }
    if !cfg.profiles.iter().any(|p| p.name == cfg.active_profile) {
        cfg.active_profile = cfg.profiles[0].name.clone();
    }
    cfg
}

pub fn save_config(cfg: &AppConfig) -> Result<(), String> {
    let Some(path) = config_path() else {
        return Err("APPDATA environment variable not set".to_owned());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emu::EmulationMode;
    use crate::engine::profile::{ButtonPreset, TriggerPreset};

    #[test]
    fn toml_config_round_trip() {
        let mut cfg = AppConfig {
            cloak_physical: true,
            ..Default::default()
        };
        cfg.profiles.push(NamedProfile {
            name: "FPS".to_owned(),
            linked_exe: Some("apex.exe".to_owned()),
            settings: ControllerProfile {
                emulation: EmulationMode::Xbox360,
                right_trigger_preset: TriggerPreset::Weapon,
                button_preset: ButtonPreset::Tactical,
                ..Default::default()
            },
        });
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let parsed: AppConfig = toml::from_str(&raw).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn legacy_toml_defaults_background_enabled() {
        let raw = r#"
active_profile = "Default"
cloak_physical = false
auto_switch_profiles = true
"#;
        let parsed: AppConfig = toml::from_str(raw).unwrap();
        assert!(parsed.work_in_background);
        assert!(!parsed.start_minimized);
    }

    #[test]
    fn default_profile_leaves_hardware_untouched() {
        let prof = ControllerProfile::default();
        assert!(!prof.override_triggers);
        assert!(!prof.override_lightbar);
    }
}
