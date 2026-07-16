use std::fs;
use std::io::ErrorKind;
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
    pub outputs: OutputMode,
    pub animation_duration_ms: u32,
    pub menu: MenuConfig,
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
            outputs: OutputMode::default(),
            animation_duration_ms: 220,
            menu: MenuConfig::default(),
        }
    }
}

impl Settings {
    fn normalized(mut self) -> Self {
        self.icon_size = self.icon_size.clamp(8, 256);
        self.animation_duration_ms = self.animation_duration_ms.min(10_000);
        self.autohide.delay_secs = self.autohide.delay_secs.clamp(1, 86_400);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputMode {
    #[default]
    #[serde(rename = "first")]
    First,
    #[serde(rename = "all")]
    All,
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
    [
        (
            "Lock",
            "system-lock-screen-symbolic",
            "swaylock -f",
            // "loginctl lock-session",
            false,
        ),
        (
            "Logout",
            "system-log-out-symbolic",
            "loginctl terminate-user $USER",
            true,
        ),
        (
            "Restart",
            "system-restart-symbolic",
            "systemctl reboot",
            true,
        ),
        (
            "Shutdown",
            "system-shutdown-symbolic",
            "systemctl poweroff",
            true,
        ),
    ]
    .map(|(label, icon, command, confirm)| MenuItem {
        label: label.into(),
        icon: Some(icon.into()),
        command: command.into(),
        confirm,
    })
    .to_vec()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    pub label: String,
    pub icon: Option<String>,
    pub command: String,
    pub confirm: bool,
}

pub fn load_pins() -> Result<Vec<String>, ConfigError> {
    let Some(path) = pins_path() else {
        return Err(ConfigError::ConfigDirNotFound);
    };

    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    Ok(serde_json::from_str::<PinStore>(&contents)?.pins)
}

pub fn save_pins(pins: &[String]) -> Result<(), ConfigError> {
    let Some(path) = pins_path() else {
        return Err(ConfigError::ConfigDirNotFound);
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_string_pretty(&PinStore {
        pins: pins.to_vec(),
    })?;
    atomic_write(&path, contents)
}

pub fn ensure_style_css() -> Result<(), ConfigError> {
    ensure_file_with_content(style_path(), DEFAULT_STYLE_CSS)
}

pub fn ensure_settings() -> Result<(), ConfigError> {
    ensure_file_with_content(
        settings_path(),
        serde_json::to_string_pretty(&Settings::default())?,
    )
}

pub fn load_settings() -> Result<Settings, ConfigError> {
    let Some(path) = settings_path() else {
        return Err(ConfigError::ConfigDirNotFound);
    };

    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(error.into()),
    };

    Ok(serde_json::from_str::<Settings>(&contents)?.normalized())
}

pub fn load_style_css() -> Result<Option<String>, ConfigError> {
    let Some(path) = style_path() else {
        return Err(ConfigError::ConfigDirNotFound);
    };
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
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

    atomic_write(&path, content)
}

fn atomic_write(path: &std::path::Path, content: impl AsRef<[u8]>) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rudo-config");
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    fs::write(&temporary, content)?;

    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }

    Ok(())
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

const DEFAULT_STYLE_CSS: &str = r"/* Rudo user overrides
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
";

#[cfg(test)]
mod tests {
    use super::{OutputMode, PinStore, Settings, atomic_write};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

    fn test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rudo-config-test-{}-{}",
            std::process::id(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn invalid_pin_json_is_an_error_instead_of_an_empty_store() {
        assert!(serde_json::from_str::<PinStore>("{not json}").is_err());
    }

    #[test]
    fn settings_support_all_outputs_and_normalize_extreme_values() {
        let settings: Settings = serde_json::from_str(
            r#"{"outputs":"all","icon_size":-5,"animation_duration_ms":999999}"#,
        )
        .unwrap();
        let settings = settings.normalized();

        assert_eq!(settings.outputs, OutputMode::All);
        assert_eq!(settings.icon_size, 8);
        assert_eq!(settings.animation_duration_ms, 10_000);
    }

    #[test]
    fn atomic_write_replaces_existing_contents() {
        let directory = test_dir();
        let path = directory.join("pins.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, "old").unwrap();

        atomic_write(&path, "new").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(directory).unwrap();
    }
}
