//! egui application: mixer, matrix and settings views, plus tray glue.
//! Independent interoperability project; not affiliated with RME.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::backend::{Backend, Command, Event};
use crate::ff800::{
    ClockSource, DeviceSettings, InLevel, Jack, MeterFrame, OutLevel, StatusInfo, SyncState,
};
use crate::model::{
    chan_label, channels, db2pos, fmt_db, lin2db, pair_label, speed_mode, Kind, Mix, MixState,
    PresetBank, N_IN, N_OUT, N_PAIRS, N_PLAY, N_SLOTS, N_SRC,
};
use crate::persist::{self, AppConfig};
use crate::tray::{self, UiCmd};
use crate::widgets::{
    self, ACCENT, AMBER, BG, BLUE, BORDER, DIMTEXT, GREEN, MUTED, PANEL, PANEL2, PANEL3, RED, TEXT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Mixer,
    Matrix,
    Settings,
}

impl Tab {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Matrix,
            2 => Self::Settings,
            _ => Self::Mixer,
        }
    }
    fn as_u8(self) -> u8 {
        match self {
            Self::Mixer => 0,
            Self::Matrix => 1,
            Self::Settings => 2,
        }
    }
}

#[derive(Clone)]
struct MeterBank {
    level: [f32; N_OUT],
    hold: [f32; N_OUT],
    clip: [bool; N_OUT],
    hold_at: [Instant; N_OUT],
}

impl Default for MeterBank {
    fn default() -> Self {
        Self {
            level: [0.0; N_OUT],
            hold: [0.0; N_OUT],
            clip: [false; N_OUT],
            hold_at: [Instant::now(); N_OUT],
        }
    }
}

impl MeterBank {
    fn push(&mut self, values: &[f32]) {
        let now = Instant::now();
        for (i, value) in values.iter().enumerate().take(N_OUT) {
            let v = value.clamp(0.0, 2.0);
            if v > self.level[i] {
                self.level[i] = v;
            }
            if v >= 0.999 {
                self.clip[i] = true;
            }
            if self.level[i] >= self.hold[i] || now.duration_since(self.hold_at[i]).as_secs_f32() > 1.5 {
                self.hold[i] = self.level[i];
                self.hold_at[i] = now;
            }
        }
    }

    fn tick(&mut self, dt: f32) {
        let fall = 10f32.powf(-26.0 * dt / 20.0);
        let now = Instant::now();
        for i in 0..N_OUT {
            self.level[i] *= fall;
            if self.level[i] < 1e-6 {
                self.level[i] = 0.0;
            }
            if now.duration_since(self.hold_at[i]).as_secs_f32() > 1.5 {
                self.hold[i] = self.level[i];
                self.hold_at[i] = now;
            }
        }
    }

    fn reset(&mut self, channel: usize) {
        if channel < N_OUT {
            self.hold[channel] = 0.0;
            self.clip[channel] = false;
            self.hold_at[channel] = Instant::now();
        }
    }

    fn reset_pair(&mut self, pair: usize) {
        self.reset(pair * 2);
        self.reset(pair * 2 + 1);
    }

    fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot {
            level: self.level,
            hold: self.hold,
            clip: self.clip,
        }
    }
}

#[derive(Clone, Copy)]
struct MeterSnapshot {
    level: [f32; N_OUT],
    hold: [f32; N_OUT],
    clip: [bool; N_OUT],
}

struct Meters {
    input: MeterBank,
    playback: MeterBank,
    output: MeterBank,
    last_frame: Instant,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            input: MeterBank::default(),
            playback: MeterBank::default(),
            output: MeterBank::default(),
            last_frame: Instant::now(),
        }
    }
}

impl Meters {
    fn push(&mut self, frame: &MeterFrame) {
        self.input.push(&frame.input);
        self.playback.push(&frame.playback);
        self.output.push(&frame.output);
    }

    fn tick(&mut self, now: Instant) {
        let dt = now.duration_since(self.last_frame).as_secs_f32().clamp(0.0, 0.1);
        self.last_frame = now;
        if dt > 0.0 {
            self.input.tick(dt);
            self.playback.tick(dt);
            self.output.tick(dt);
        }
    }

    fn bank(&self, kind: Kind) -> &MeterBank {
        match kind {
            Kind::In => &self.input,
            Kind::Play => &self.playback,
            Kind::Out => &self.output,
        }
    }
}

pub struct App {
    backend: Backend,
    mix: MixState,
    settings: DeviceSettings,
    status: StatusInfo,
    meters: Meters,
    node: Option<String>,
    conn_error: Option<String>,
    connected: bool,
    tab: Tab,
    matrix_kind: Kind,
    presets: PresetBank,
    config: AppConfig,
    active_slot: Option<usize>,
    store_armed: bool,
    solo_memory: Option<([bool; N_IN], [bool; N_PLAY])>,
    dim: bool,
    main_mute: bool,
    ui_rx: Receiver<UiCmd>,
    visible: Arc<AtomicBool>,
    hidden: bool,
    quit: bool,
    start_hidden_done: bool,
    dirty_save: bool,
    last_save: Instant,
    mix_dirty: bool,
    settings_dirty: bool,
    last_sync: Instant,
    toast: Option<(String, Instant)>,
    matrix_selected: Option<(usize, usize)>,
    last_frame: Instant,
    tray_ok: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        widgets::apply_theme(&cc.egui_ctx);

        let mut config = persist::load_config();
        let start_hidden = config.start_hidden || std::env::args().any(|a| a == "--minimized");
        if start_hidden {
            config.start_hidden = true;
        }
        if let Some(size) = config.window_size {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(size[0], size[1])));
        }
        if config.window_maximized {
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }

        let mix = persist::load_mix();
        let presets = persist::load_presets();
        let settings = persist::load_settings();
        let backend = Backend::spawn();
        backend.set_settings(&settings);
        backend.set_mix(&mix);

        let (ui_tx, ui_rx) = mpsc::channel();
        let visible = Arc::new(AtomicBool::new(!start_hidden));
        let tray_ok = tray::spawn(ui_tx, cc.egui_ctx.clone(), visible.clone());

        Self {
            backend,
            mix,
            settings,
            status: StatusInfo::default(),
            meters: Meters::default(),
            node: None,
            conn_error: None,
            connected: false,
            tab: Tab::from_u8(config.tab),
            matrix_kind: if config.matrix_kind == 1 { Kind::Play } else { Kind::In },
            presets,
            config,
            active_slot: None,
            store_armed: false,
            solo_memory: None,
            dim: false,
            main_mute: false,
            ui_rx,
            visible,
            hidden: false,
            quit: false,
            start_hidden_done: !start_hidden,
            dirty_save: false,
            last_save: Instant::now(),
            mix_dirty: false,
            settings_dirty: false,
            last_sync: Instant::now(),
            toast: None,
            matrix_selected: None,
            last_frame: Instant::now(),
            tray_ok,
        }
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn mark_mix(&mut self) {
        self.mix_dirty = true;
        self.dirty_save = true;
    }

    fn mark_settings(&mut self) {
        self.settings.known = true;
        self.settings_dirty = true;
        self.dirty_save = true;
    }

    fn mark_config(&mut self) {
        self.dirty_save = true;
    }

    fn backend_mix(&self) -> MixState {
        let mut m = self.mix.clone();
        let main = self.config.main_pair.min(N_PAIRS - 1);
        if self.dim {
            if let Some(gain) = m.out[main].gain {
                m.out[main].gain = Some(gain - 20.0);
            }
        }
        if self.main_mute {
            m.out[main].mute = true;
        }
        m
    }

    fn sync_now(&mut self) {
        self.backend.set_mix(&self.backend_mix());
        if self.settings_dirty {
            self.backend.set_settings(&self.settings);
            self.settings_dirty = false;
        }
        self.mix_dirty = false;
        self.last_sync = Instant::now();
    }

    fn save_all(&mut self) {
        persist::save_mix(&self.mix);
        persist::save_presets(&self.presets);
        persist::save_settings(&self.settings);
        persist::save_config(&self.config);
        self.dirty_save = false;
        self.last_save = Instant::now();
    }

    fn store_slot(&mut self, slot: usize, silent: bool) {
        if slot >= N_SLOTS {
            return;
        }
        self.presets.slots[slot] = Some(Mix::from_state(&self.mix));
        self.active_slot = Some(slot);
        self.store_armed = false;
        self.dirty_save = true;
        if !silent {
            self.toast(format!("Stored preset {}", slot + 1));
        }
    }

    fn recall_slot(&mut self, slot: usize) {
        if slot >= N_SLOTS {
            return;
        }
        let Some(preset) = self.presets.slots[slot].clone() else {
            self.store_slot(slot, false);
            return;
        };
        preset.apply_to(&mut self.mix);
        self.active_slot = Some(slot);
        self.solo_memory = None;
        self.mark_mix();
        self.toast(format!("Recalled preset {}", slot + 1));
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.visible.store(true, Ordering::Relaxed);
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        self.visible.store(false, Ordering::Relaxed);
        self.hidden = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn handle_ui_cmd(&mut self, cmd: UiCmd, ctx: &egui::Context) {
        match cmd {
            UiCmd::MuteAll => {
                let any_live = self.mix.out.iter().any(|o| !o.mute);
                for out in &mut self.mix.out {
                    out.mute = any_live;
                }
                self.mark_mix();
                self.toast(if any_live { "All outputs muted" } else { "All outputs unmuted" });
            }
            UiCmd::ShowTab(tab) => {
                self.tab = Tab::from_u8(tab);
                self.config.tab = self.tab.as_u8();
                self.mark_config();
                self.show_window(ctx);
            }
            UiCmd::Quit => {
                self.quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Connection { node, error } => {
                self.connected = node.is_some();
                if let Some(node) = node {
                    self.node = Some(node.clone());
                    self.conn_error = None;
                    self.toast(format!("Connected to {node}"));
                } else if let Some(err) = error {
                    self.conn_error = Some(err);
                }
            }
            Event::Status(status) => {
                self.status = status;
            }
            Event::Settings(settings) => {
                // Status-derived settings arrive only on first connect (known == false).
                if !self.settings.known {
                    self.settings = settings;
                    self.dirty_save = true;
                    self.settings_dirty = true;
                }
            }
            Event::FlashSettings(settings) => {
                self.settings = settings;
                self.settings.known = true;
                self.mark_settings();
                self.toast("Loaded saved settings from device flash");
            }
            Event::FlashMixer(mixer) => {
                self.mix.sends_in = mixer.sends_in;
                self.mix.sends_play = mixer.sends_play;
                for (out, saved) in self.mix.out.iter_mut().zip(mixer.out) {
                    out.gain = saved.gain;
                }
                self.mark_mix();
                self.toast("Loaded saved mixer from device flash");
            }
            Event::FlashError(err) => {
                self.toast(format!("Flash read failed: {err}"));
            }
            Event::Meters(frame) => {
                self.meters.push(&frame);
            }
            Event::Error(err) => {
                self.conn_error = Some(err);
            }
        }
    }

    // ------------------------------------------------------------ top bar

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::NONE
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 5))
                    .stroke(Stroke::new(1.0_f32, BORDER)),
            )
            .show(ctx, |ui| {
                // row 1: identity, view tabs, device state
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("FFMix")
                            .size(17.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.label(
                        egui::RichText::new("Fireface 800")
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(14.0);
                    for (tab, label) in [
                        (Tab::Mixer, "MIXER"),
                        (Tab::Matrix, "MATRIX"),
                        (Tab::Settings, "SETTINGS"),
                    ] {
                        if widgets::tab_button(ui, label, self.tab == tab).clicked() {
                            self.tab = tab;
                            self.config.tab = tab.as_u8();
                            self.mark_config();
                        }
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button("⋯").clicked() {
                                ui.memory_mut(|m| m.toggle_popup(Id::new("app_menu")));
                            }
                            egui::popup_below_widget(
                                ui,
                                Id::new("app_menu"),
                                &ui.response(),
                                egui::PopupCloseBehavior::CloseOnClick,
                                |ui| {
                                    ui.set_min_width(190.0);
                                    if ui.button("Rescan FireWire device").clicked() {
                                        self.backend.send(Command::Rescan);
                                        self.toast("Rescanning /dev/fw*");
                                        ui.close_menu();
                                    }
                                    if ui.button("Reset all peak meters").clicked() {
                                        for i in 0..N_OUT {
                                            self.meters.input.reset(i);
                                            self.meters.playback.reset(i);
                                            self.meters.output.reset(i);
                                        }
                                        ui.close_menu();
                                    }
                                    if ui.button("Mute all outputs").clicked() {
                                        let any_live = self.mix.out.iter().any(|o| !o.mute);
                                        for out in &mut self.mix.out {
                                            out.mute = any_live;
                                        }
                                        self.mark_mix();
                                        ui.close_menu();
                                    }
                                    if ui.button("Open config folder").clicked() {
                                        let _ = std::process::Command::new("xdg-open")
                                            .arg(persist::config_dir())
                                            .spawn();
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if ui.button("Quit").clicked() {
                                        self.quit = true;
                                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                        ui.close_menu();
                                    }
                                },
                            );
                            ui.add_space(6.0);
                            let (label, dot) = if self.connected {
                                (
                                    format!(
                                        "{}  ·  {}",
                                        rate_text(self.status.rate),
                                        self.status.source
                                    ),
                                    GREEN,
                                )
                            } else {
                                ("no device".to_owned(), RED)
                            };
                            ui.label(egui::RichText::new(label).size(10.5).color(MUTED));
                            let (r, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, dot);
                        },
                    );
                });
                ui.add_space(3.0);
                // row 2: submix + snapshots + solo
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("SUBMIX")
                            .size(9.0)
                            .color(MUTED),
                    );
                    let mode = speed_mode(self.status.rate);
                    let present = channels(Kind::Out, mode).len() / 2;
                    let mut selected = self.mix.selected;
                    egui::ComboBox::from_id_salt("submix")
                        .width(132.0)
                        .selected_text(pair_label(Kind::Out, selected.min(N_PAIRS - 1)))
                        .show_ui(ui, |ui| {
                            for pair in 0..present.min(N_PAIRS) {
                                ui.selectable_value(
                                    &mut selected,
                                    pair,
                                    pair_label(Kind::Out, pair),
                                );
                            }
                        });
                    if selected != self.mix.selected {
                        self.mix.selected = selected;
                        self.mark_mix();
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "inputs and playback feed {}",
                            pair_label(Kind::Out, self.mix.selected.min(N_PAIRS - 1))
                        ))
                        .size(9.5)
                        .color(DIMTEXT),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button("STORE").clicked() {
                                self.store_armed = true;
                                self.toast("Click a preset slot to store the current mix");
                            }
                            for slot in (0..N_SLOTS).rev() {
                                let (rect, resp) = ui
                                    .allocate_exact_size(Vec2::new(23.0, 22.0), Sense::click());
                                let filled = self.presets.slots[slot].is_some();
                                let active = self.active_slot == Some(slot);
                                let hovered = resp.hovered();
                                let fill = if active {
                                    AMBER
                                } else if filled {
                                    if hovered { PANEL3 } else { PANEL2 }
                                } else if hovered {
                                    PANEL2
                                } else {
                                    PANEL
                                };
                                ui.painter().rect_filled(rect, 4.0, fill);
                                ui.painter().rect_stroke(
                                    rect,
                                    4.0,
                                    Stroke::new(1.0_f32, if active { AMBER } else { BORDER }),
                                    StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    format!("{}", slot + 1),
                                    FontId::proportional(10.5),
                                    if active {
                                        Color32::from_rgb(0x18, 0x1a, 0x1c)
                                    } else if filled {
                                        TEXT
                                    } else {
                                        MUTED
                                    },
                                );
                                if resp.clicked() {
                                    if self.store_armed {
                                        self.store_slot(slot, false);
                                    } else {
                                        self.recall_slot(slot);
                                    }
                                }
                                if resp.secondary_clicked() {
                                    self.store_slot(slot, false);
                                }
                            }
                            ui.add_space(4.0);
                            let any_solo = self.mix.any_solo();
                            let (rect, resp) = ui
                                .allocate_exact_size(Vec2::new(48.0, 22.0), Sense::click());
                            let fill = if any_solo { AMBER } else { PANEL2 };
                            ui.painter().rect_filled(rect, 4.0, fill);
                            ui.painter().rect_stroke(
                                rect,
                                4.0,
                                Stroke::new(1.0_f32, if any_solo { AMBER } else { BORDER }),
                                StrokeKind::Inside,
                            );
                            ui.painter().text(
                                rect.center(),
                                Align2::CENTER_CENTER,
                                "SOLO",
                                FontId::proportional(10.0),
                                if any_solo { Color32::from_rgb(0x18, 0x1a, 0x1c) } else { MUTED },
                            );
                            if resp.clicked() {
                                if any_solo {
                                    self.solo_memory =
                                        Some((self.mix.solo_in, self.mix.solo_play));
                                    self.mix.clear_solo();
                                } else if let Some((i, p)) = self.solo_memory.take() {
                                    self.mix.solo_in = i;
                                    self.mix.solo_play = p;
                                }
                                self.mark_mix();
                            }
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("PRESETS")
                                    .size(9.0)
                                    .color(MUTED),
                            );
                        },
                    );
                });
            });
    }

    // ------------------------------------------------------------ mixer

    fn mixer_view(&mut self, ui: &mut egui::Ui) {
        let (avail, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let gap = 6.0;
        // Keep the mixer readable on very large monitors: cap the content width and
        // give each row a sane height instead of stretching faders over the screen.
        let content_w = (avail.width() - 6.0).min(2000.0).max(320.0);
        let x0 = avail.left() + (avail.width() - content_w).max(0.0) * 0.5 + 3.0;
        let row_h = ((avail.height() - 2.0 * gap) / 3.0).clamp(140.0, 380.0);
        let total_h = row_h * 3.0 + gap * 2.0;
        let y0 = avail.top() + ((avail.height() - total_h).max(0.0)) * 0.5;
        let mode = speed_mode(self.status.rate);
        for (i, kind) in [Kind::In, Kind::Play, Kind::Out].into_iter().enumerate() {
            let row = Rect::from_min_size(
                Pos2::new(x0, y0 + i as f32 * (row_h + gap)),
                Vec2::new(content_w, row_h),
            );
            self.draw_row(ui, row, kind, mode);
        }
        ui.advance_cursor_after_rect(avail);
    }

    fn draw_row(&mut self, ui: &mut egui::Ui, rect: Rect, kind: Kind, mode: u32) {
        let painter = ui.painter();
        painter.rect_filled(rect, 7.0, PANEL);
        painter.rect_stroke(rect, 7.0, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);

        let header_w = 86.0;
        let header = Rect::from_min_size(rect.min + Vec2::new(1.0, 1.0), Vec2::new(header_w, rect.height() - 2.0));
        painter.rect_filled(header, 6.0, blend(PANEL2, widgets::kind_color(kind), 0.10));
        painter.rect_filled(
            Rect::from_min_size(header.min + Vec2::new(0.0, 8.0), Vec2::new(4.0, header.height() - 16.0)),
            1.0,
            widgets::kind_color(kind),
        );
        let title = match kind {
            Kind::In => "INPUTS",
            Kind::Play => "PLAYBACK",
            Kind::Out => "OUTPUTS",
        };
        painter.text(
            header.min + Vec2::new(12.0, 10.0),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(10.5),
            widgets::kind_color(kind),
        );
        let sub = match kind {
            Kind::In | Kind::Play => format!(
                "feed ▸ {}",
                pair_label(Kind::Out, self.mix.selected.min(N_PAIRS - 1))
            ),
            Kind::Out => "master · click a name".to_owned(),
        };
        painter.text(
            header.min + Vec2::new(12.0, 26.0),
            Align2::LEFT_TOP,
            sub,
            FontId::proportional(8.5),
            MUTED,
        );
        // Shared fader scale along the right edge of the row header.
        let scale_top = rect.top() + 3.0 + if kind == Kind::Out { 37.0 } else { 72.0 };
        let scale_bottom = rect.bottom() - 3.0 - 12.0;
        if scale_bottom > scale_top + 40.0 {
            for db in [6.0, 0.0, -6.0, -12.0, -24.0, -40.0, -60.0] {
                let y = scale_bottom - db2pos(Some(db)) * (scale_bottom - scale_top);
                painter.text(
                    Pos2::new(header.right() - 8.0, y),
                    Align2::RIGHT_CENTER,
                    if db > 0.0 { format!("+{db:.0}") } else { format!("{db:.0}") },
                    FontId::proportional(7.0),
                    if db == 0.0 { TEXT } else { DIMTEXT },
                );
            }
        }
        let chans = channels(kind, mode);
        let nchan = chans.len().max(1);
        let strips_w = rect.width() - header_w - 8.0;
        let unit = (strips_w / nchan as f32).clamp(22.0, 72.0);
        let mut x = header.right() + 4.0;
        let top = rect.top() + 3.0;
        let bottom = rect.bottom() - 3.0;

        if kind == Kind::Out {
            let pairs = chans.len() / 2;
            for pair in 0..pairs {
                let w = unit * 2.0;
                if x + w > rect.right() - 2.0 {
                    break;
                }
                let strip = Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + w - 2.0, bottom));
                self.draw_strip(ui, strip, kind, pair, &[chans[2 * pair], chans[2 * pair + 1]]);
                x += w;
            }
        } else {
            let mut i = 0usize;
            while i < chans.len() {
                let c = chans[i];
                let linked = c % 2 == 0
                    && self.stereo(kind)[c / 2]
                    && chans.get(i + 1) == Some(&(c + 1));
                let take = if linked { 2 } else { 1 };
                if x + unit * take as f32 > rect.right() - 2.0 {
                    break;
                }
                let strip = Rect::from_min_max(
                    Pos2::new(x, top),
                    Pos2::new(x + unit * take as f32 - 2.0, bottom),
                );
                let cs: Vec<usize> = chans[i..i + take].to_vec();
                self.draw_strip(ui, strip, kind, c, &cs);
                x += unit * take as f32;
                i += take;
            }
        }
    }

    fn stereo(&self, kind: Kind) -> &[bool; N_PAIRS] {
        match kind {
            Kind::In => &self.mix.stereo_in,
            Kind::Play => &self.mix.stereo_play,
            Kind::Out => &self.mix.stereo_in,
        }
    }

    fn draw_strip(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        kind: Kind,
        idx: usize,
        chans: &[usize],
    ) {
        let pair = if kind == Kind::Out { idx } else { chans[0] / 2 };
        let stereo_strip = kind == Kind::Out || chans.len() == 2;
        let name = if kind == Kind::Out {
            pair_label(Kind::Out, pair)
        } else if chans.len() == 2 {
            pair_label(kind, pair)
        } else {
            chan_label(kind, chans[0])
        };
        let key = MixState::strip_key(kind, idx);
        let group = self.mix.groups.get(&key).copied();
        let selected = kind == Kind::Out && pair == self.mix.selected;
        let color = widgets::kind_color(kind);
        let painter = ui.painter().with_clip_rect(rect);

        let hover = ui
            .interact(rect, Id::new(("strip", kind, idx)), Sense::click());
        let is_hovered = hover.hovered();
        let hover = hover.on_hover_text(format!(
                "{name}\n{} / {}\n{}",
                match kind {
                    Kind::In => "hardware input",
                    Kind::Play => "software playback",
                    Kind::Out => "hardware output",
                },
                if kind == Kind::Out {
                    format!("master {}", fmt_db(self.mix.out[pair].gain))
                } else {
                    format!("send {}", fmt_db(self.mix.strip_db(kind, idx)))
                },
                group.map(|g| format!("Fader group {g}")).unwrap_or_default()
            ));
        let clicked = hover.clicked();
        if kind == Kind::Out && clicked {
            self.mix.selected = pair;
            self.mark_mix();
        }
        let mut group_choice: Option<Option<u8>> = None;
        hover.context_menu(|ui| {
            ui.label(egui::RichText::new(&name).strong());
            if ui.button("Fader group 1").clicked() {
                group_choice = Some(Some(1));
                ui.close_menu();
            }
            if ui.button("Fader group 2").clicked() {
                group_choice = Some(Some(2));
                ui.close_menu();
            }
            if ui.button("Fader group 3").clicked() {
                group_choice = Some(Some(3));
                ui.close_menu();
            }
            if ui.button("Fader group 4").clicked() {
                group_choice = Some(Some(4));
                ui.close_menu();
            }
            if ui.button("No group").clicked() {
                group_choice = Some(None);
                ui.close_menu();
            }
        });
        if let Some(g) = group_choice {
            match g {
                Some(g) => {
                    self.mix.groups.insert(key.clone(), g);
                }
                None => {
                    self.mix.groups.remove(&key);
                }
            }
            self.mark_mix();
        }

        let base = blend(Color32::from_rgb(0x2b, 0x2f, 0x34), color, 0.07);
        let fill = if selected {
            blend(base, AMBER, 0.13)
        } else if is_hovered {
            blend(base, Color32::WHITE, 0.055)
        } else {
            base
        };
        painter.rect_filled(rect, 4.0, fill);
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(if selected { 1.8_f32 } else { 1.0_f32 }, if selected { AMBER } else { BORDER }),
            StrokeKind::Inside,
        );
        // kind colour cap + tinted name band
        let band = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 20.0));
        painter.rect_filled(band, 3.0, blend(Color32::from_rgb(0x20, 0x24, 0x28), color, 0.24));
        painter.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 2.0)),
            2.0,
            color,
        );
        painter.text(
            Pos2::new(rect.center().x, rect.top() + 5.0),
            Align2::CENTER_TOP,
            &name,
            FontId::proportional(if name.len() > 8 { 8.2 } else { 9.3 }),
            blend(color, Color32::WHITE, 0.62),
        );
        if let Some(g) = group {
            painter.text(
                Pos2::new(rect.right() - 4.0, rect.top() + 5.0),
                Align2::RIGHT_TOP,
                format!("G{g}"),
                FontId::proportional(7.5),
                AMBER,
            );
        }

        let btn_y = rect.top() + 20.0;
        let btn_h = 13.0;
        let btn_w = ((rect.width() - 6.0) / 3.0).min(15.0).max(9.0);
        let mut bx = rect.left() + 2.0;
        let muted = match kind {
            Kind::In => chans.iter().any(|&c| self.mix.mute_in[c]),
            Kind::Play => chans.iter().any(|&c| self.mix.mute_play[c]),
            Kind::Out => self.mix.out[pair].mute,
        };
        let soloed = match kind {
            Kind::In => chans.iter().any(|&c| self.mix.solo_in[c]),
            Kind::Play => chans.iter().any(|&c| self.mix.solo_play[c]),
            Kind::Out => false,
        };
        if widgets::small_toggle(
            ui,
            Id::new(("mute", kind, idx)),
            Rect::from_min_size(Pos2::new(bx, btn_y), Vec2::new(btn_w, btn_h)),
            "M",
            muted,
            RED,
        )
        .clicked()
        {
            match kind {
                Kind::In => {
                    let new = !chans.iter().any(|&c| self.mix.mute_in[c]);
                    for &c in chans {
                        self.mix.mute_in[c] = new;
                    }
                }
                Kind::Play => {
                    let new = !chans.iter().any(|&c| self.mix.mute_play[c]);
                    for &c in chans {
                        self.mix.mute_play[c] = new;
                    }
                }
                Kind::Out => self.mix.out[pair].mute = !self.mix.out[pair].mute,
            }
            self.mark_mix();
        }
        bx += btn_w + 2.0;
        if kind != Kind::Out {
            if widgets::small_toggle(
                ui,
                Id::new(("solo", kind, idx)),
                Rect::from_min_size(Pos2::new(bx, btn_y), Vec2::new(btn_w, btn_h)),
                "S",
                soloed,
                AMBER,
            )
            .clicked()
            {
                let new = !soloed;
                match kind {
                    Kind::In => {
                        for &c in chans {
                            self.mix.solo_in[c] = new;
                        }
                    }
                    Kind::Play => {
                        for &c in chans {
                            self.mix.solo_play[c] = new;
                        }
                    }
                    Kind::Out => {}
                }
                self.solo_memory = None;
                self.mark_mix();
            }
            bx += btn_w + 2.0;
            if widgets::small_toggle(
                ui,
                Id::new(("st", kind, idx)),
                Rect::from_min_size(Pos2::new(bx, btn_y), Vec2::new(btn_w, btn_h)),
                "ST",
                stereo_strip,
                BLUE,
            )
            .clicked()
            {
                let p = chans[0] / 2;
                match kind {
                    Kind::In => self.mix.stereo_in[p] = !self.mix.stereo_in[p],
                    Kind::Play => self.mix.stereo_play[p] = !self.mix.stereo_play[p],
                    Kind::Out => {}
                }
                self.mark_mix();
            }
        }

        let mut top_of_fader = btn_y + btn_h + 4.0;
        if kind != Kind::Out {
            let knob_rect = Rect::from_center_size(
                Pos2::new(rect.center().x, top_of_fader + 10.0),
                Vec2::new(22.0, 22.0),
            );
            let mut pan = self.mix.strip_pan(kind, idx);
            if widgets::knob(ui, Id::new(("pan", kind, idx)), &mut pan, knob_rect) {
                self.mix.set_strip_pan(kind, idx, pan);
                self.mark_mix();
            }
            let pan_value = self.mix.strip_pan(kind, idx);
            painter.text(
                Pos2::new(rect.center().x, top_of_fader + 23.0),
                Align2::CENTER_TOP,
                widgets::knob_text(pan_value),
                FontId::proportional(8.0),
                if pan_value.abs() > 0.01 { BLUE } else { MUTED },
            );
            top_of_fader += 35.0;
        }

        let meter_w = if rect.width() >= 48.0 {
            20.0
        } else if rect.width() >= 35.0 {
            16.0
        } else {
            12.0
        };
        let fader_w = (rect.width() - meter_w - 6.0).clamp(13.0, 21.0);
        let fader_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 2.0, top_of_fader),
            Pos2::new(rect.left() + 2.0 + fader_w, rect.bottom() - 12.0),
        );
        let mut db = self.mix.strip_db(kind, idx);
        if widgets::fader(ui, Id::new(("fader", kind, idx)), &mut db, fader_rect, true) {
            let old = self.mix.strip_db(kind, idx);
            self.mix.set_strip_db(kind, idx, db);
            let changed = self.mix.group_follow(&key, old, db);
            let _ = changed;
            self.mark_mix();
        }
        painter.text(
            Pos2::new(fader_rect.center().x, rect.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            fmt_db(self.mix.strip_db(kind, idx)),
            FontId::proportional(8.4),
            MUTED,
        );

        let meter_rect = Rect::from_min_max(
            Pos2::new(rect.right() - meter_w - 2.0, top_of_fader),
            Pos2::new(rect.right() - 2.0, rect.bottom() - 12.0),
        );
        let bank = self.meters.bank(kind);
        let n = if stereo_strip { 2 } else { 1 };
        let mut levels = [0.0f32; 2];
        let mut holds = [0.0f32; 2];
        let mut clips = [false; 2];
        for (j, &c) in chans.iter().take(n).enumerate() {
            if c < N_OUT {
                levels[j] = bank.level[c];
                holds[j] = bank.hold[c];
                clips[j] = bank.clip[c];
            }
        }
        if widgets::meter(
            ui,
            Id::new(("meter", kind, idx)),
            &levels[..n],
            &holds[..n],
            &clips[..n],
            meter_rect,
        ) {
            for &c in chans.iter().take(n) {
                self.meters.input.reset(c);
                self.meters.playback.reset(c);
                self.meters.output.reset(c);
            }
        }
        if rect.width() >= 42.0 {
            let peak = levels[..n].iter().copied().fold(0.0f32, f32::max);
            let peak_db = lin2db(peak);
            painter.text(
                Pos2::new(rect.right() - 2.0, rect.bottom() - 2.0),
                Align2::RIGHT_BOTTOM,
                fmt_db(peak_db),
                FontId::proportional(7.2),
                if clips[..n].iter().any(|&c| c) {
                    RED
                } else if peak_db.map(|d| d > -6.0).unwrap_or(false) {
                    AMBER
                } else {
                    DIMTEXT
                },
            );
        }
    }

    // ------------------------------------------------------------ matrix

    fn matrix_view(&mut self, ui: &mut egui::Ui) {
        let live_any = self
            .meters
            .input
            .level
            .iter()
            .chain(self.meters.playback.level.iter())
            .chain(self.meters.output.level.iter())
            .any(|&v| widgets::meter_frac(v) > 0.08);
        let mode = speed_mode(self.status.rate);
        ui.horizontal(|ui| {
            for (kind, label) in [(Kind::In, "INPUTS"), (Kind::Play, "PLAYBACK")] {
                if widgets::tab_button(ui, label, self.matrix_kind == kind).clicked() {
                    self.matrix_kind = kind;
                    self.config.matrix_kind = if kind == Kind::Play { 1 } else { 0 };
                    self.mark_config();
                }
            }
            ui.label(
                egui::RichText::new("ROWS = SOURCES · COLS = OUTPUTS")
                    .size(10.0)
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("Click toggles 0 dB / -∞ · scroll ±0.5 dB · right-click clears")
                        .size(9.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);
                live_badge(ui, live_any);
            });
        });
        ui.add_space(6.0);
        let avail = ui.available_rect_before_wrap();
        let inspector_h = 38.0;
        let grid_rect = Rect::from_min_max(
            avail.min,
            Pos2::new(avail.right(), (avail.bottom() - inspector_h).max(avail.top() + 40.0)),
        );
        let chans = channels(self.matrix_kind, mode);
        let npairs = (chans.len() / 2).max(1).min(N_PAIRS);
        let source_meters = self.meters.bank(self.matrix_kind).snapshot();
        let output_meters = self.meters.output.snapshot();
        let matrix_gains = self.mix.compute_matrix(true);
        let source_color = widgets::kind_color(self.matrix_kind);
        let left_w = 74.0;
        let top_h = 46.0;
        let cw = ((grid_rect.width() - left_w - 8.0) / npairs as f32).clamp(22.0, 112.0);
        let ch = ((grid_rect.height() - top_h - 8.0) / npairs as f32).clamp(18.0, 66.0);
        let grid_w = cw * npairs as f32;
        let gx = grid_rect.left() + left_w + ((grid_rect.width() - left_w - grid_w) * 0.5).max(0.0);
        let gy = grid_rect.top() + top_h;
        let painter = ui.painter().with_clip_rect(grid_rect);
        painter.rect_filled(grid_rect, 6.0, PANEL);
        painter.rect_stroke(grid_rect, 6.0, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);

        // column headers: outputs, one level bar per output pair
        painter.text(
            Pos2::new(grid_rect.left() + 8.0, grid_rect.top() + 4.0),
            Align2::LEFT_TOP,
            "SOURCE",
            FontId::proportional(8.0),
            MUTED,
        );
        for op in 0..npairs {
            let x = gx + op as f32 * cw;
            let label = pair_label(Kind::Out, op);
            painter.text(
                Pos2::new(x + cw * 0.5, grid_rect.top() + 4.0),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(8.0),
                MUTED,
            );
            let level = output_meters.level[2 * op].max(output_meters.level[2 * op + 1]);
            let hold = output_meters.hold[2 * op].max(output_meters.hold[2 * op + 1]);
            let clip = output_meters.clip[2 * op] || output_meters.clip[2 * op + 1];
            let meter_rect = Rect::from_min_size(
                Pos2::new(x + 4.0, grid_rect.top() + 17.0),
                Vec2::new((cw - 8.0).max(6.0), 4.0),
            );
            draw_h_meter(&painter, meter_rect, level, hold, clip, widgets::KIND_OUT);
            if cw >= 36.0 && level > 0.001 {
                painter.text(
                    Pos2::new(x + cw * 0.5, grid_rect.top() + 23.0),
                    Align2::CENTER_TOP,
                    fmt_level_short(level),
                    FontId::proportional(7.0),
                    MUTED,
                );
            }
            let resp = ui.interact(meter_rect, Id::new(("mx-out-meter", op)), Sense::click());
            if resp.clicked() {
                self.meters.output.reset_pair(op);
            }
            if resp.hovered() {
                resp.on_hover_text(format!(
                    "{} output\nLevel  {}\nPeak  {}{}\nClick to reset peak",
                    pair_label(Kind::Out, op),
                    fmt_level(level),
                    fmt_level(hold),
                    if clip { "\nCLIP" } else { "" }
                ));
            }
        }
        // row headers + cells: sources are rows, outputs are columns
        for sp in 0..npairs {
            let y = gy + sp as f32 * ch;
            let label_y = if ch >= 30.0 { y + ch * 0.5 - 6.0 } else { y + ch * 0.5 };
            painter.text(
                Pos2::new(gx - 15.0, label_y),
                Align2::RIGHT_CENTER,
                pair_label(self.matrix_kind, sp),
                FontId::proportional(8.2),
                MUTED,
            );
            let level = source_meters.level[2 * sp].max(source_meters.level[2 * sp + 1]);
            let hold = source_meters.hold[2 * sp].max(source_meters.hold[2 * sp + 1]);
            let clip = source_meters.clip[2 * sp] || source_meters.clip[2 * sp + 1];
            if ch >= 30.0 && level > 0.001 {
                painter.text(
                    Pos2::new(gx - 15.0, y + ch * 0.5 + 6.0),
                    Align2::RIGHT_CENTER,
                    fmt_level_short(level),
                    FontId::proportional(7.0),
                    MUTED,
                );
            }
            let meter_rect = Rect::from_min_size(
                Pos2::new(gx - 9.0, y + 3.0),
                Vec2::new(4.0, (ch - 6.0).max(6.0)),
            );
            draw_v_meter(&painter, meter_rect, level, hold, clip, source_color);
            let resp = ui.interact(
                meter_rect,
                Id::new(("mx-src-meter", self.matrix_kind, sp)),
                Sense::click(),
            );
            if resp.clicked() {
                match self.matrix_kind {
                    Kind::In => self.meters.input.reset_pair(sp),
                    Kind::Play => self.meters.playback.reset_pair(sp),
                    Kind::Out => {}
                }
            }
            if resp.hovered() {
                resp.on_hover_text(format!(
                    "{} source\nLevel  {}\nPeak  {}{}\nClick to reset peak",
                    pair_label(self.matrix_kind, sp),
                    fmt_level(level),
                    fmt_level(hold),
                    if clip { "\nCLIP" } else { "" }
                ));
            }
            for op in 0..npairs {
                let x = gx + op as f32 * cw;
                let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cw - 1.0, ch - 1.0));
                let (db, db2) = self.matrix_cell(self.matrix_kind, sp, op);
                let db = match (db, db2) {
                    (None, None) => None,
                    (a, b) => Some(a.unwrap_or(-80.0).max(b.unwrap_or(-80.0))),
                };
                let selected = self.matrix_selected == Some((sp, op));
                let fill = cell_color(db);
                let live = matrix_cell_live(&matrix_gains, self.matrix_kind, sp, op, &source_meters);
                let activity = widgets::meter_frac(live);
                painter.rect_filled(cell, 2.0, fill);
                if activity > 0.005 {
                    // The FF800 does not expose per-crosspoint meters. This is the
                    // source level times the routed gain (mute/solo/pan folded in),
                    // before the output master.
                    painter.rect_filled(cell, 2.0, blend(fill, source_color, 0.68 * activity));
                }
                if db.is_some() {
                    // Every connected cell carries a meter groove, even while its
                    // source is silent; -∞ cells intentionally have none.
                    let track = Rect::from_min_max(
                        Pos2::new(cell.min.x + 3.0, cell.max.y - 5.0),
                        Pos2::new(cell.max.x - 3.0, cell.max.y - 2.0),
                    );
                    if track.width() > 0.0 {
                        painter.rect_filled(track, 1.0, Color32::from_rgb(0x0b, 0x0d, 0x0f));
                        let filled = track.width() * activity;
                        if filled > 0.3 {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    track.min,
                                    Pos2::new(track.min.x + filled, track.max.y),
                                ),
                                1.0,
                                blend(source_color, Color32::WHITE, 0.22),
                            );
                        }
                    }
                }
                painter.rect_stroke(
                    cell,
                    2.0,
                    Stroke::new(if selected { 1.6_f32 } else { 1.0_f32 }, if selected { ACCENT } else { Color32::from_black_alpha(80) }),
                    StrokeKind::Inside,
                );
                if cw >= 32.0 {
                    painter.text(
                        cell.center(),
                        Align2::CENTER_CENTER,
                        fmt_db(db),
                        FontId::proportional(8.2),
                        if db.is_some() { TEXT } else { DIMTEXT },
                    );
                }
                let resp = ui.interact(
                    cell,
                    Id::new(("mx", self.matrix_kind, sp, op)),
                    Sense::click_and_drag(),
                );
                if resp.clicked() {
                    self.matrix_selected = Some((sp, op));
                    let new = if db.is_some() { None } else { Some(0.0) };
                    self.set_matrix_cell(self.matrix_kind, sp, op, new);
                }
                if resp.secondary_clicked() {
                    self.matrix_selected = Some((sp, op));
                    self.set_matrix_cell(self.matrix_kind, sp, op, None);
                }
                if resp.hovered() {
                    let scroll = ui.input(|i| i.raw_scroll_delta.y);
                    if scroll.abs() > 0.1 {
                        let base = db.unwrap_or(-80.0);
                        let new = if scroll > 0.0 { (base + 0.5).min(6.0) } else { (base - 0.5).max(-80.0) };
                        self.set_matrix_cell(
                            self.matrix_kind,
                            sp,
                            op,
                            if new <= -80.0 { None } else { Some(new) },
                        );
                    }
                }
                if resp.dragged() {
                    let dy = ui.input(|i| i.pointer.delta().y);
                    if dy.abs() > 0.0 {
                        let base = db.unwrap_or(-80.0);
                        let new = (base - dy * 0.2).clamp(-80.0, 6.0);
                        self.set_matrix_cell(
                            self.matrix_kind,
                            sp,
                            op,
                            if new <= -80.0 { None } else { Some(new) },
                        );
                    }
                }
                if resp.clicked() || resp.secondary_clicked() {
                    self.matrix_selected = Some((sp, op));
                }
                if resp.hovered() {
                    let source_level =
                        source_meters.level[2 * sp].max(source_meters.level[2 * sp + 1]);
                    let output_level =
                        output_meters.level[2 * op].max(output_meters.level[2 * op + 1]);
                    resp.on_hover_text(format!(
                        "{} -> {}\nSource  {}\nSend gain  {}\nCrosspoint  {} (est., pre-master)\nOutput  {} (post-master)",
                        pair_label(self.matrix_kind, sp),
                        pair_label(Kind::Out, op),
                        fmt_level(source_level),
                        fmt_db(db),
                        fmt_level(live),
                        fmt_level(output_level),
                    ));
                }
            }
        }

        // inspector
        let inspector = Rect::from_min_max(
            Pos2::new(avail.left(), grid_rect.bottom() + 4.0),
            avail.right_bottom(),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(inspector), |ui| {
            ui.horizontal(|ui| {
                if let Some((sp, op)) = self.matrix_selected {
                    let src_name = pair_label(self.matrix_kind, sp);
                    let out_name = pair_label(Kind::Out, op);
                    ui.label(
                        egui::RichText::new(format!("{out_name}  <-  {src_name}"))
                            .size(11.5)
                            .color(TEXT),
                    );
                    let (a, b) = self.matrix_cell(self.matrix_kind, sp, op);
                    let cur = a.or(b).unwrap_or(-80.0);
                    let mut v = cur.max(-80.0);
                    let changed = ui
                        .add(
                            egui::Slider::new(&mut v, -40.0..=6.0)
                                .fixed_decimals(1)
                                .suffix(" dB")
                                .show_value(true),
                        )
                        .changed();
                    if changed {
                        self.set_matrix_cell(self.matrix_kind, sp, op, Some(v));
                    }
                    if ui.button("0 dB").clicked() {
                        self.set_matrix_cell(self.matrix_kind, sp, op, Some(0.0));
                    }
                    if ui.button("-∞").clicked() {
                        self.set_matrix_cell(self.matrix_kind, sp, op, None);
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Select a cell to edit its send level.")
                            .size(10.5)
                            .color(MUTED),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (label, db) in [("+6", Some(6.0)), ("0", Some(0.0)), ("-20", Some(-20.0)), ("-∞", None)] {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, cell_color(db));
                        ui.label(egui::RichText::new(label).size(9.0).color(MUTED));
                    }
                });
            });
        });
        ui.advance_cursor_after_rect(avail);
    }

    fn matrix_cell(&self, kind: Kind, src_pair: usize, out_pair: usize) -> (Option<f32>, Option<f32>) {
        let c0 = src_pair * 2;
        match kind {
            Kind::In => (
                self.mix.sends_in.get(c0).map(|s| s[out_pair].db).unwrap_or(None),
                self.mix.sends_in.get(c0 + 1).map(|s| s[out_pair].db).unwrap_or(None),
            ),
            Kind::Play => (
                self.mix.sends_play.get(c0).map(|s| s[out_pair].db).unwrap_or(None),
                self.mix.sends_play.get(c0 + 1).map(|s| s[out_pair].db).unwrap_or(None),
            ),
            Kind::Out => (None, None),
        }
    }

    fn set_matrix_cell(&mut self, kind: Kind, src_pair: usize, out_pair: usize, db: Option<f32>) {
        let c0 = src_pair * 2;
        let db = db.map(|d| d.clamp(-80.0, 6.0)).and_then(|d| if d <= -80.0 { None } else { Some(d) });
        match kind {
            Kind::In => {
                if c0 < N_IN {
                    self.mix.sends_in[c0][out_pair].db = db;
                }
                if c0 + 1 < N_IN {
                    self.mix.sends_in[c0 + 1][out_pair].db = db;
                }
            }
            Kind::Play => {
                if c0 < N_PLAY {
                    self.mix.sends_play[c0][out_pair].db = db;
                }
                if c0 + 1 < N_PLAY {
                    self.mix.sends_play[c0 + 1][out_pair].db = db;
                }
            }
            Kind::Out => {}
        }
        self.mark_mix();
    }

    // ------------------------------------------------------------ settings

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    self.settings_left(&mut cols[0]);
                    self.settings_right(&mut cols[1]);
                });
            });
    }

    fn settings_left(&mut self, ui: &mut egui::Ui) {
        widgets::card(ui, "DEVICE", |ui| {
            widgets::kv(ui, "Interface", "RME Fireface 800");
            widgets::kv(ui, "Node", self.node.clone().unwrap_or_else(|| "—".to_owned()));
            let state = if self.connected {
                "connected".to_owned()
            } else if self
                .conn_error
                .as_deref()
                .map(|e| e.contains("Conflicting mixer"))
                .unwrap_or(false)
            {
                "conflict — see banner".to_owned()
            } else {
                self.conn_error.clone().unwrap_or_else(|| "waiting".to_owned())
            };
            widgets::kv(ui, "State", state);
            widgets::kv(ui, "Rate", rate_text(self.status.rate));
            widgets::kv(ui, "Clock source", self.status.source.clone());
            widgets::kv(ui, "Transport", "direct FireWire (/dev/fw*)");
            if let Some(err) = &self.conn_error {
                if err.contains("Conflicting mixer") {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Another mixer is driving the DSP (Openface Mixer or snd-fireface-ctl-service).",
                        )
                        .size(9.5)
                        .color(AMBER),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Stop now").clicked() {
                            let _ = std::process::Command::new("systemctl")
                                .args(["--user", "stop", "openface-mixer-engine@ff800.service"])
                                .status();
                            let _ = std::process::Command::new("systemctl")
                                .args(["--user", "stop", "snd-fireface-ctl.service"])
                                .status();
                            self.backend.send(Command::Rescan);
                        }
                        if ui.button("Disable at login").clicked() {
                            for unit in [
                                "openface-mixer-engine@ff800.service",
                                "snd-fireface-ctl.service",
                            ] {
                                let _ = std::process::Command::new("systemctl")
                                    .args(["--user", "disable", "--now", unit])
                                    .status();
                            }
                            self.backend.send(Command::Rescan);
                        }
                    });
                }
            }
        });

        widgets::card(ui, "DEVICE FLASH", |ui| {
            ui.label(
                egui::RichText::new(
                    "Read the state saved in the interface's flash. Reading never writes flash.",
                )
                .size(9.5)
                .color(MUTED),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.connected, egui::Button::new("Load saved settings"))
                    .clicked()
                {
                    self.backend.send(Command::ReadFlashSettings);
                    self.toast("Reading saved settings from device flash…");
                }
                if ui
                    .add_enabled(self.connected, egui::Button::new("Load saved mixer"))
                    .clicked()
                {
                    self.backend.send(Command::ReadFlashMixer);
                    self.toast("Reading saved mixer from device flash…");
                }
            });
            ui.label(
                egui::RichText::new(
                    "Flash does not store mutes, stereo links, groups or solo; those stay as set.",
                )
                .size(8.5)
                .color(DIMTEXT),
            );
        });

        widgets::card(ui, "CLOCK", |ui| {
            let mut clock = self.settings.clock;
            egui::ComboBox::from_id_salt("clock_source")
                .width(160.0)
                .selected_text(clock.label())
                .show_ui(ui, |ui| {
                    for c in ClockSource::ALL {
                        ui.selectable_value(&mut clock, c, c.label());
                    }
                });
            if clock != self.settings.clock {
                self.settings.clock = clock;
                self.mark_settings();
            }
            ui.add_space(4.0);
            for input in &self.status.inputs {
                ui.horizontal(|ui| {
                    let color = match input.state {
                        SyncState::Sync => GREEN,
                        SyncState::Lock => AMBER,
                        SyncState::NoLock => DIMTEXT,
                    };
                    let (r, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.0, color);
                    ui.label(egui::RichText::new(input.label).size(10.5).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}{}",
                                input.state.label(),
                                input
                                    .rate
                                    .map(|r| format!(" · {}", rate_text(Some(r))))
                                    .unwrap_or_default()
                            ))
                            .size(10.0)
                            .color(if input.state == SyncState::NoLock { DIMTEXT } else { TEXT }),
                        );
                    });
                });
            }
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Status is read from the device; the FF800 cannot report its input options.")
                    .size(9.0)
                    .color(DIMTEXT),
            );
        });

        widgets::card(ui, "S/PDIF & OPTICAL", |ui| {
            let mut changed = false;
            changed |= checkbox(ui, &mut self.settings.spdif_in_optical, "S/PDIF input is optical");
            changed |= checkbox(ui, &mut self.settings.opt_out_spdif, "Optical output carries S/PDIF");
            changed |= checkbox(ui, &mut self.settings.spdif_pro, "S/PDIF output: professional");
            changed |= checkbox(ui, &mut self.settings.emphasis, "S/PDIF output emphasis");
            changed |= checkbox(ui, &mut self.settings.non_audio, "S/PDIF output non-audio");
            changed |= checkbox(ui, &mut self.settings.word_single, "Word clock out: single speed");
            if changed {
                self.mark_settings();
            }
        });
    }

    fn settings_right(&mut self, ui: &mut egui::Ui) {
        widgets::card(ui, "LEVELS", |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Input level").size(10.5).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut level = self.settings.in_level;
                    egui::ComboBox::from_id_salt("in_level")
                        .width(110.0)
                        .selected_text(level.label())
                        .show_ui(ui, |ui| {
                            for l in InLevel::ALL {
                                ui.selectable_value(&mut level, l, l.label());
                            }
                        });
                    if level != self.settings.in_level {
                        self.settings.in_level = level;
                        changed = true;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Output level").size(10.5).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut level = self.settings.out_level;
                    egui::ComboBox::from_id_salt("out_level")
                        .width(110.0)
                        .selected_text(level.label())
                        .show_ui(ui, |ui| {
                            for l in OutLevel::ALL {
                                ui.selectable_value(&mut level, l, l.label());
                            }
                        });
                    if level != self.settings.out_level {
                        self.settings.out_level = level;
                        changed = true;
                    }
                });
            });
            if changed {
                self.mark_settings();
            }
        });

        widgets::card(ui, "INPUT OPTIONS", |ui| {
            let mut changed = false;
            for (i, label) in [(0usize, "AN 1 jack"), (1, "AN 7 jack"), (2, "AN 8 jack")] {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label).size(10.5).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut jack = self.settings.jacks[i];
                        egui::ComboBox::from_id_salt(("jack", i))
                            .width(110.0)
                            .selected_text(jack.label())
                            .show_ui(ui, |ui| {
                                for j in Jack::ALL {
                                    ui.selectable_value(&mut jack, j, j.label());
                                }
                            });
                        if jack != self.settings.jacks[i] {
                            self.settings.jacks[i] = jack;
                            changed = true;
                        }
                    });
                });
            }
            for i in 0..4 {
                changed |= checkbox(ui, &mut self.settings.p48[i], &format!("48 V · AN {}", 7 + i));
            }
            ui.separator();
            changed |= checkbox(ui, &mut self.settings.drive, "AN 1 instrument drive (+25 dB)");
            changed |= checkbox(ui, &mut self.settings.limiter, "AN 1 instrument limiter");
            changed |= checkbox(ui, &mut self.settings.speaker_emu, "AN 1 speaker emulation");
            if changed {
                self.mark_settings();
            }
        });

        widgets::card(ui, "CONTROL ROOM", |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Main out").size(10.5).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut pair = self.config.main_pair.min(N_PAIRS - 1);
                    egui::ComboBox::from_id_salt("main_pair")
                        .width(110.0)
                        .selected_text(pair_label(Kind::Out, pair))
                        .show_ui(ui, |ui| {
                            for p in 0..N_PAIRS {
                                ui.selectable_value(&mut pair, p, pair_label(Kind::Out, p));
                            }
                        });
                    if pair != self.config.main_pair {
                        self.config.main_pair = pair;
                        self.mark_config();
                        self.mark_mix();
                    }
                });
            });
            let mut changed = false;
            changed |= checkbox(ui, &mut self.dim, "Dim (-20 dB)");
            changed |= checkbox(ui, &mut self.main_mute, "Mute main out");
            if changed {
                self.mark_mix();
            }
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Monitor actions change the output sent to the DSP, not the stored mix.")
                    .size(9.0)
                    .color(DIMTEXT),
            );
        });

        widgets::card(ui, "APPLICATION", |ui| {
            let mut close_to_tray = self.config.close_to_tray;
            if checkbox(ui, &mut close_to_tray, "Close button hides to tray") {
                self.config.close_to_tray = close_to_tray;
                self.mark_config();
            }
            let mut start_hidden = self.config.start_hidden;
            if checkbox(ui, &mut start_hidden, "Start hidden (tray only)") {
                self.config.start_hidden = start_hidden;
                self.mark_config();
            }
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!(
                    "Tray: {} · config: {}",
                    if self.tray_ok { "started" } else { "unavailable" },
                    persist::config_dir().display()
                ))
                .size(9.0)
                .color(DIMTEXT),
            );
        });

        widgets::card(ui, "ABOUT", |ui| {
            ui.label(
                egui::RichText::new(
                    "Independent TotalMix-style control for the Fireface 800's own DSP \
                     mixer. The mix keeps running in the interface when this window is \
                     closed.",
                )
                .size(10.0)
                .color(MUTED),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Independent implementation; mixer model adapted from Openface \
                     Mixer (MIT). No RME code, firmware, or artwork is included. \
                     Direct /dev/fw* transport; no ffado or snd-fireface-ctl-service \
                     required. Not affiliated with RME.",
                )
                .size(9.0)
                .color(DIMTEXT),
            );
        });
    }

    // ------------------------------------------------------------ conflict banner

    fn conflict_banner(&mut self, ctx: &egui::Context) {
        if self.connected {
            return;
        }
        let Some(err) = self.conn_error.clone() else { return };
        if !err.contains("Conflicting mixer") {
            return;
        }
        egui::TopBottomPanel::top("conflict")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(0x3a, 0x29, 0x1b))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .stroke(Stroke::new(1.0_f32, AMBER.gamma_multiply(0.55))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("!").size(14.0).strong().color(AMBER));
                    ui.label(
                        egui::RichText::new(
                            "Another mixer is driving the DSP. Stop it so FFMix can own the mixer.",
                        )
                        .size(10.5)
                        .color(TEXT),
                    );
                    if ui.button("Stop conflicting mixer").clicked() {
                        let _ = std::process::Command::new("systemctl")
                            .args(["--user", "stop", "openface-mixer-engine@ff800.service"])
                            .status();
                        let _ = std::process::Command::new("systemctl")
                            .args(["--user", "stop", "snd-fireface-ctl.service"])
                            .status();
                        self.backend.send(Command::Rescan);
                    }
                    if ui.button("Disable at login").clicked() {
                        for unit in [
                            "openface-mixer-engine@ff800.service",
                            "snd-fireface-ctl.service",
                        ] {
                            let _ = std::process::Command::new("systemctl")
                                .args(["--user", "disable", "--now", unit])
                                .status();
                        }
                        self.backend.send(Command::Rescan);
                    }
                    if ui.button("Retry").clicked() {
                        self.backend.send(Command::Rescan);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("direct FireWire · one controller at a time")
                                .size(9.0)
                                .color(DIMTEXT),
                        );
                    });
                });
            });
    }

    // ------------------------------------------------------------ status bar

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .stroke(Stroke::new(1.0_f32, BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (color, text) = if self.connected {
                        (GREEN, self.node.clone().unwrap_or_else(|| "Fireface 800".to_owned()))
                    } else {
                        let text = match self.conn_error.as_deref() {
                            Some(err) if err.contains("Conflicting mixer") => {
                                "conflicting mixer running".to_owned()
                            }
                            Some(err) => err.to_owned(),
                            None => "waiting for Fireface 800".to_owned(),
                        };
                        (RED, text)
                    };
                    let (dot, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, color);
                    ui.label(egui::RichText::new(text).size(9.8).color(MUTED));
                    ui.label(egui::RichText::new("·").size(9.8).color(DIMTEXT));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} in / {} play / {} out · {} · {}",
                            N_IN,
                            N_PLAY,
                            N_OUT,
                            rate_text(self.status.rate),
                            self.status.source
                        ))
                        .size(9.8)
                        .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("FFMix 0.1 · direct DSP")
                                .size(9.5)
                                .color(DIMTEXT),
                        );
                        if self.settings.known {
                            ui.label(
                                egui::RichText::new("config synced")
                                    .size(9.5)
                                    .color(DIMTEXT),
                            );
                        }
                    });
                });
            });
    }

    fn toast_layer(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = self.toast.clone() else { return };
        if at.elapsed() > Duration::from_secs(3) {
            self.toast = None;
            return;
        }
        egui::Area::new(Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -46.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(Color32::from_rgb(0x1b, 0x1e, 0x22))
                    .stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(msg).size(11.0).color(TEXT));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.meters.tick(now);
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        let _ = dt;
        self.last_frame = now;

        // Keyboard control for accessibility and for setups without a mouse.
        let ctrl = ctx.input(|i| i.modifiers.ctrl);
        let ctrl_shift = ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift);
        if ctrl && !ctrl_shift {
            let key = |k| ctx.input(|i| i.key_pressed(k));
            let tab = if key(egui::Key::Num1) {
                Some(Tab::Mixer)
            } else if key(egui::Key::Num2) {
                Some(Tab::Matrix)
            } else if key(egui::Key::Num3) {
                Some(Tab::Settings)
            } else {
                None
            };
            if let Some(tab) = tab {
                self.tab = tab;
                self.config.tab = tab.as_u8();
                self.mark_config();
            }
            if key(egui::Key::Q) {
                self.quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if ctrl_shift {
            let key = |k| ctx.input(|i| i.key_pressed(k));
            let mut changed = false;
            if key(egui::Key::ArrowUp) || key(egui::Key::ArrowDown) {
                let delta = if key(egui::Key::ArrowUp) { 1.0 } else { -1.0 };
                let pair = self.mix.selected.min(N_PAIRS - 1);
                let cur = self.mix.out[pair].gain.unwrap_or(-80.0);
                let next = (cur + delta).clamp(-80.0, 6.0);
                self.mix.out[pair].gain = if next <= -80.0 { None } else { Some(next) };
                changed = true;
                self.toast(format!(
                    "{} master {}",
                    pair_label(Kind::Out, pair),
                    fmt_db(self.mix.out[pair].gain)
                ));
            }
            if key(egui::Key::ArrowLeft) || key(egui::Key::ArrowRight) {
                let mode = speed_mode(self.status.rate);
                let present = (channels(Kind::Out, mode).len() / 2).max(1);
                let cur = self.mix.selected.min(present - 1);
                let next = if key(egui::Key::ArrowRight) {
                    (cur + 1) % present
                } else {
                    (cur + present - 1) % present
                };
                self.mix.selected = next;
                changed = true;
                self.toast(format!("Submix {}", pair_label(Kind::Out, next)));
            }
            if key(egui::Key::M) {
                let pair = self.mix.selected.min(N_PAIRS - 1);
                self.mix.out[pair].mute = !self.mix.out[pair].mute;
                changed = true;
                self.toast(if self.mix.out[pair].mute {
                    "Main out muted"
                } else {
                    "Main out unmuted"
                });
            }
            if key(egui::Key::Num0) {
                let pair = self.mix.selected.min(N_PAIRS - 1);
                self.mix.out[pair].gain = Some(0.0);
                changed = true;
                self.toast("Main out set to 0.0 dB");
            }
            if key(egui::Key::R) {
                for i in 0..N_OUT {
                    self.meters.input.reset(i);
                    self.meters.playback.reset(i);
                    self.meters.output.reset(i);
                }
            }
            if key(egui::Key::Num1) || key(egui::Key::Num2) {
                self.tab = Tab::Matrix;
                self.config.tab = self.tab.as_u8();
                self.matrix_kind = if key(egui::Key::Num2) { Kind::Play } else { Kind::In };
                self.config.matrix_kind = if self.matrix_kind == Kind::Play { 1 } else { 0 };
                self.mark_config();
            }
            if changed {
                self.mark_mix();
            }
        }

        // Preset shortcuts: F1..F8 recall, Shift+F1..F8 store.
        let shift_only = ctx.input(|i| i.modifiers.shift);
        let preset_keys = [
            egui::Key::F1,
            egui::Key::F2,
            egui::Key::F3,
            egui::Key::F4,
            egui::Key::F5,
            egui::Key::F6,
            egui::Key::F7,
            egui::Key::F8,
        ];
        for (slot, key) in preset_keys.into_iter().enumerate() {
            if ctx.input(|i| i.key_pressed(key)) {
                if shift_only {
                    self.store_slot(slot, false);
                } else {
                    self.recall_slot(slot);
                }
            }
        }

        // Matrix keyboard editing: arrows move, Enter toggles 0 dB / -inf.
        if self.tab == Tab::Matrix && !ctrl && !ctx.input(|i| i.modifiers.shift) {
            let mode = speed_mode(self.status.rate);
            let npairs = (channels(self.matrix_kind, mode).len() / 2).max(1).min(N_PAIRS);
            let (mut sp, mut op) = self.matrix_selected.unwrap_or((0, 0));
            let key = |k| ctx.input(|i| i.key_pressed(k));
            let mut moved = false;
            if key(egui::Key::ArrowLeft) {
                op = (op + npairs - 1) % npairs;
                moved = true;
            }
            if key(egui::Key::ArrowRight) {
                op = (op + 1) % npairs;
                moved = true;
            }
            if key(egui::Key::ArrowUp) {
                sp = (sp + npairs - 1) % npairs;
                moved = true;
            }
            if key(egui::Key::ArrowDown) {
                sp = (sp + 1) % npairs;
                moved = true;
            }
            if moved {
                self.matrix_selected = Some((sp, op));
            }
            let (a, b) = self.matrix_cell(self.matrix_kind, sp, op);
            if key(egui::Key::Enter) || key(egui::Key::Space) {
                self.matrix_selected = Some((sp, op));
                let new = if a.is_some() || b.is_some() { None } else { Some(0.0) };
                self.set_matrix_cell(self.matrix_kind, sp, op, new);
            }
            if key(egui::Key::Backspace) || key(egui::Key::Delete) {
                self.matrix_selected = Some((sp, op));
                self.set_matrix_cell(self.matrix_kind, sp, op, None);
            }
            if key(egui::Key::Plus) || key(egui::Key::Equals) {
                self.matrix_selected = Some((sp, op));
                let cur = a.or(b).unwrap_or(-80.0);
                self.set_matrix_cell(self.matrix_kind, sp, op, Some((cur + 0.5).min(6.0)));
            }
            if key(egui::Key::Minus) {
                self.matrix_selected = Some((sp, op));
                let cur = a.or(b).unwrap_or(-80.0);
                let new = (cur - 0.5).max(-80.0);
                self.set_matrix_cell(
                    self.matrix_kind,
                    sp,
                    op,
                    if new <= -80.0 { None } else { Some(new) },
                );
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if self.quit || !self.config.close_to_tray {
                // allow close
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_window(ctx);
                self.toast("Hidden to tray — click the tray icon to reopen");
            }
        }

        while let Ok(cmd) = self.ui_rx.try_recv() {
            self.handle_ui_cmd(cmd, ctx);
        }
        while let Ok(event) = self.backend.events.try_recv() {
            self.handle_event(event);
        }

        if !self.start_hidden_done {
            self.start_hidden_done = true;
            self.hide_window(ctx);
        }

        self.top_bar(ctx);
        self.conflict_banner(ctx);
        self.status_bar(ctx);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(8, 8)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Mixer => self.mixer_view(ui),
                Tab::Matrix => self.matrix_view(ui),
                Tab::Settings => self.settings_view(ui),
            });

        self.toast_layer(ctx);

        if (self.mix_dirty || self.settings_dirty)
            && self.last_sync.elapsed() >= Duration::from_millis(25)
        {
            self.sync_now();
        }
        if self.dirty_save && self.last_save.elapsed() >= Duration::from_secs(1) {
            self.save_all();
        }

        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.config.window_size = Some([rect.width(), rect.height()]);
        }
        if let Some(max) = ctx.input(|i| i.viewport().maximized) {
            self.config.window_maximized = max;
        }

        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.sync_now();
        self.save_all();
    }
}

// ---------------------------------------------------------------- helpers

fn rate_text(rate: Option<u32>) -> String {
    match rate {
        Some(44100) => "44.1 kHz".to_owned(),
        Some(88200) => "88.2 kHz".to_owned(),
        Some(176400) => "176.4 kHz".to_owned(),
        Some(r) => format!("{:.0} kHz", r as f32 / 1000.0),
        None => "no clock".to_owned(),
    }
}

fn matrix_cell_live(
    gains: &[[f32; N_SRC]; N_OUT],
    kind: Kind,
    src_pair: usize,
    out_pair: usize,
    source: &MeterSnapshot,
) -> f32 {
    let base = match kind {
        Kind::In => 0,
        Kind::Play => N_IN,
        Kind::Out => return 0.0,
    };
    let c0 = src_pair * 2;
    let mut peak = 0.0f32;
    for c in [c0, c0 + 1] {
        let src = base + c;
        let level = source.level.get(c).copied().unwrap_or(0.0);
        for out in [2 * out_pair, 2 * out_pair + 1] {
            peak = peak.max(level * gains[out][src]);
        }
    }
    peak.clamp(0.0, 2.0)
}

fn live_badge(ui: &mut egui::Ui, active: bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(48.0, 16.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, PANEL2);
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
    painter.circle_filled(
        Pos2::new(rect.min.x + 9.0, rect.center().y),
        2.2,
        if active { GREEN } else { DIMTEXT },
    );
    painter.text(
        Pos2::new(rect.min.x + 15.0, rect.center().y),
        Align2::LEFT_CENTER,
        "LIVE",
        FontId::proportional(8.5),
        if active { TEXT } else { MUTED },
    );
    resp.on_hover_text(
        "Rows meter sources, columns meter outputs.\nCell glow estimates source x routed gain before the output master.",
    );
}

fn draw_v_meter(
    painter: &egui::Painter,
    rect: Rect,
    level: f32,
    hold: f32,
    clip: bool,
    color: Color32,
) {
    painter.rect_filled(rect, 1.0, Color32::from_rgb(0x0b, 0x0d, 0x0f));
    let frac = widgets::meter_frac(level);
    if frac > 0.0 {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.min.x, rect.max.y - rect.height() * frac),
                rect.max,
            ),
            1.0,
            color,
        );
    }
    let hold_frac = widgets::meter_frac(hold);
    if hold_frac > 0.0 {
        let y = rect.max.y - rect.height() * hold_frac;
        painter.line_segment(
            [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 190)),
        );
    }
    if clip {
        painter.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 2.0)),
            0.5,
            RED,
        );
    }
}

fn draw_h_meter(
    painter: &egui::Painter,
    rect: Rect,
    level: f32,
    hold: f32,
    clip: bool,
    color: Color32,
) {
    painter.rect_filled(rect, 1.0, Color32::from_rgb(0x0b, 0x0d, 0x0f));
    let frac = widgets::meter_frac(level);
    if frac > 0.0 {
        painter.rect_filled(
            Rect::from_min_max(
                rect.min,
                Pos2::new(rect.min.x + rect.width() * frac, rect.max.y),
            ),
            1.0,
            color,
        );
    }
    let hold_frac = widgets::meter_frac(hold);
    if hold_frac > 0.0 {
        let x = rect.min.x + rect.width() * hold_frac;
        painter.line_segment(
            [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 190)),
        );
    }
    if clip {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.max.x - 2.0, rect.min.y), rect.max),
            0.5,
            RED,
        );
    }
}

fn fmt_level(lin: f32) -> String {
    match lin2db(lin) {
        Some(db) => format!("{db:.1} dBFS"),
        None => "-∞".to_owned(),
    }
}

fn fmt_level_short(lin: f32) -> String {
    match lin2db(lin) {
        Some(db) => format!("{db:.0}"),
        None => "-∞".to_owned(),
    }
}

fn cell_color(db: Option<f32>) -> Color32 {
    let Some(db) = db else {
        return Color32::from_rgb(0x18, 0x1a, 0x1d);
    };
    if db > 0.05 {
        let t = ((db + 1.0) / 7.0).clamp(0.0, 1.0);
        blend(Color32::from_rgb(0x5a, 0x45, 0x1a), Color32::from_rgb(0xe3, 0xb3, 0x41), t)
    } else {
        let t = ((db + 40.0) / 40.0).clamp(0.0, 1.0);
        blend(Color32::from_rgb(0x1a, 0x22, 0x28), Color32::from_rgb(0x2b, 0x7a, 0x8c), t)
    }
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn checkbox(ui: &mut egui::Ui, value: &mut bool, label: &str) -> bool {
    let before = *value;
    ui.checkbox(value, egui::RichText::new(label).size(10.5));
    before != *value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_live_uses_source_base_and_both_output_legs() {
        let mut source = MeterSnapshot {
            level: [0.0; N_OUT],
            hold: [0.0; N_OUT],
            clip: [false; N_OUT],
        };
        source.level[0] = 0.5;
        let mut gains = [[0.0; N_SRC]; N_OUT];
        gains[0][0] = 0.25;
        gains[1][N_IN] = 0.5;

        assert_eq!(matrix_cell_live(&gains, Kind::In, 0, 0, &source), 0.125);
        assert_eq!(matrix_cell_live(&gains, Kind::Play, 0, 0, &source), 0.25);
    }
}
