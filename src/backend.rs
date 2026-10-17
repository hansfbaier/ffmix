//! Hardware worker: owns the FireWire handle and is the only thread that talks to
//! the device. The UI sends whole mixer states; the worker writes only changed
//! DSP blocks and publishes meters/status back.

use std::fs;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::ff800::{
    self, DeviceSettings, MeterFrame, StatusInfo, CONFIG_WORDS, METER_LEN, MIXER_BLOCK, REG_CONFIG,
    REG_METERS, REG_MIXER, REG_OUTPUT, REG_STATUS,
};
use crate::flash::{self, FlashMixer};
use crate::fw::Firewire;
use crate::model::{MixState, N_OUT, N_SRC};

pub enum Command {
    Mix(Box<MixState>),
    Settings(Box<DeviceSettings>),
    ReadFlashSettings,
    ReadFlashMixer,
    Rescan,
}

pub enum Event {
    Connection { node: Option<String>, error: Option<String> },
    Status(StatusInfo),
    Settings(DeviceSettings),
    FlashSettings(DeviceSettings),
    FlashMixer(FlashMixer),
    FlashError(String),
    Meters(MeterFrame),
    Error(String),
}

pub struct Backend {
    pub cmds: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Backend {
    pub fn spawn() -> Self {
        let (cmds, cmd_rx) = mpsc::channel();
        let (evt_tx, events) = mpsc::channel();
        thread::Builder::new()
            .name("ff800-io".to_owned())
            .spawn(move || run(cmd_rx, evt_tx))
            .expect("spawn FireWire worker");
        Self { cmds, events }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.cmds.send(cmd);
    }

    pub fn set_mix(&self, mix: &MixState) {
        self.send(Command::Mix(Box::new(mix.clone())));
    }

    pub fn set_settings(&self, settings: &DeviceSettings) {
        self.send(Command::Settings(Box::new(settings.clone())));
    }
}

#[derive(Default)]
struct Sent {
    mix: [Option<[u32; MIXER_BLOCK]>; N_OUT],
    out: [Option<u32>; N_OUT],
    config: Option<[u32; CONFIG_WORDS]>,
}

/// Smoothstep ramp for DSP coefficients. Mouse faders, pan knobs and matrix
/// cells arrive as small but abrupt coefficient changes; writing them directly
/// produces zipper noise. Each new target starts from the current interpolated
/// value, so fast drags chain smoothly instead of restarting from the old value.
struct Ramp {
    initialized: bool,
    active: bool,
    started: Instant,
    start: [[f32; N_SRC]; N_OUT],
    target: [[f32; N_SRC]; N_OUT],
    current: [[f32; N_SRC]; N_OUT],
    start_out: [f32; N_OUT],
    target_out: [f32; N_OUT],
    current_out: [f32; N_OUT],
}

const RAMP_MS: f32 = 30.0;

impl Default for Ramp {
    fn default() -> Self {
        Self {
            initialized: false,
            active: false,
            started: Instant::now(),
            start: [[0.0; N_SRC]; N_OUT],
            target: [[0.0; N_SRC]; N_OUT],
            current: [[0.0; N_SRC]; N_OUT],
            start_out: [0.0; N_OUT],
            target_out: [0.0; N_OUT],
            current_out: [0.0; N_OUT],
        }
    }
}

impl Ramp {
    fn reset(&mut self) {
        self.initialized = false;
        self.active = false;
    }

    fn retarget(
        &mut self,
        target: [[f32; N_SRC]; N_OUT],
        target_out: [f32; N_OUT],
        now: Instant,
    ) {
        if !self.initialized {
            self.current = target;
            self.target = target;
            self.current_out = target_out;
            self.target_out = target_out;
            self.initialized = true;
            self.active = false;
            return;
        }
        if target != self.target || target_out != self.target_out {
            self.start = self.current;
            self.start_out = self.current_out;
            self.target = target;
            self.target_out = target_out;
            self.started = now;
            self.active = true;
        }
    }

    fn tick(&mut self, now: Instant) {
        if !self.active {
            return;
        }
        let t = now.duration_since(self.started).as_secs_f32() / (RAMP_MS / 1000.0);
        if t >= 1.0 {
            self.current = self.target;
            self.current_out = self.target_out;
            self.active = false;
            return;
        }
        let t = t.clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        for o in 0..N_OUT {
            for src in 0..N_SRC {
                self.current[o][src] =
                    self.start[o][src] + (self.target[o][src] - self.start[o][src]) * s;
            }
            self.current_out[o] =
                self.start_out[o] + (self.target_out[o] - self.start_out[o]) * s;
        }
    }
}

impl Sent {
    fn reset(&mut self) {
        self.mix = [None; N_OUT];
        self.out = [None; N_OUT];
        self.config = None;
    }
}

fn run(cmd_rx: Receiver<Command>, evt_tx: Sender<Event>) {
    let mut fw: Option<Firewire> = None;
    let mut mix: Option<MixState> = None;
    let mut dirty = false;
    let mut settings = DeviceSettings::default();
    let mut sent = Sent::default();
    let mut ramp = Ramp::default();
    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut last_meters = Instant::now() - Duration::from_secs(1);
    let mut next_open = Instant::now();
    let mut last_conn_error: Option<String> = None;
    let mut status_failures = 0u32;
    let mut meter_failures = 0u32;
    let mut last_conflict_check = Instant::now() - Duration::from_secs(1);

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Command::Mix(m) => {
                    mix = Some(*m);
                    dirty = true;
                }
                Command::Settings(s) => {
                    settings = *s;
                    dirty = true;
                }
                Command::ReadFlashSettings => {
                    let Some(device) = fw.as_mut() else {
                        let _ = evt_tx.send(Event::FlashError(
                            "not connected to the Fireface".to_owned(),
                        ));
                        continue;
                    };
                    match read_flash_settings(device) {
                        Ok(settings) => {
                            let _ = evt_tx.send(Event::FlashSettings(settings));
                        }
                        Err(err) => {
                            let _ = evt_tx.send(Event::FlashError(err.to_string()));
                        }
                    }
                }
                Command::ReadFlashMixer => {
                    let Some(device) = fw.as_mut() else {
                        let _ = evt_tx.send(Event::FlashError(
                            "not connected to the Fireface".to_owned(),
                        ));
                        continue;
                    };
                    match read_flash_mixer(device) {
                        Ok(mixer) => {
                            let _ = evt_tx.send(Event::FlashMixer(mixer));
                        }
                        Err(err) => {
                            let _ = evt_tx.send(Event::FlashError(err.to_string()));
                        }
                    }
                }
                Command::Rescan => {
                    fw = None;
                    dirty = true;
                    sent.reset();
                    ramp.reset();
                    next_open = Instant::now();
                    last_conn_error = None;
                    let _ = evt_tx.send(Event::Connection { node: None, error: None });
                }
            }
        }

        if fw.is_none() && Instant::now() >= next_open {
            if std::env::var_os("FFMIX_ALLOW_CONFLICT").is_none() {
                if let Some(conflict) = conflicting_process() {
                    let msg = format!(
                        "Conflicting mixer is running: {conflict}. Stop it before FFMix can control the Fireface 800."
                    );
                    if last_conn_error.as_deref() != Some(&msg) {
                        let _ = evt_tx.send(Event::Connection {
                            node: None,
                            error: Some(msg.clone()),
                        });
                        last_conn_error = Some(msg);
                    }
                    next_open = Instant::now() + Duration::from_secs(1);
                    thread::sleep(Duration::from_millis(3));
                    continue;
                }
            }
            match Firewire::open_ff800() {
                Ok(mut device) => {
                    match device.read_block(REG_STATUS, 8) {
                        Ok(raw) if raw.len() >= 8 => {
                            let q0 = le32(&raw, 0);
                            let q1 = le32(&raw, 4);
                            if !settings.known {
                                settings.apply_status(q0, q1);
                                let _ = evt_tx.send(Event::Settings(settings.clone()));
                            }
                            let _ = evt_tx.send(Event::Connection {
                                node: Some(device.node.clone()),
                                error: None,
                            });
                            let _ = evt_tx.send(Event::Status(ff800::status_info(q0, q1)));
                            last_conn_error = None;
                            sent.reset();
                            ramp.reset();
                            dirty = true;
                            status_failures = 0;
                            meter_failures = 0;
                            last_status = Instant::now();
                            last_meters = Instant::now();
                            fw = Some(device);
                        }
                        Ok(_) | Err(_) => {
                            let msg = "status read failed while opening the device".to_owned();
                            if last_conn_error.as_deref() != Some(&msg) {
                                let _ = evt_tx.send(Event::Connection {
                                    node: None,
                                    error: Some(msg.clone()),
                                });
                                last_conn_error = Some(msg);
                            }
                            next_open = Instant::now() + Duration::from_secs(1);
                        }
                    }
                }
                Err(err) => {
                    let msg = err.to_string();
                    if last_conn_error.as_deref() != Some(&msg) {
                        let _ = evt_tx.send(Event::Connection {
                            node: None,
                            error: Some(msg.clone()),
                        });
                        last_conn_error = Some(msg);
                    }
                    next_open = Instant::now() + Duration::from_secs(1);
                }
            }
        }

        if fw.is_some()
            && std::env::var_os("FFMIX_ALLOW_CONFLICT").is_none()
            && last_conflict_check.elapsed() >= Duration::from_secs(1)
        {
            last_conflict_check = Instant::now();
            if let Some(conflict) = conflicting_process() {
                let msg = format!(
                    "Conflicting mixer is running: {conflict}. Stop it before FFMix can control the Fireface 800."
                );
                let _ = evt_tx.send(Event::Connection {
                    node: None,
                    error: Some(msg.clone()),
                });
                let _ = evt_tx.send(Event::Error(msg.clone()));
                last_conn_error = Some(msg);
                fw = None;
                sent.reset();
                ramp.reset();
                dirty = true;
                status_failures = 0;
                meter_failures = 0;
                next_open = Instant::now() + Duration::from_secs(1);
                thread::sleep(Duration::from_millis(3));
                continue;
            }
        }

        if let Some(device) = fw.as_mut() {
            let mut failed: Option<io::Error> = None;

            if dirty || ramp.active {
                match mix.as_ref() {
                    Some(m) => match sync(
                        device,
                        m,
                        &settings,
                        &mut sent,
                        &mut ramp,
                        Instant::now(),
                    ) {
                        Ok(_routes) => dirty = false,
                        Err(err) => failed = Some(err),
                    },
                    None => {
                        dirty = false;
                        ramp.reset();
                    }
                }
            }

            if failed.is_none() && last_status.elapsed() >= Duration::from_millis(500) {
                last_status = Instant::now();
                match device.read_block(REG_STATUS, 8) {
                    Ok(raw) if raw.len() >= 8 => {
                        status_failures = 0;
                        let q0 = le32(&raw, 0);
                        let q1 = le32(&raw, 4);
                        if !settings.known {
                            settings.apply_status(q0, q1);
                            let _ = evt_tx.send(Event::Settings(settings.clone()));
                            dirty = true;
                        }
                        let _ = evt_tx.send(Event::Status(ff800::status_info(q0, q1)));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        status_failures += 1;
                        if status_failures >= 3 {
                            failed = Some(err);
                        }
                    }
                }
            }

            if failed.is_none() && last_meters.elapsed() >= crate::fw::meter_period() {
                last_meters = Instant::now();
                match device.read_block(REG_METERS, METER_LEN) {
                    Ok(raw) if raw.len() >= METER_LEN => {
                        meter_failures = 0;
                        let _ = evt_tx.send(Event::Meters(ff800::decode_meters(&raw)));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        meter_failures += 1;
                        if meter_failures >= 5 {
                            failed = Some(err);
                        }
                    }
                }
            }

            if let Some(err) = failed {
                let msg = err.to_string();
                let _ = evt_tx.send(Event::Connection {
                    node: None,
                    error: Some(msg.clone()),
                });
                let _ = evt_tx.send(Event::Error(msg));
                fw = None;
                sent.reset();
                ramp.reset();
                dirty = true;
                next_open = Instant::now() + Duration::from_secs(1);
            }
        }

        thread::sleep(Duration::from_millis(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_smoothsteps_from_current_to_target() {
        let out = [0.0; N_OUT];
        let mut target = [[0.0; N_SRC]; N_OUT];
        target[0][0] = 1.0;
        let mut ramp = Ramp::default();
        let now = Instant::now();
        ramp.retarget(target, out, now);
        assert_eq!(ramp.current[0][0], 1.0);

        let mut target2 = target;
        target2[0][0] = 0.0;
        ramp.retarget(target2, out, now);
        assert!(ramp.active);

        ramp.started = now - Duration::from_millis(15);
        ramp.tick(now);
        assert!(ramp.current[0][0] > 0.0 && ramp.current[0][0] < 1.0);

        ramp.started = now - Duration::from_millis(31);
        ramp.tick(now);
        assert!(!ramp.active);
        assert_eq!(ramp.current[0][0], 0.0);
    }
}

fn sync(
    fw: &mut Firewire,
    mix: &MixState,
    settings: &DeviceSettings,
    sent: &mut Sent,
    ramp: &mut Ramp,
    now: Instant,
) -> io::Result<usize> {
    if settings.known {
        let words = settings.config_words();
        if sent.config != Some(words) {
            fw.write_block(REG_CONFIG, &words)?;
            sent.config = Some(words);
        }
    }

    ramp.retarget(mix.compute_matrix(true), mix.compute_out_gains(), now);
    ramp.tick(now);
    let mut routes = 0usize;

    for o in 0..N_OUT {
        let block = ff800::mixer_block(&ramp.current, o);
        routes += block.iter().filter(|&&v| v != 0).count();
        if sent.mix[o].as_ref() != Some(&block) {
            fw.write_block(REG_MIXER + 0x100u64 * o as u64, &block)?;
            sent.mix[o] = Some(block);
        }
        let vol = ff800::encode_gain(ramp.current_out[o]);
        if sent.out[o] != Some(vol) {
            fw.write_block(REG_OUTPUT + 4u64 * o as u64, &[vol])?;
            sent.out[o] = Some(vol);
        }
    }
    Ok(routes)
}

fn read_flash_bytes(fw: &mut Firewire, mut addr: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let n = (len - out.len()).min(256);
        let block = fw.read_block(addr, n)?;
        if block.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short flash block read",
            ));
        }
        addr += block.len() as u64;
        out.extend_from_slice(&block);
    }
    Ok(out)
}

fn read_flash_settings(fw: &mut Firewire) -> io::Result<DeviceSettings> {
    let raw = read_flash_bytes(fw, flash::SETTINGS_ADDR, flash::SETTINGS_BYTES)?;
    flash::decode_settings(&raw).map_err(io::Error::other)
}

fn read_flash_mixer(fw: &mut Firewire) -> io::Result<FlashMixer> {
    let volume = read_flash_bytes(fw, flash::MIXER_VOLUME_ADDR, flash::MIXER_ARRAY_BYTES)?;
    let pan = read_flash_bytes(fw, flash::MIXER_PAN_ADDR, flash::MIXER_ARRAY_BYTES)?;
    let hw = read_flash_bytes(fw, flash::MIXER_HW_ADDR, flash::MIXER_HW_BYTES)?;
    flash::decode_mixer(&volume, &pan, &hw).map_err(io::Error::other)
}

fn le32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Openface Mixer's engine and snd-fireface-ctl-service also drive the DSP.
/// Running two sources of truth results in a fight over the mixer registers.
fn conflicting_process() -> Option<String> {
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(bytes) = fs::read(format!("/proc/{name}/cmdline")) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let cmd = String::from_utf8_lossy(&bytes).replace('\0', " ");
        if cmd.contains("openface-mixer-engine") && cmd.contains("ff800") {
            return Some(cmd.trim().to_owned());
        }
        if cmd.contains("snd-fireface-ctl-service") {
            return Some(cmd.trim().to_owned());
        }
    }
    None
}
