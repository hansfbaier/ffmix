//! Minimal Linux firewire-core (`/dev/fw*`) client: asynchronous quadlet/block
//! transactions, bus-reset aware. This is an independent Rust implementation of
//! the firewire-cdev ABI; no vendor or third-party driver source is included.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::time::Duration;

use crate::ff800::{FF800_UNIT_VERSION, RME_OUI};

const FW_CDEV_IOC_GET_INFO: libc::c_ulong = 0xc028_2300;
const FW_CDEV_IOC_SEND_REQUEST: libc::c_ulong = 0x4028_2301;

const TCODE_WRITE_BLOCK: u32 = 0x1;
const TCODE_READ_BLOCK: u32 = 0x5;

const RCODE_COMPLETE: u32 = 0x0;
const RCODE_GENERATION: u32 = 0x13;

const EVENT_BUS_RESET: u32 = 0x0;
const EVENT_RESPONSE: u32 = 0x1;

const TIMEOUT_MS: i32 = 200;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FwCdevGetInfo {
    version: u32,
    rom_length: u32,
    rom: u64,
    bus_reset: u64,
    bus_reset_closure: u64,
    card: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FwCdevSendRequest {
    tcode: u32,
    length: u32,
    offset: u64,
    closure: u64,
    data: u64,
    generation: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FwCdevEventBusReset {
    closure: u64,
    r#type: u32,
    node_id: u32,
    local_node_id: u32,
    bm_node_id: u32,
    irm_node_id: u32,
    root_node_id: u32,
    generation: u32,
}

pub struct Firewire {
    file: File,
    generation: u32,
    closure: u64,
    pub node: String,
}

impl Firewire {
    pub fn open_ff800() -> io::Result<Self> {
        let node = find_ff800_node().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "RME Fireface 800 not found in /sys/bus/firewire/devices",
            )
        })?;
        let path = format!("/dev/{node}");
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut reset = FwCdevEventBusReset::default();
        let mut info = FwCdevGetInfo {
            version: 4,
            bus_reset: (&mut reset as *mut FwCdevEventBusReset) as u64,
            ..Default::default()
        };
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), FW_CDEV_IOC_GET_INFO, &mut info) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file, generation: reset.generation, closure: 0, node })
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    fn refresh_generation(&mut self) -> io::Result<()> {
        let mut reset = FwCdevEventBusReset::default();
        let mut info = FwCdevGetInfo {
            version: 4,
            bus_reset: (&mut reset as *mut FwCdevEventBusReset) as u64,
            ..Default::default()
        };
        let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), FW_CDEV_IOC_GET_INFO, &mut info) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        self.generation = reset.generation;
        Ok(())
    }

    fn poll_event(&self) -> io::Result<Vec<u8>> {
        let mut pfd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, TIMEOUT_MS) };
        if rc == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "FireWire request timed out"));
        }
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return self.poll_event();
            }
            return Err(err);
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "FireWire device disappeared"));
        }
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { libc::read(self.file.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 {
                return Ok(buf[..n as usize].to_vec());
            }
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                _ => return Err(err),
            }
        }
    }

    #[allow(unused_assignments)]
    fn request(&mut self, tcode: u32, offset: u64, data: &mut [u8]) -> io::Result<usize> {
        for _attempt in 0..2 {
            self.closure = self.closure.wrapping_add(1);
            let closure = self.closure;
            let req = FwCdevSendRequest {
                tcode,
                length: data.len() as u32,
                offset,
                closure,
                data: data.as_mut_ptr() as u64,
                generation: self.generation,
            };
            let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), FW_CDEV_IOC_SEND_REQUEST, &req) };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }

            let mut retry_generation = false;
            loop {
                let ev = self.poll_event()?;
                if ev.len() < 12 {
                    continue;
                }
                let ty = u32::from_ne_bytes([ev[8], ev[9], ev[10], ev[11]]);
                match ty {
                    EVENT_BUS_RESET => {
                        if ev.len() >= 36 {
                            self.generation =
                                u32::from_ne_bytes([ev[32], ev[33], ev[34], ev[35]]);
                        }
                    }
                    EVENT_RESPONSE => {
                        if ev.len() < 20 {
                            continue;
                        }
                        let ev_closure =
                            u64::from_ne_bytes(ev[0..8].try_into().unwrap());
                        if ev_closure != closure {
                            continue;
                        }
                        let rcode = u32::from_ne_bytes([ev[12], ev[13], ev[14], ev[15]]);
                        let len = u32::from_ne_bytes([ev[16], ev[17], ev[18], ev[19]]) as usize;
                        match rcode {
                            RCODE_COMPLETE => {
                                let n = len.min(data.len());
                                if n > 0 && ev.len() >= 20 + n {
                                    data[..n].copy_from_slice(&ev[20..20 + n]);
                                }
                                return Ok(n);
                            }
                            RCODE_GENERATION => {
                                retry_generation = true;
                                break;
                            }
                            other => {
                                return Err(io::Error::other(format!(
                                    "FireWire transaction failed: rcode {other:#x}"
                                )));
                            }
                        }
                    }
                    _ => {}
                }
            }
            if retry_generation {
                self.refresh_generation()?;
            }
        }
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "FireWire bus kept resetting",
        ))
    }

    pub fn read_block(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut b = vec![0u8; len];
        let n = self.request(TCODE_READ_BLOCK, offset, &mut b)?;
        b.truncate(n);
        Ok(b)
    }

    pub fn write_block(&mut self, offset: u64, values: &[u32]) -> io::Result<()> {
        let mut b = Vec::with_capacity(values.len() * 4);
        for v in values {
            b.extend_from_slice(&v.to_le_bytes());
        }
        self.request(TCODE_WRITE_BLOCK, offset, &mut b)?;
        Ok(())
    }
}

fn read_hex(path: &std::path::Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    let text = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).unwrap_or(text);
    u64::from_str_radix(text, 16).ok()
}

fn find_ff800_node() -> Option<String> {
    let entries = fs::read_dir("/sys/bus/firewire/devices").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("fw") || !name.contains('.') {
            continue;
        }
        let path = entry.path();
        if read_hex(&path.join("specifier_id")) == Some(RME_OUI)
            && read_hex(&path.join("version")) == Some(FF800_UNIT_VERSION)
        {
            return Some(name.split('.').next()?.to_owned());
        }
    }
    None
}

/// The FF800 needs ~30 ms between meter reads; this helper keeps the duration
/// calculation in one place.
pub fn meter_period() -> Duration {
    Duration::from_millis(30)
}
