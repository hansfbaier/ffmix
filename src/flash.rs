//! Read-only decoding of the FF800's saved flash state.
//!
//! The device stores its power-on configuration and mixer snapshot in a
//! memory-mapped flash. The layout and scaling are documented in FFADO's RME
//! register notes; this module implements the decoding independently.

use crate::ff800::{ClockSource, DeviceSettings, InLevel, Jack, OutLevel};
use crate::model::{lin2db, OutPair, Send, N_IN, N_PAIRS, N_PLAY};

pub const SETTINGS_ADDR: u64 = 0x3000_f0000;
pub const SETTINGS_WORDS: usize = 59;
pub const SETTINGS_BYTES: usize = SETTINGS_WORDS * 4;
pub const MIXER_VOLUME_ADDR: u64 = 0x3000_e2000;
pub const MIXER_PAN_ADDR: u64 = 0x3000_e2800;
pub const MIXER_HW_ADDR: u64 = 0x3000_e3000;
pub const MIXER_ARRAY_BYTES: usize = 0x0800;
pub const MIXER_HW_BYTES: usize = 0x0100;

const FLASH_ROW: usize = 32;
const FLASH_0DB_VOL: u16 = 0x0323;
const FADER_MAX: f32 = 0x1_0000 as f32;
const FADER_0DB: f32 = 0x8000 as f32;

#[derive(Clone, Debug, Default)]
pub struct FlashMixer {
    pub sends_in: [[Send; N_PAIRS]; N_IN],
    pub sends_play: [[Send; N_PAIRS]; N_PLAY],
    pub out: [OutPair; N_PAIRS],
}

fn word(raw: &[u8], index: usize) -> u32 {
    let off = index * 4;
    u32::from_le_bytes([
        raw[off],
        raw[off + 1],
        raw[off + 2],
        raw[off + 3],
    ])
}

fn half(raw: &[u8], index: usize) -> u16 {
    let off = index * 2;
    u16::from_le_bytes([raw[off], raw[off + 1]])
}

fn jack(value: u32) -> Jack {
    match value {
        0 => Jack::Rear,
        1 => Jack::Front,
        _ => Jack::Both,
    }
}

fn clock_source(mode: u32, sync_ref: u32) -> Result<ClockSource, String> {
    match mode {
        0 => Ok(ClockSource::Internal),
        1 => match sync_ref {
            0 => Ok(ClockSource::Adat1),
            1 => Ok(ClockSource::Adat2),
            2 => Ok(ClockSource::Spdif),
            3 => Ok(ClockSource::Word),
            other => Err(format!("flash clock reference {other} is not supported")),
        },
        other => Err(format!("flash clock mode {other} is not supported")),
    }
}

pub fn decode_settings(raw: &[u8]) -> Result<DeviceSettings, String> {
    if raw.len() < SETTINGS_BYTES {
        return Err(format!(
            "short flash settings block: {} bytes",
            raw.len()
        ));
    }

    let mut settings = DeviceSettings::default();
    settings.known = true;
    settings.clock = clock_source(word(raw, 7), word(raw, 9))?;
    settings.spdif_in_optical = word(raw, 4) == 0;
    settings.opt_out_spdif = word(raw, 10) == 1;
    settings.spdif_pro = word(raw, 6) != 0;
    settings.emphasis = word(raw, 5) != 0;
    settings.non_audio = word(raw, 8) != 0;
    settings.word_single = word(raw, 46) != 0;
    settings.in_level = match word(raw, 29) {
        0 => InLevel::Low,
        2 => InLevel::Pro,
        1 => InLevel::Con,
        other => return Err(format!("flash input level {other} is not supported")),
    };
    settings.out_level = match word(raw, 30) {
        0 => OutLevel::Con,
        1 => OutLevel::Pro,
        2 => OutLevel::High,
        other => return Err(format!("flash output level {other} is not supported")),
    };
    settings.jacks = [
        jack(word(raw, 37)),
        jack(word(raw, 31)),
        jack(word(raw, 32)),
    ];
    settings.p48 = [
        word(raw, 33) != 0,
        word(raw, 34) != 0,
        word(raw, 35) != 0,
        word(raw, 36) != 0,
    ];
    settings.drive = word(raw, 39) != 0;
    settings.speaker_emu = word(raw, 38) != 0;
    settings.limiter = word(raw, 49) == 0;

    Ok(settings)
}

/// Convert the 10-bit flash volume code to the 16-bit fader scale used by the
/// matrix. 0x0323 is the explicit 0 dB code; 0x10000 would be +6 dB.
fn flash_vol_to_fader(value: u16) -> f32 {
    if value == 0 {
        0.0
    } else if value == FLASH_0DB_VOL {
        FADER_0DB
    } else {
        let curve = (3.0f32 * value as f32 / 1023.0).exp() - 1.0;
        FADER_MAX * curve / (3.0f32.exp() - 1.0)
    }
}

/// The flash stores independent left/right faders per source and output pair.
/// FFMix's balance law has one gain and one pan; max gain plus differential pan
/// reproduces those two gains exactly.
fn flash_send(volume: u16, pan: u16) -> Send {
    let fader = flash_vol_to_fader(volume);
    if fader <= 0.0 {
        return Send::default();
    }
    let pan = pan.min(256) as f32 / 256.0;
    let left = fader * (1.0 - pan) / FADER_0DB;
    let right = fader * pan / FADER_0DB;
    let gain = left.max(right);
    if gain <= 0.0 {
        return Send::default();
    }
    Send {
        db: lin2db(gain),
        pan: ((right - left) / gain).clamp(-1.0, 1.0),
    }
}

pub fn decode_mixer(volume: &[u8], pan: &[u8], hw: &[u8]) -> Result<FlashMixer, String> {
    if volume.len() < MIXER_ARRAY_BYTES
        || pan.len() < MIXER_ARRAY_BYTES
        || hw.len() < 2 * N_PAIRS
    {
        return Err("short flash mixer block".to_owned());
    }

    let mut mixer = FlashMixer::default();
    for op in 0..N_PAIRS {
        let input_base = op * 2 * FLASH_ROW;
        let playback_base = (op * 2 + 1) * FLASH_ROW;
        for c in 0..N_IN {
            let i = input_base + c;
            mixer.sends_in[c][op] = flash_send(half(volume, i), half(pan, i));
        }
        for c in 0..N_PLAY {
            let i = playback_base + c;
            mixer.sends_play[c][op] = flash_send(half(volume, i), half(pan, i));
        }

        let left = flash_vol_to_fader(half(hw, 2 * op));
        let right = flash_vol_to_fader(half(hw, 2 * op + 1));
        let gain = left.max(right) / FADER_0DB;
        mixer.out[op] = OutPair {
            gain: lin2db(gain),
            mute: false,
        };
    }

    Ok(mixer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_word(raw: &mut [u8], index: usize, value: u32) {
        raw[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_half(raw: &mut [u8], index: usize, value: u16) {
        raw[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn settings_decode_maps_clock_levels_and_jacks() {
        let mut raw = [0u8; SETTINGS_BYTES];
        put_word(&mut raw, 4, 0); // S/PDIF input optical
        put_word(&mut raw, 5, 1); // emphasis
        put_word(&mut raw, 6, 1); // professional
        put_word(&mut raw, 7, 1); // slave
        put_word(&mut raw, 8, 1); // non-audio
        put_word(&mut raw, 9, 3); // word clock
        put_word(&mut raw, 10, 1); // optical S/PDIF output
        put_word(&mut raw, 29, 2); // +4 dBu
        put_word(&mut raw, 30, 0); // -10 dBV
        put_word(&mut raw, 31, 1); // AN 7 front
        put_word(&mut raw, 32, 0); // AN 8 rear
        put_word(&mut raw, 33, 1); // 48 V AN 7
        put_word(&mut raw, 35, 1); // 48 V AN 9
        put_word(&mut raw, 37, 2); // AN 1 front + rear
        put_word(&mut raw, 38, 1); // speaker emulation
        put_word(&mut raw, 39, 1); // drive
        put_word(&mut raw, 46, 1); // word clock single speed
        put_word(&mut raw, 49, 0); // limiter enabled

        let s = decode_settings(&raw).expect("decode");
        assert_eq!(s.clock, ClockSource::Word);
        assert!(s.spdif_in_optical);
        assert!(s.opt_out_spdif);
        assert!(s.spdif_pro && s.emphasis && s.non_audio);
        assert_eq!(s.in_level, InLevel::Pro);
        assert_eq!(s.out_level, OutLevel::Con);
        assert_eq!(s.jacks, [Jack::Both, Jack::Front, Jack::Rear]);
        assert_eq!(s.p48, [true, false, true, false]);
        assert!(s.drive && s.speaker_emu && s.limiter);
        assert!(s.word_single);
    }

    #[test]
    fn mixer_decode_maps_center_and_hard_pan() {
        let mut volume = [0u8; MIXER_ARRAY_BYTES];
        let mut pan = [0u8; MIXER_ARRAY_BYTES];
        let mut hw = [0u8; MIXER_HW_BYTES];

        put_half(&mut volume, 0, FLASH_0DB_VOL);
        put_half(&mut pan, 0, 128);
        put_half(&mut volume, 1, FLASH_0DB_VOL);
        put_half(&mut pan, 1, 0);
        put_half(&mut volume, FLASH_ROW, FLASH_0DB_VOL);
        put_half(&mut pan, FLASH_ROW, 128);
        put_half(&mut hw, 0, FLASH_0DB_VOL);
        put_half(&mut hw, 1, FLASH_0DB_VOL);

        let m = decode_mixer(&volume, &pan, &hw).expect("decode");
        let center = m.sends_in[0][0];
        assert!((center.db.unwrap() + 6.0206).abs() < 0.01);
        assert_eq!(center.pan, 0.0);
        let left = m.sends_in[1][0];
        assert!((left.db.unwrap()).abs() < 0.01);
        assert_eq!(left.pan, -1.0);
        assert!((m.sends_play[0][0].db.unwrap() + 6.0206).abs() < 0.01);
        assert!((m.out[0].gain.unwrap()).abs() < 0.01);
    }
}
