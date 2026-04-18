use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct PinStore {
    pins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub autohide: AutoHideSettings,
    pub show_pin_button: bool,
    pub icon_size: i32,
    pub position: String,
    pub animation_duration_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoHideSettings {
    pub enabled: bool,
    pub delay_secs: u64,
}

impl Default for AutoHideSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_secs: 3,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autohide: AutoHideSettings::default(),
            show_pin_button: true,
            icon_size: 24,
            position: "bottom".to_string(),
            animation_duration_ms: 220,
        }
    }
}

pub fn load_pins() -> Vec<String> {
    let Some(path) = pins_path() else {
        return Vec::new();
    };

    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };

    serde_json::from_str::<PinStore>(&contents)
        .map(|store| store.pins)
        .unwrap_or_default()
}

pub fn save_pins(pins: &[String]) {
    if let Err(error) = save_pins_inner(pins) {
        eprintln!("failed to save dock pins: {error}");
    }
}

fn save_pins_inner(pins: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(path) = pins_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_string_pretty(&PinStore {
        pins: pins.to_vec(),
    })?;
    fs::write(path, contents)?;
    Ok(())
}

pub fn ensure_style_css() {
    if let Err(error) = ensure_style_css_inner() {
        eprintln!("failed to prepare dock style config: {error}");
    }
}

pub fn ensure_settings() {
    if let Err(error) = ensure_settings_inner() {
        eprintln!("failed to prepare dock settings config: {error}");
    }
}

pub fn load_settings() -> Settings {
    let Some(path) = settings_path() else {
        return Settings::default();
    };

    let Ok(contents) = fs::read_to_string(path) else {
        return Settings::default();
    };

    serde_json::from_str::<Settings>(&contents).unwrap_or_default()
}

pub fn load_style_css() -> Option<String> {
    let path = style_path()?;
    fs::read_to_string(path).ok()
}

fn ensure_style_css_inner() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(path) = style_path() else {
        return Ok(());
    };

    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, DEFAULT_STYLE_CSS)?;
    Ok(())
}

fn ensure_settings_inner() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(path) = settings_path() else {
        return Ok(());
    };

    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_string_pretty(&Settings::default())?;
    fs::write(path, contents)?;
    Ok(())
}

pub fn config_dir() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join("rudo"))
}

pub fn pins_path() -> Option<PathBuf> {
    Some(config_dir()?.join("pins.json"))
}

pub fn style_path() -> Option<PathBuf> {
    Some(config_dir()?.join("style.css"))
}

pub fn settings_path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.json"))
}

const DEFAULT_STYLE_CSS: &str = r#"/* Rudo user overrides
 *
 * This file is loaded on every Rudo start after the built-in theme.
 * Override any selector you want here.
 *
 * Examples:
 *
 * .dock-surface {
 *     border-radius: 22px;
 *     background: rgba(12, 16, 24, 0.94);
 * }
 *
 * .dock-item.is-active {
 *     border-color: rgba(120, 210, 255, 0.55);
 * }
 */
"#;
