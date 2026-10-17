//! StatusNotifierItem tray icon (KDE/waybar/GNOME AppIndicator).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray, TrayService};

#[derive(Clone, Debug)]
pub enum UiCmd {
    MuteAll,
    ShowTab(u8),
    Quit,
}

pub fn spawn(tx: Sender<UiCmd>, ctx: egui::Context, visible: Arc<AtomicBool>) -> bool {
    let tray = MixerTray { tx, ctx, visible };
    TrayService::new(tray).spawn();
    true
}

struct MixerTray {
    tx: Sender<UiCmd>,
    ctx: egui::Context,
    visible: Arc<AtomicBool>,
}

impl MixerTray {
    fn toggle(&mut self) {
        let show = !self.visible.load(Ordering::Relaxed);
        self.visible.store(show, Ordering::Relaxed);
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(show));
        if show {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.ctx.request_repaint();
    }

    fn show(&mut self) {
        self.visible.store(true, Ordering::Relaxed);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.ctx.request_repaint();
    }
}

impl Tray for MixerTray {
    fn id(&self) -> String {
        "ffmix".to_owned()
    }

    fn title(&self) -> String {
        "FFMix — Fireface 800".to_owned()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![icon32()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.toggle();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Show / hide window".to_owned(),
                activate: Box::new(|t: &mut Self| t.toggle()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Mixer".to_owned(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send(UiCmd::ShowTab(0));
                    t.show();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Matrix".to_owned(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send(UiCmd::ShowTab(1));
                    t.show();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Settings".to_owned(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send(UiCmd::ShowTab(2));
                    t.show();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Mute all outputs".to_owned(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send(UiCmd::MuteAll);
                    t.show();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".to_owned(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send(UiCmd::Quit);
                    t.ctx.request_repaint();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn icon32() -> Icon {
    let size = 32usize;
    fn put(data: &mut [u8], size: usize, x: usize, y: usize, a: u8, r: u8, g: u8, b: u8) {
        let i = (y * size + x) * 4;
        data[i] = a;
        data[i + 1] = r;
        data[i + 2] = g;
        data[i + 3] = b;
    }
    let mut data = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            // rounded square
            let dx = (x as i32 - 15).abs().max(0) as u32;
            let dy = (y as i32 - 15).abs().max(0) as u32;
            let outside = dx * dx + dy * dy > 14 * 14;
            if outside {
                put(&mut data, size, x, y, 0, 0, 0, 0);
            } else {
                let shade = if y < 16 { 0x24 } else { 0x1e };
                put(&mut data, size, x, y, 0xff, 0x18, shade + 2, 0x1c);
            }
        }
    }
    // three horizontal fader tracks
    for (row, y) in [(0usize, 10usize), (1, 16), (2, 22)] {
        for x in 7..25 {
            put(&mut data, size, x, y, 0xff, 0x3d, 0x43, 0x4b);
        }
        let knob_x = [10usize, 19, 14][row];
        let (r, g, b) = [(0x4f, 0xb8, 0xa4), (0xe3, 0xb3, 0x41), (0x4c, 0xc2, 0x7a)][row];
        for yy in y.saturating_sub(2)..=(y + 2).min(31) {
            for xx in knob_x.saturating_sub(2)..=(knob_x + 2).min(31) {
                put(&mut data, size, xx, yy, 0xff, r, g, b);
            }
        }
    }
    Icon { width: size as i32, height: size as i32, data }
}
