use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("config directory not found")]
    ConfigDirNotFound,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PinStore {
    pins: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    #[default]
    #[serde(rename = "bottom")]
    Bottom,
    #[serde(rename = "top")]
    Top,
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub autohide: AutoHideSettings,
    pub show_pin_button: bool,
    pub icon_size: i32,
    pub position: Position,
    pub animation_duration_ms: u32,
    pub menu: MenuConfig,
    pub group_by_output: bool,
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
            position: Position::default(),
            animation_duration_ms: 220,
            menu: MenuConfig::default(),
            group_by_output: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuConfig {
    pub enabled: bool,
    pub icon: String,
    pub position: MenuPosition,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuPosition {
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "end")]
    End,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            icon: "system-lock-screen-symbolic".to_string(),
            position: MenuPosition::End,
            items: default_menu_items(),
        }
    }
}

fn default_menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem {
            label: "Lock".to_string(),
            icon: Some("system-lock-screen-symbolic".to_string()),
            command: "loginctl lock-session".to_string(),
            confirm: false,
        },
        MenuItem {
            label: "Logout".to_string(),
            icon: Some("system-log-out-symbolic".to_string()),
            command: "loginctl terminate-user $USER".to_string(),
            confirm: true,
        },
        MenuItem {
            label: "Restart".to_string(),
            icon: Some("system-restart-symbolic".to_string()),
            command: "systemctl reboot".to_string(),
            confirm: true,
        },
        MenuItem {
            label: "Shutdown".to_string(),
            icon: Some("system-shutdown-symbolic".to_string()),
            command: "systemctl poweroff".to_string(),
            confirm: true,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    pub label: String,
    pub icon: Option<String>,
    pub command: String,
    pub confirm: bool,
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

fn save_pins_inner(pins: &[String]) -> Result<(), ConfigError> {
    let Some(path) = pins_path() else {
        return Err(ConfigError::ConfigDirNotFound);
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

fn ensure_file_with_content(
    path: Option<PathBuf>,
    content: impl AsRef<[u8]>,
) -> Result<(), ConfigError> {
    let Some(path) = path else {
        return Err(ConfigError::ConfigDirNotFound);
    };

    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, content)?;
    Ok(())
}

fn ensure_style_css_inner() -> Result<(), ConfigError> {
    ensure_file_with_content(style_path(), DEFAULT_STYLE_CSS)
}

fn ensure_settings_inner() -> Result<(), ConfigError> {
    ensure_file_with_content(
        settings_path(),
        serde_json::to_string_pretty(&Settings::default())?,
    )
}

pub fn config_dir() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join("rudo"))
}

pub fn pins_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("pins.json"))
}

pub fn style_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("style.css"))
}

pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("settings.json"))
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
