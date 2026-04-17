use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct PinStore {
    pins: Vec<String>,
}

pub fn load_pins() -> Vec<String> {
    let Some(path) = config_path() else {
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
    let Some(path) = config_path() else {
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

fn config_path() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join("rudo").join("pins.json"))
}
