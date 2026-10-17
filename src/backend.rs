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
use crate::fw::Firewire;
use crate::model::{MixState, N_OUT};

pub enum Command {
    Mix(Box<MixState>),
    Settings(Box<DeviceSettings>),
    Rescan,
}

pub enum Event {
    Connection { node: Option<String>, error: Option<String> },
    Status(StatusInfo),
    Settings(DeviceSettings),
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
                Command::Rescan => {
                    fw = None;
                    dirty = true;
                    sent.reset();
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

            if dirty {
                match mix.as_ref() {
                    Some(m) => match sync(device, m, &settings, &mut sent) {
                        Ok(_routes) => dirty = false,
                        Err(err) => failed = Some(err),
                    },
                    None => dirty = false,
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
                dirty = true;
                next_open = Instant::now() + Duration::from_secs(1);
            }
        }

        thread::sleep(Duration::from_millis(3));
    }
}

fn sync(
    fw: &mut Firewire,
    mix: &MixState,
    settings: &DeviceSettings,
    sent: &mut Sent,
) -> io::Result<usize> {
    if settings.known {
        let words = settings.config_words();
        if sent.config != Some(words) {
            fw.write_block(REG_CONFIG, &words)?;
            sent.config = Some(words);
        }
    }

    let gains = mix.compute_matrix(true);
    let out = mix.compute_out_gains();
    let mut routes = 0usize;

    for o in 0..N_OUT {
        let block = ff800::mixer_block(&gains, o);
        routes += block.iter().filter(|&&v| v != 0).count();
        if sent.mix[o].as_ref() != Some(&block) {
            fw.write_block(REG_MIXER + 0x100u64 * o as u64, &block)?;
            sent.mix[o] = Some(block);
        }
        let vol = ff800::encode_gain(out[o]);
        if sent.out[o] != Some(vol) {
            fw.write_block(REG_OUTPUT + 4u64 * o as u64, &[vol])?;
            sent.out[o] = Some(vol);
        }
    }
    Ok(routes)
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
