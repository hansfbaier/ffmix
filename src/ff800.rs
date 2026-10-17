//! Fireface 800 register interface: DSP mixer, output volumes, configuration,
//! status and level meters. Everything is little-endian and transferred with
//! asynchronous FireWire block/quadlet transactions.
//!
//! Register addresses, bit fields and meter layout are hardware-interface facts.
//! This Rust implementation is original; the mixer model and FF800 encoding were
//! adapted from the MIT-licensed Openface Mixer. No RME code, firmware, binaries,
//! installer contents, or artwork are included.

use crate::model::N_IN;
use crate::model::{N_OUT, N_PLAY, N_SRC};
use serde::{Deserialize, Serialize};

pub const REG_MIXER: u64 = 0x0000_8008_0000;
pub const REG_OUTPUT: u64 = 0x0000_8008_1f80;
pub const REG_CONFIG: u64 = 0x0000_fc88_f014;
pub const REG_STATUS: u64 = 0x0000_801c_0000;
pub const REG_METERS: u64 = 0x0000_8010_0000;

pub const MIXER_BLOCK: usize = 64;
pub const MIXER_AVAIL: usize = 32;
pub const CONFIG_WORDS: usize = 3;
pub const METER_LEN: usize = 8 * (N_IN + 2 * N_OUT) + 4 * (N_IN + N_PLAY + N_OUT);

pub const GAIN_ZERO: f32 = 32768.0;
pub const GAIN_MAX: u32 = 0x0001_0000;
pub const METER_MASK: u32 = 0x7fff_ff00;
pub const METER_FULL: f32 = 0x7fff_ff00u32 as f32;

pub const RME_OUI: u64 = 0x0000_0a35;
pub const FF800_UNIT_VERSION: u64 = 0x0001;

// ---------------------------------------------------------------- settings

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockSource {
    Internal,
    Adat1,
    Adat2,
    Spdif,
    Word,
}

impl Default for ClockSource {
    fn default() -> Self {
        Self::Internal
    }
}

impl ClockSource {
    pub const ALL: [Self; 5] = [Self::Internal, Self::Adat1, Self::Adat2, Self::Spdif, Self::Word];

    pub fn label(self) -> &'static str {
        match self {
            Self::Internal => "Internal",
            Self::Adat1 => "ADAT 1",
            Self::Adat2 => "ADAT 2",
            Self::Spdif => "S/PDIF",
            Self::Word => "Word Clock",
        }
    }

    fn bits(self) -> u32 {
        match self {
            Self::Internal => 0x0000_0001,
            Self::Adat1 => 0x0000_0000,
            Self::Adat2 => 0x0000_0400,
            Self::Spdif => 0x0000_0c00,
            Self::Word => 0x0000_1400,
        }
    }

    fn from_status1(q1: u32) -> Self {
        match q1 & 0x0000_1c01 {
            0x0000_0001 => Self::Internal,
            0x0000_0400 => Self::Adat2,
            0x0000_0c00 => Self::Spdif,
            0x0000_1000 => Self::Word,
            _ => Self::Adat1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InLevel {
    Low,
    Pro,
    Con,
}

impl Default for InLevel {
    fn default() -> Self {
        Self::Low
    }
}

impl InLevel {
    pub const ALL: [Self; 3] = [Self::Low, Self::Pro, Self::Con];
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Lo Gain",
            Self::Pro => "+4 dBu",
            Self::Con => "-10 dBV",
        }
    }
    fn bits(self) -> (u32, u32) {
        match self {
            Self::Low => (0x0000_0008, 0x0000_0000),
            Self::Pro => (0x0000_0010, 0x0000_0002),
            Self::Con => (0x0000_0020, 0x0000_0003),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutLevel {
    Con,
    Pro,
    High,
}

impl Default for OutLevel {
    fn default() -> Self {
        Self::High
    }
}

impl OutLevel {
    pub const ALL: [Self; 3] = [Self::Con, Self::Pro, Self::High];
    pub fn label(self) -> &'static str {
        match self {
            Self::Con => "-10 dBV",
            Self::Pro => "+4 dBu",
            Self::High => "Hi Gain",
        }
    }
    fn bits(self) -> (u32, u32) {
        match self {
            Self::Con => (0x0000_1000, 0x0000_0008),
            Self::Pro => (0x0000_0800, 0x0000_0018),
            Self::High => (0x0000_0400, 0x0000_0010),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Jack {
    Front,
    Rear,
    Both,
}

impl Default for Jack {
    fn default() -> Self {
        Self::Front
    }
}

impl Jack {
    pub const ALL: [Self; 3] = [Self::Front, Self::Rear, Self::Both];
    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front",
            Self::Rear => "Rear",
            Self::Both => "Front + Rear",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    /// True once the defaults have been filled in from the device's status register.
    pub known: bool,
    pub clock: ClockSource,
    pub spdif_in_optical: bool,
    pub opt_out_spdif: bool,
    pub spdif_pro: bool,
    pub emphasis: bool,
    pub non_audio: bool,
    pub word_single: bool,
    pub in_level: InLevel,
    pub out_level: OutLevel,
    /// AN 1, AN 7, AN 8.
    pub jacks: [Jack; 3],
    /// AN 7..=AN 10.
    pub p48: [bool; 4],
    pub drive: bool,
    pub limiter: bool,
    pub speaker_emu: bool,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            known: false,
            clock: ClockSource::Internal,
            spdif_in_optical: false,
            opt_out_spdif: false,
            spdif_pro: false,
            emphasis: false,
            non_audio: false,
            word_single: false,
            in_level: InLevel::Low,
            out_level: OutLevel::High,
            jacks: [Jack::Front; 3],
            p48: [false; 4],
            drive: false,
            limiter: false,
            speaker_emu: false,
        }
    }
}

const Q0_INST_DRIVE: u32 = 0x0000_0200;
const Q0_INST_SPEAKER_EMU: u32 = 0x0000_0004;
const Q1_INST_DRIVE: u32 = 0x0000_0200;
const Q2_CONTINUE_AT_ERRORS: u32 = 0x8000_0000;
const Q2_INST_LIMITER: u32 = 0x0001_0000;
const Q2_WORD_OUT_SINGLE: u32 = 0x0000_2000;
const Q2_SPDIF_IN_OPTICAL: u32 = 0x0000_0200;
const Q2_OPT_OUT_SPDIF: u32 = 0x0000_0100;
const Q2_SPDIF_OUT_NON_AUDIO: u32 = 0x0000_0080;
const Q2_SPDIF_OUT_EMPHASIS: u32 = 0x0000_0040;
const Q2_SPDIF_OUT_PRO: u32 = 0x0000_0020;
const Q2_RATES: u32 = 0x0000_001e;

impl DeviceSettings {
    /// Take the options the device reports (status quadlet 1).
    pub fn apply_status(&mut self, _q0: u32, q1: u32) {
        self.clock = ClockSource::from_status1(q1);
        self.spdif_in_optical = q1 & 0x0000_0200 != 0;
        self.opt_out_spdif = q1 & 0x0000_0100 != 0;
        self.spdif_pro = q1 & 0x0000_0020 != 0;
        self.emphasis = q1 & 0x0000_0040 != 0;
        self.word_single = q1 & 0x0000_2000 != 0;
        self.known = true;
    }

    /// The three configuration quadlets. Write-only on the FF800.
    pub fn config_words(&self) -> [u32; CONFIG_WORDS] {
        let mut q = [0u32, 0u32, Q2_CONTINUE_AT_ERRORS | Q2_RATES];
        q[2] |= self.clock.bits();
        let (b0, b1) = self.in_level.bits();
        q[0] |= b0;
        q[1] |= b1;
        let (b0, b1) = self.out_level.bits();
        q[0] |= b0;
        q[1] |= b1;

        // (rear bit, front bit) for AN 1, AN 7, AN 8
        const JACK_BITS: [(u32, u32); 3] = [
            (0x0000_0004, 0x0000_0800),
            (0x0000_0040, 0x0000_0020),
            (0x0000_0100, 0x0000_0080),
        ];
        for (jack, (rear, front)) in self.jacks.iter().zip(JACK_BITS) {
            if *jack != Jack::Front {
                q[1] |= rear;
            }
            if *jack != Jack::Rear {
                q[1] |= front;
            }
        }

        const P48_BITS: [u32; 4] = [0x0000_0001, 0x0000_0080, 0x0000_0002, 0x0000_0100];
        for (on, bit) in self.p48.iter().zip(P48_BITS) {
            if *on {
                q[0] |= bit;
            }
        }
        if self.drive {
            q[0] |= Q0_INST_DRIVE;
            q[1] |= Q1_INST_DRIVE;
        }
        if self.speaker_emu {
            q[0] |= Q0_INST_SPEAKER_EMU;
        }
        if self.limiter {
            q[2] |= Q2_INST_LIMITER;
        }
        if self.spdif_in_optical {
            q[2] |= Q2_SPDIF_IN_OPTICAL;
        }
        if self.opt_out_spdif {
            q[2] |= Q2_OPT_OUT_SPDIF;
        }
        if self.spdif_pro {
            q[2] |= Q2_SPDIF_OUT_PRO;
        }
        if self.emphasis {
            q[2] |= Q2_SPDIF_OUT_EMPHASIS;
        }
        if self.non_audio {
            q[2] |= Q2_SPDIF_OUT_NON_AUDIO;
        }
        if self.word_single {
            q[2] |= Q2_WORD_OUT_SINGLE;
        }
        q
    }
}

// ---------------------------------------------------------------- status

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncState {
    NoLock,
    Lock,
    Sync,
}

impl SyncState {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoLock => "No Lock",
            Self::Lock => "Lock",
            Self::Sync => "Sync",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InputStatus {
    pub label: &'static str,
    pub state: SyncState,
    pub rate: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct StatusInfo {
    pub rate: Option<u32>,
    pub source: String,
    pub inputs: Vec<InputStatus>,
}

fn rate_from_q0(q0: u32) -> Option<u32> {
    match q0 & 0x1e00_0000 {
        0x0200_0000 => Some(32_000),
        0x0400_0000 => Some(44_100),
        0x0600_0000 => Some(48_000),
        0x0800_0000 => Some(64_000),
        0x0a00_0000 => Some(88_200),
        0x0e00_0000 => Some(96_000),
        0x0c00_0000 => Some(128_000),
        0x1000_0000 => Some(176_400),
        0x1200_0000 => Some(192_000),
        _ => None,
    }
}

fn spdif_rate(q0: u32) -> Option<u32> {
    match q0 & 0x0003_c000 {
        0x0000_4000 => Some(32_000),
        0x0000_8000 => Some(44_100),
        0x0000_c000 => Some(48_000),
        0x0001_0000 => Some(64_000),
        0x0001_4000 => Some(88_200),
        0x0001_8000 => Some(96_000),
        0x0001_c000 => Some(128_000),
        0x0002_0000 => Some(176_400),
        0x0002_4000 => Some(192_000),
        _ => None,
    }
}

pub fn status_rate(q1: u32) -> Option<u32> {
    match q1 & 0x1e {
        0x02 => Some(32_000),
        0x00 => Some(44_100),
        0x06 => Some(48_000),
        0x0a => Some(64_000),
        0x08 => Some(88_200),
        0x0e => Some(96_000),
        0x12 => Some(128_000),
        0x10 => Some(176_400),
        0x16 => Some(192_000),
        _ => None,
    }
}

pub fn status_info(q0: u32, q1: u32) -> StatusInfo {
    let internal = q1 & 1 != 0;
    let ext_rate = rate_from_q0(q0);
    let active = match q0 & 0x01c0_0000 {
        0x01c0_0000 => "Internal",
        0x0000_0000 => "ADAT 1",
        0x0040_0000 => "ADAT 2",
        0x00c0_0000 => "S/PDIF",
        0x0100_0000 => "Word Clock",
        0x0180_0000 => "TCO",
        _ => "—",
    };
    let inputs_def: [(&'static str, u32, u32); 4] = [
        ("ADAT 1", 0x0000_1000, 0x0000_0400),
        ("ADAT 2", 0x0000_2000, 0x0000_0800),
        ("S/PDIF", 0x0004_0000, 0x0010_0000),
        ("Word Clock", 0x2000_0000, 0x4000_0000),
    ];
    let mut inputs = Vec::with_capacity(4);
    for (label, lock, sync) in inputs_def {
        let state = if q0 & lock != 0 && q0 & sync != 0 {
            SyncState::Sync
        } else if q0 & lock != 0 {
            SyncState::Lock
        } else {
            SyncState::NoLock
        };
        let rate = if q0 & lock == 0 {
            None
        } else if label == "S/PDIF" {
            spdif_rate(q0)
        } else if active == label {
            ext_rate
        } else {
            None
        };
        inputs.push(InputStatus { label, state, rate });
    }
    let from_q1 = status_rate(q1);
    StatusInfo {
        rate: if internal || ext_rate.is_none() { from_q1 } else { ext_rate },
        source: if internal { "Internal".to_owned() } else { active.to_owned() },
        inputs,
    }
}

// ---------------------------------------------------------------- encoding

/// Linear gain -> FF800 mixer coefficient. 0x8000 is unity, 0x10000 is +6 dB.
pub fn encode_gain(lin: f32) -> u32 {
    if !(lin > 0.0) {
        return 0;
    }
    (lin * GAIN_ZERO).round().clamp(0.0, GAIN_MAX as f32) as u32
}

/// One output's 64-quadlet mixer block: 28 inputs at 0..28, 28 playbacks at 32..60.
pub fn mixer_block(gains: &[[f32; N_SRC]; N_OUT], out: usize) -> [u32; MIXER_BLOCK] {
    let mut block = [0u32; MIXER_BLOCK];
    for i in 0..N_IN {
        block[i] = encode_gain(gains[out][i]);
    }
    for p in 0..N_PLAY {
        block[MIXER_AVAIL + p] = encode_gain(gains[out][N_IN + p]);
    }
    block
}

#[derive(Clone, Debug, Default)]
pub struct MeterFrame {
    pub input: [f32; N_IN],
    pub playback: [f32; N_PLAY],
    pub output: [f32; N_OUT],
}

/// Decode the 1008-byte meter block. The last 84 quadlets hold inputs, playback and
/// outputs; the preceding data is per-mixer peak history which we do not need.
pub fn decode_meters(raw: &[u8]) -> MeterFrame {
    let mut frame = MeterFrame::default();
    let mut off = 8 * (N_IN + 2 * N_OUT);
    let mut take = |dst: &mut [f32]| {
        for v in dst.iter_mut() {
            let Some(bytes) = raw.get(off..off + 4) else { return };
            let word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            *v = ((word & METER_MASK) as f32 / METER_FULL).clamp(0.0, 2.0);
            off += 4;
        }
    };
    take(&mut frame.input);
    take(&mut frame.playback);
    take(&mut frame.output);
    frame
}
