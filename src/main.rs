//! FFMix — independent TotalMix FX-style control for the RME Fireface 800.
//! Not affiliated with or endorsed by RME; no RME code or assets are included.

mod app;
mod backend;
mod ff800;
mod fw;
mod model;
mod persist;
mod tray;
mod widgets;

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::sync::Arc;

use app::App;

fn main() -> eframe::Result<()> {
    // winit cannot hide/unhide a native Wayland window (set_visible is a no-op),
    // which would break close-to-tray. Run as an X11/XWayland client by default;
    // FFMIX_NATIVE_WAYLAND=1 keeps the native backend.
    if std::env::var_os("FFMIX_NATIVE_WAYLAND").is_none()
        && std::env::var_os("DISPLAY").is_some()
    {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("ffmix {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "ffmix {} — independent TotalMix-style mixer for the RME Fireface 800\n\
             \n\
             Usage: ffmix [--minimized]\n\
             \n\
             The window can be closed to the system tray. Quit from the tray menu or\n\
             the application menu (⋯ → Quit).\n\
             \n\
             Not affiliated with or endorsed by RME. No RME code or assets are included.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    if std::env::args().any(|a| a == "--probe") {
        probe();
        return Ok(());
    }
    if std::env::args().any(|a| a == "--write-test") {
        write_test();
        return Ok(());
    }

    let _lock = match single_instance() {
        Some(lock) => lock,
        None => {
            eprintln!("ffmix: another instance is already running");
            std::process::exit(1);
        }
    };

    let icon = app_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FFMix — Fireface 800")
            .with_app_id("ffmix")
            .with_inner_size([1600.0, 980.0])
            .with_min_inner_size([980.0, 620.0])
            .with_icon(Arc::new(icon)),
        vsync: true,
        ..Default::default()
    };

    eframe::run_native(
        "FFMix — Fireface 800",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

fn probe() {
    use std::time::Duration;
    match fw::Firewire::open_ff800() {
        Ok(mut dev) => {
            println!("node: /dev/{} generation {}", dev.node, dev.generation());
            for n in 0..5 {
                match dev.read_block(ff800::REG_STATUS, 8) {
                    Ok(raw) if raw.len() >= 8 => {
                        let q0 = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                        let q1 = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
                        let info = ff800::status_info(q0, q1);
                        println!("status[{n}]: q0={q0:#010x} q1={q1:#010x} rate={:?} source={} inputs={:?}",
                            info.rate, info.source,
                            info.inputs.iter().map(|i| (i.label, i.state.label(), i.rate)).collect::<Vec<_>>());
                    }
                    Ok(_) => println!("status[{n}]: short read"),
                    Err(e) => println!("status[{n}]: {e}"),
                }
                match dev.read_block(ff800::REG_METERS, ff800::METER_LEN) {
                    Ok(raw) if raw.len() >= ff800::METER_LEN => {
                        let m = ff800::decode_meters(&raw);
                        let max = |a: &[f32]| a.iter().copied().fold(0.0f32, f32::max);
                        println!("meters[{n}]: in={:.4} play={:.4} out={:.4}",
                            max(&m.input), max(&m.playback), max(&m.output));
                    }
                    Ok(raw) => println!("meters[{n}]: short read {} bytes", raw.len()),
                    Err(e) => println!("meters[{n}]: {e}"),
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Err(e) => println!("open: {e}"),
    }
}

fn write_test() {
    match fw::Firewire::open_ff800() {
        Ok(mut dev) => {
            println!("node /dev/{} generation {}", dev.node, dev.generation());
            let zero = [0u32; ff800::MIXER_BLOCK];
            match dev.write_block(ff800::REG_MIXER + 0x100 * 27, &zero) {
                Ok(()) => println!("mixer block output 28: OK"),
                Err(e) => println!("mixer block output 28: {e}"),
            }
            match dev.write_block(ff800::REG_OUTPUT + 4 * 27, &[0x8000]) {
                Ok(()) => println!("output 28 volume 0 dB: OK"),
                Err(e) => println!("output 28 volume: {e}"),
            }
            match dev.read_block(ff800::REG_STATUS, 8) {
                Ok(raw) if raw.len() >= 8 => println!("status after write: {:02x?}", raw),
                Ok(raw) => println!("short status {:?}", raw),
                Err(e) => println!("status: {e}"),
            }
        }
        Err(e) => println!("open: {e}"),
    }
}

fn single_instance() -> Option<std::fs::File> {
    let path = format!("/tmp/ffmix-{}.lock", unsafe { libc::getuid() });
    let file = OpenOptions::new().create(true).write(true).open(path).ok()?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    (rc == 0).then_some(file)
}

fn app_icon() -> egui::IconData {
    let size = 64usize;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let i = (y * size + x) * 4;
            let dx = (x as i32 - 31).abs().max(0) as u32;
            let dy = (y as i32 - 31).abs().max(0) as u32;
            let inside = dx * dx + dy * dy <= 30 * 30;
            if !inside {
                continue;
            }
            let bg = if y < 32 { 0x24 } else { 0x1d };
            rgba[i] = 0x18;
            rgba[i + 1] = bg;
            rgba[i + 2] = bg + 2;
            rgba[i + 3] = 0xff;
        }
    }
    for (row, y) in [(0usize, 20usize), (1, 32), (2, 44)] {
        for x in 14..50 {
            let i = (y * size + x) * 4;
            rgba[i] = 0x3d;
            rgba[i + 1] = 0x43;
            rgba[i + 2] = 0x4b;
            rgba[i + 3] = 0xff;
        }
        let kx = [23usize, 41, 31][row];
        let (r, g, b) = [(0x4f, 0xb8, 0xa4), (0xe3, 0xb3, 0x41), (0x4c, 0xc2, 0x7a)][row];
        for yy in y.saturating_sub(4)..=(y + 4).min(size - 1) {
            for xx in kx.saturating_sub(4)..=(kx + 4).min(size - 1) {
                let i = (yy * size + xx) * 4;
                rgba[i] = r;
                rgba[i + 1] = g;
                rgba[i + 2] = b;
                rgba[i + 3] = 0xff;
            }
        }
    }
    egui::IconData { rgba, width: size as u32, height: size as u32 }
}
