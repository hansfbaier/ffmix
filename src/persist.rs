//! Small JSON store in `$XDG_CONFIG_HOME/ffmix` (or `~/.config/ffmix`).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ff800::DeviceSettings;
use crate::model::{MixState, PresetBank};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 0 = mixer, 1 = matrix, 2 = settings.
    pub tab: u8,
    /// 0 = inputs -> outputs, 1 = playback -> outputs.
    pub matrix_kind: u8,
    pub close_to_tray: bool,
    pub start_hidden: bool,
    /// Output pair used by the Dim button and the main-out readout.
    pub main_pair: usize,
    pub window_size: Option<[f32; 2]>,
    pub window_maximized: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            tab: 0,
            matrix_kind: 0,
            close_to_tray: true,
            start_hidden: false,
            main_pair: 0,
            window_size: None,
            window_maximized: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("ffmix");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config").join("ffmix")
}

fn path(name: &str) -> PathBuf {
    config_dir().join(name)
}

fn load<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    let p = path(name);
    match fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("ffmix: ignoring malformed {}: {err}", p.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

fn save<T: Serialize>(name: &str, value: &T) {
    let dir = config_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("ffmix: cannot create {}: {err}", dir.display());
        return;
    }
    let p = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => {
            if let Err(err) = fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, &p)) {
                eprintln!("ffmix: cannot write {}: {err}", p.display());
            }
        }
        Err(err) => eprintln!("ffmix: cannot serialise {name}: {err}"),
    }
}

pub fn load_mix() -> MixState {
    load("mix.json")
}

pub fn save_mix(st: &MixState) {
    save("mix.json", st);
}

pub fn load_presets() -> PresetBank {
    load("presets.json")
}

pub fn save_presets(presets: &PresetBank) {
    save("presets.json", presets);
}

pub fn load_settings() -> DeviceSettings {
    load("settings.json")
}

pub fn save_settings(settings: &DeviceSettings) {
    save("settings.json", settings);
}

pub fn load_config() -> AppConfig {
    load("config.json")
}

pub fn save_config(config: &AppConfig) {
    save("config.json", config);
}
