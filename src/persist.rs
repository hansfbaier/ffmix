//! Small JSON store in `$XDG_CONFIG_HOME/ffmix` (or `~/.config/ffmix`).

use std::fs;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ff800::DeviceSettings;
use crate::model::{
    Mix, MixState, MixStateV1, PresetBank, PresetBankV1, N_OUT, N_PAIRS, N_SLOTS,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 0 = mixer, 1 = matrix, 2 = settings.
    pub tab: u8,
    /// 0 = inputs -> outputs, 1 = playback -> outputs.
    pub matrix_kind: u8,
    pub close_to_tray: bool,
    /// Omitting the field (older configs) falls back to `true`: the app is a tray
    /// app and starts in the tray when a StatusNotifier host is available.
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
            start_hidden: true,
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

/// Version 1 stored one send per output pair, version 2 one per output channel.
/// The width of the first `sends_in` row tells the shapes apart.
fn sends_width(value: &serde_json::Value) -> Option<usize> {
    value
        .get("sends_in")?
        .as_array()?
        .first()?
        .as_array()
        .map(Vec::len)
}

fn load_text(name: &str) -> Option<String> {
    fs::read_to_string(path(name)).ok()
}

fn read_json(name: &str) -> Option<serde_json::Value> {
    let text = load_text(name)?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!(
                "ffmix: ignoring malformed {}: {err}",
                path(name).display()
            );
            None
        }
    }
}

fn load_json<T: DeserializeOwned>(name: &str, value: serde_json::Value) -> Option<T> {
    match serde_json::from_value(value) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            eprintln!(
                "ffmix: ignoring malformed {}: {err}",
                path(name).display()
            );
            None
        }
    }
}

pub fn load_mix() -> MixState {
    let Some(value) = read_json("mix.json") else {
        return MixState::default();
    };
    let mut state = if sends_width(&value) == Some(N_OUT) {
        load_json("mix.json", value).unwrap_or_default()
    } else {
        load_json::<MixStateV1>("mix.json", value)
            .map(MixState::from_v1)
            .unwrap_or_default()
    };
    // Older layouts stored L->R and R->L sends for linked pairs.
    state.clear_cross_connections();
    state
}

pub fn save_mix(st: &MixState) {
    save("mix.json", st);
}

pub fn load_presets() -> PresetBank {
    let Some(value) = read_json("presets.json") else {
        return PresetBank::default();
    };
    let paired = value
        .get("slots")
        .and_then(|slots| slots.as_array())
        .and_then(|slots| slots.iter().filter_map(sends_width).next())
        == Some(N_PAIRS);
    if !paired {
        let mut bank: PresetBank = load_json("presets.json", value).unwrap_or_default();
        for preset in bank.slots.iter_mut().flatten() {
            preset.clear_cross_connections();
        }
        return bank;
    }
    let Some(old) = load_json::<PresetBankV1>("presets.json", value) else {
        return PresetBank::default();
    };
    let mut slots: [Option<Box<Mix>>; N_SLOTS] = Default::default();
    for (slot, preset) in old.slots.into_iter().enumerate() {
        slots[slot] = preset.map(|preset| Box::new(Mix::from_v1(*preset)));
    }
    PresetBank { slots }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MixV1, Send, N_IN};

    #[test]
    fn send_width_tells_the_shapes_apart() {
        let v1 = serde_json::to_value(MixStateV1::default()).unwrap();
        assert_eq!(sends_width(&v1), Some(N_PAIRS));
        let v2 = serde_json::to_value(MixState::default()).unwrap();
        assert_eq!(sends_width(&v2), Some(N_OUT));
    }

    #[test]
    fn v1_presets_migrate_to_channels() {
        let mut old = PresetBankV1::default();
        old.slots[0] = Some(Box::new(MixV1 {
            sends_in: {
                let mut sends = [[Send::default(); N_PAIRS]; N_IN];
                sends[0][0] = Send { db: Some(-6.0), pan: 0.5 };
                sends
            },
            ..Default::default()
        }));
        let value = serde_json::to_value(&old).unwrap();
        let paired = value
            .get("slots")
            .and_then(|slots| slots.as_array())
            .and_then(|slots| slots.iter().filter_map(sends_width).next())
            == Some(N_PAIRS);
        assert!(paired);
        let bank = load_json::<PresetBankV1>("presets.json", value).unwrap();
        let migrated = bank.slots[0].as_ref().map(|p| Mix::from_v1(*p.clone()));
        let mix = migrated.expect("slot 0");
        assert_eq!(mix.sends_in[0][0], mix.sends_in[0][1]);
        assert_eq!(mix.sends_in[0][0].db, Some(-6.0));
    }
}



