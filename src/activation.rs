//! Single-instance activation.
//!
//! The first instance binds a Unix socket; a later launch connects and asks it
//! to show the window instead of dying behind a lock. That matters when the
//! window is hidden in the tray and the user starts the app again.

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

pub enum Instance {
    Primary(UnixListener),
    Raised,
}

pub fn path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    // SAFETY: getuid takes no arguments, has no preconditions and cannot fail.
    dir.join(format!("ffmix-{}.sock", unsafe { libc::getuid() }))
}

pub fn claim() -> Instance {
    let path = path();
    if let Ok(listener) = bind(&path) {
        return Instance::Primary(listener);
    }
    if raise(&path) {
        return Instance::Raised;
    }
    // A crashed instance leaves the socket file behind; it accepts no
    // connection, so replace it.
    let _ = std::fs::remove_file(&path);
    match bind(&path) {
        Ok(listener) => Instance::Primary(listener),
        Err(err) => {
            eprintln!("ffmix: cannot own {}: {err}", path.display());
            std::process::exit(1);
        }
    }
}

fn bind(path: &std::path::Path) -> std::io::Result<UnixListener> {
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn raise(path: &std::path::Path) -> bool {
    match UnixStream::connect(path) {
        Ok(mut stream) => stream.write_all(b"show\n").is_ok(),
        Err(_) => false,
    }
}

pub fn remove_socket() {
    let _ = std::fs::remove_file(path());
}
