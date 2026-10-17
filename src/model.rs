//! Fireface 800 mix model: 28 mono hardware inputs, 28 mono playback channels and
//! 28 hardware outputs grouped as 14 stereo pairs.
//!
//! The model follows TotalMix's submix workflow. Every source channel has one send
//! (gain + pan) into every output pair. The mixer view edits the sends into the
//! selected pair; the matrix view edits the same values directly. Mute, solo, stereo
//! links and pan are folded into the 28 x 56 linear gain matrix that the DSP receives.
//!
//! The structure and taper follow Openface Mixer (MIT) by Thomas Nelson, which is the
//! reference implementation for this hardware on Linux.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const N_IN: usize = 28;
pub const N_PLAY: usize = 28;
pub const N_OUT: usize = 28;
pub const N_SRC: usize = N_IN + N_PLAY;
pub const N_PAIRS: usize = N_OUT / 2;
pub const N_SLOTS: usize = 8;

pub const FADER_MAX_DB: f32 = 6.0;
pub const FADER_MIN_DB: f32 = -80.0;

/// One send: level into an output pair, and pan within that pair.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Send {
    /// `None` is -inf.
    pub db: Option<f32>,
    /// -1 = hard left, 0 = centre, +1 = hard right.
    pub pan: f32,
}

impl Default for Send {
    fn default() -> Self {
        Self { db: None, pan: 0.0 }
    }
}

/// Output-pair master. `gain: None` is -inf; `mute` forces silence.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct OutPair {
    pub gain: Option<f32>,
    pub mute: bool,
}

impl Default for OutPair {
    fn default() -> Self {
        Self { gain: Some(0.0), mute: false }
    }
}

/// The live mixer state. This is owned by the UI and sent to the backend as a whole;
/// the backend only writes changed DSP blocks.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MixState {
    pub version: u32,
    /// Output pair whose submix the mixer row faders edit.
    pub selected: usize,
    pub stereo_in: [bool; N_PAIRS],
    pub stereo_play: [bool; N_PAIRS],
    pub mute_in: [bool; N_IN],
    pub mute_play: [bool; N_PLAY],
    /// Solo is monitoring, like TotalMix: never saved, never part of a preset.
    #[serde(skip)]
    pub solo_in: [bool; N_IN],
    #[serde(skip)]
    pub solo_play: [bool; N_PLAY],
    /// `sends_in[channel][output_pair]`.
    pub sends_in: [[Send; N_PAIRS]; N_IN],
    /// `sends_play[channel][output_pair]`.
    pub sends_play: [[Send; N_PAIRS]; N_PLAY],
    pub out: [OutPair; N_PAIRS],
    /// Strip key ("in:0", "play:0", "out:0") -> fader group 1..=4.
    pub groups: HashMap<String, u8>,
}

impl Default for MixState {
    fn default() -> Self {
        let mut st = Self {
            version: 1,
            selected: 0, // AN 1/2, the default main out
            stereo_in: [false; N_PAIRS],
            stereo_play: [true; N_PAIRS],
            mute_in: [false; N_IN],
            mute_play: [false; N_PLAY],
            solo_in: [false; N_IN],
            solo_play: [false; N_PLAY],
            sends_in: [[Send::default(); N_PAIRS]; N_IN],
            sends_play: [[Send::default(); N_PAIRS]; N_PLAY],
            out: [OutPair::default(); N_PAIRS],
            groups: HashMap::new(),
        };
        // Transparent default: playback n -> output n, as the hardware does with no mixer.
        for c in 0..N_PLAY {
            st.sends_play[c][c / 2] = Send { db: Some(0.0), pan: 0.0 };
        }
        st
    }
}

/// The part of the state that presets store (everything except UI/monitor state).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Mix {
    pub stereo_in: [bool; N_PAIRS],
    pub stereo_play: [bool; N_PAIRS],
    pub mute_in: [bool; N_IN],
    pub mute_play: [bool; N_PLAY],
    pub sends_in: [[Send; N_PAIRS]; N_IN],
    pub sends_play: [[Send; N_PAIRS]; N_PLAY],
    pub out: [OutPair; N_PAIRS],
    pub groups: HashMap<String, u8>,
}

impl Mix {
    pub fn from_state(st: &MixState) -> Self {
        Self {
            stereo_in: st.stereo_in,
            stereo_play: st.stereo_play,
            mute_in: st.mute_in,
            mute_play: st.mute_play,
            sends_in: st.sends_in,
            sends_play: st.sends_play,
            out: st.out,
            groups: st.groups.clone(),
        }
    }

    pub fn apply_to(&self, st: &mut MixState) {
        st.stereo_in = self.stereo_in;
        st.stereo_play = self.stereo_play;
        st.mute_in = self.mute_in;
        st.mute_play = self.mute_play;
        st.sends_in = self.sends_in;
        st.sends_play = self.sends_play;
        st.out = self.out;
        st.groups = self.groups.clone();
        st.solo_in = [false; N_IN];
        st.solo_play = [false; N_PLAY];
        if st.selected >= N_PAIRS {
            st.selected = 0;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PresetBank {
    pub slots: [Option<Mix>; N_SLOTS],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    In,
    Play,
    Out,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::In => "in",
            Kind::Play => "play",
            Kind::Out => "out",
        }
    }


}

// ---------------------------------------------------------------- dB helpers

pub fn db2lin(db: Option<f32>) -> f32 {
    match db {
        None => 0.0,
        Some(d) if !d.is_finite() || d <= FADER_MIN_DB => 0.0,
        Some(d) => 10.0f32.powf(d / 20.0),
    }
}

pub fn lin2db(v: f32) -> Option<f32> {
    if v > 1e-9 {
        Some(20.0 * v.log10())
    } else {
        None
    }
}

pub fn fmt_db(db: Option<f32>) -> String {
    match db {
        None => "-∞".to_owned(),
        Some(d) if !d.is_finite() => "-∞".to_owned(),
        Some(d) if d.abs() < 0.05 => "0.0".to_owned(),
        Some(d) => format!("{d:+.1}"),
    }
}

/// Fader taper, position 0..1 -> dB. Most travel sits around 0 dB.
pub const TAPER: [(f32, f32); 8] = [
    (0.0, -90.0),
    (0.05, -60.0),
    (0.12, -45.0),
    (0.25, -30.0),
    (0.40, -20.0),
    (0.60, -10.0),
    (0.80, 0.0),
    (1.0, 6.0),
];

pub fn pos2db(p: f32) -> Option<f32> {
    if p <= 0.001 {
        return None;
    }
    let p = p.clamp(0.0, 1.0);
    for w in TAPER.windows(2) {
        let (p0, d0) = w[0];
        let (p1, d1) = w[1];
        if p <= p1 {
            let t = (p - p0) / (p1 - p0);
            return Some(d0 + t * (d1 - d0));
        }
    }
    Some(TAPER[TAPER.len() - 1].1)
}

pub fn db2pos(db: Option<f32>) -> f32 {
    let Some(db) = db else { return 0.0 };
    if !db.is_finite() || db <= TAPER[0].1 {
        return 0.0;
    }
    for w in TAPER.windows(2) {
        let (p0, d0) = w[0];
        let (p1, d1) = w[1];
        if db <= d1 {
            let t = (db - d0) / (d1 - d0);
            return (p0 + t * (p1 - p0)).clamp(0.0, 1.0);
        }
    }
    1.0
}

pub fn pan_text(pan: f32) -> String {
    let v = (pan * 64.0).round() as i32;
    if v == 0 {
        "C".to_owned()
    } else if v < 0 {
        format!("L{}", -v)
    } else {
        format!("R{v}")
    }
}

// ---------------------------------------------------------------- channel naming

pub fn speed_mode(rate: Option<u32>) -> u32 {
    match rate {
        Some(r) if r > 100_000 => 4,
        Some(r) if r > 50_000 => 2,
        _ => 1,
    }
}

/// Channel numbers present at speed mode 1/2/4. Channel numbers stay fixed; the
/// missing ADAT channels are simply skipped, as in the kernel driver.
pub fn channels(kind: Kind, mode: u32) -> Vec<usize> {
    let _ = kind;
    let analog_spdif: Vec<usize> = (0..12).collect();
    if mode >= 4 {
        return analog_spdif;
    }
    let per_port = (8 / mode) as usize;
    let mut v = analog_spdif;
    v.extend(12..12 + per_port);
    v.extend(20..20 + per_port);
    v
}

pub fn chan_label(kind: Kind, c: usize) -> String {
    if c >= 20 {
        format!("A2 {}", c - 19)
    } else if c >= 12 {
        format!("A1 {}", c - 11)
    } else if c >= 10 {
        format!("SPDIF {}", if c == 10 { "L" } else { "R" })
    } else if kind != Kind::In && c >= 8 {
        // Outputs 9/10 are the phones; inputs 9/10 are the rear mic inputs.
        format!("PH {}", c + 1)
    } else {
        format!("AN {}", c + 1)
    }
}

fn last_word(s: &str) -> &str {
    s.rsplit_once(' ').map(|(_, b)| b).unwrap_or(s)
}

pub fn pair_label(kind: Kind, pair: usize) -> String {
    let c = pair * 2;
    if c >= 10 && c < 12 {
        return "SPDIF".to_owned();
    }
    let a = chan_label(kind, c);
    let b = chan_label(kind, c + 1);
    format!("{a}/{}", last_word(&b))
}

// ---------------------------------------------------------------- matrix

impl MixState {
    pub fn any_solo(&self) -> bool {
        self.solo_in.iter().any(|&v| v) || self.solo_play.iter().any(|&v| v)
    }

    pub fn clear_solo(&mut self) {
        self.solo_in = [false; N_IN];
        self.solo_play = [false; N_PLAY];
    }

    /// Flatten sends, pan, mute and solo into gains[output][source].
    pub fn compute_matrix(&self, solo: bool) -> [[f32; N_SRC]; N_OUT] {
        let mut g = [[0.0f32; N_SRC]; N_OUT];
        let soloing = solo && self.any_solo();

        for (kind, base, count, stereo, mutes, solos, sends) in [
            (
                Kind::In,
                0usize,
                N_IN,
                &self.stereo_in,
                &self.mute_in,
                &self.solo_in,
                &self.sends_in,
            ),
            (
                Kind::Play,
                N_IN,
                N_PLAY,
                &self.stereo_play,
                &self.mute_play,
                &self.solo_play,
                &self.sends_play,
            ),
        ] {
            let _ = kind;
            for c in 0..count {
                if mutes[c] {
                    continue;
                }
                let linked = stereo[c / 2];
                let left = c % 2 == 0;
                let silenced = soloing && !solos[c];
                for p in 0..N_PAIRS {
                    // Solo-in-place reaches only the submix being edited.
                    if silenced && p == self.selected {
                        continue;
                    }
                    let send = sends[c][p];
                    let lin = db2lin(send.db);
                    if lin == 0.0 {
                        continue;
                    }
                    // Balance law, centre = full on both sides (Openface Mixer).
                    let gl = lin * (1.0 - send.pan).min(1.0);
                    let gr = lin * (1.0 + send.pan).min(1.0);
                    let src = base + c;
                    if linked {
                        if left {
                            g[2 * p][src] += gl;
                        } else {
                            g[2 * p + 1][src] += gr;
                        }
                    } else {
                        g[2 * p][src] += gl;
                        g[2 * p + 1][src] += gr;
                    }
                }
            }
        }
        g
    }

    /// Output master linear gains, one per hardware output channel.
    pub fn compute_out_gains(&self) -> [f32; N_OUT] {
        let mut g = [0.0f32; N_OUT];
        for (p, out) in self.out.iter().enumerate() {
            let v = if out.mute { 0.0 } else { db2lin(out.gain) };
            g[2 * p] = v;
            g[2 * p + 1] = v;
        }
        g
    }

    // ------------------------------------------------------------ strips

    pub fn strip_key(kind: Kind, idx: usize) -> String {
        format!("{}:{idx}", kind.key())
    }

    /// Channels covered by the strip starting at `c`.
    pub fn strip_channels(&self, kind: Kind, c: usize) -> Vec<usize> {
        match kind {
            Kind::Out => vec![2 * c, 2 * c + 1],
            Kind::In | Kind::Play => {
                let stereo = if kind == Kind::In { &self.stereo_in } else { &self.stereo_play };
                if c % 2 == 0 && stereo[c / 2] {
                    vec![c, c + 1]
                } else {
                    vec![c]
                }
            }
        }
    }

    pub fn strip_db(&self, kind: Kind, idx: usize) -> Option<f32> {
        match kind {
            Kind::Out => self.out.get(idx).and_then(|o| o.gain),
            Kind::In => self.sends_in.get(idx).map(|s| s[self.selected].db).unwrap_or(None),
            Kind::Play => self.sends_play.get(idx).map(|s| s[self.selected].db).unwrap_or(None),
        }
    }

    pub fn set_strip_db(&mut self, kind: Kind, idx: usize, db: Option<f32>) {
        let db = db.map(|d| d.clamp(FADER_MIN_DB, FADER_MAX_DB));
        let val = match db {
            Some(d) if d <= FADER_MIN_DB => None,
            other => other,
        };
        match kind {
            Kind::Out => {
                if let Some(o) = self.out.get_mut(idx) {
                    o.gain = val;
                }
            }
            Kind::In => {
                if idx < N_IN {
                    for c in self.strip_channels(Kind::In, idx) {
                        self.sends_in[c][self.selected].db = val;
                    }
                }
            }
            Kind::Play => {
                if idx < N_PLAY {
                    for c in self.strip_channels(Kind::Play, idx) {
                        self.sends_play[c][self.selected].db = val;
                    }
                }
            }
        }
    }

    pub fn strip_pan(&self, kind: Kind, idx: usize) -> f32 {
        match kind {
            Kind::Out => 0.0,
            Kind::In => self.sends_in.get(idx).map(|s| s[self.selected].pan).unwrap_or(0.0),
            Kind::Play => self.sends_play.get(idx).map(|s| s[self.selected].pan).unwrap_or(0.0),
        }
    }

    pub fn set_strip_pan(&mut self, kind: Kind, idx: usize, pan: f32) {
        let pan = pan.clamp(-1.0, 1.0);
        match kind {
            Kind::Out => {}
            Kind::In => {
                if idx < N_IN {
                    for c in self.strip_channels(Kind::In, idx) {
                        self.sends_in[c][self.selected].pan = pan;
                    }
                }
            }
            Kind::Play => {
                if idx < N_PLAY {
                    for c in self.strip_channels(Kind::Play, idx) {
                        self.sends_play[c][self.selected].pan = pan;
                    }
                }
            }
        }
    }

    pub fn group_follow(&mut self, key: &str, old_db: Option<f32>, new_db: Option<f32>) -> Vec<String> {
        let Some(group) = self.groups.get(key).copied() else {
            return Vec::new();
        };
        let old = old_db.unwrap_or(FADER_MIN_DB);
        let new = new_db.unwrap_or(FADER_MIN_DB);
        let delta = new - old;
        if delta == 0.0 {
            return Vec::new();
        }
        let mut changed = Vec::new();
        let keys: Vec<String> = self
            .groups
            .iter()
            .filter(|(k, g)| **g == group && k.as_str() != key)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            let Some((kind, idx)) = parse_strip_key(&k) else { continue };
            let cur = self.strip_db(kind, idx).unwrap_or(FADER_MIN_DB);
            let mut val = (cur + delta).min(FADER_MAX_DB);
            if val <= FADER_MIN_DB {
                val = FADER_MIN_DB;
            }
            let new_val = if val <= FADER_MIN_DB { None } else { Some(val) };
            if new_val != self.strip_db(kind, idx) {
                self.set_strip_db(kind, idx, new_val);
                changed.push(k);
            }
        }
        changed
    }

}

pub fn parse_strip_key(key: &str) -> Option<(Kind, usize)> {
    let (k, idx) = key.split_once(':')?;
    let idx = idx.parse().ok()?;
    let kind = match k {
        "in" => Kind::In,
        "play" => Kind::Play,
        "out" => Kind::Out,
        _ => return None,
    };
    Some((kind, idx))
}
