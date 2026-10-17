//! Custom-painted TotalMix-style controls.

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};

use crate::model::{db2pos, lin2db, pan_text, pos2db};

pub const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x21, 0x24, 0x28);
pub const PANEL2: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x33);
pub const PANEL3: Color32 = Color32::from_rgb(0x33, 0x38, 0x3e);
pub const BORDER: Color32 = Color32::from_rgb(0x3f, 0x46, 0x4d);
pub const TEXT: Color32 = Color32::from_rgb(0xd9, 0xdc, 0xdf);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x92, 0x9b);
pub const DIMTEXT: Color32 = Color32::from_rgb(0x67, 0x6e, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0x82, 0xd7, 0xc2);
pub const AMBER: Color32 = Color32::from_rgb(0xe3, 0xb3, 0x41);
pub const RED: Color32 = Color32::from_rgb(0xe0, 0x52, 0x52);
pub const GREEN: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x7a);
pub const BLUE: Color32 = Color32::from_rgb(0x72, 0xae, 0xe8);
pub const KIND_IN: Color32 = Color32::from_rgb(0x4d, 0x8b, 0xd4);
pub const KIND_PLAY: Color32 = Color32::from_rgb(0x4f, 0xb8, 0xa4);
pub const KIND_OUT: Color32 = Color32::from_rgb(0xd9, 0x9b, 0x3d);

pub fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = Color32::from_rgb(0x0e, 0x10, 0x12);
    style.visuals.faint_bg_color = PANEL;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.30);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.warn_fg_color = AMBER;
    style.visuals.error_fg_color = RED;
    style.visuals.window_corner_radius = CornerRadius::same(7);
    style.visuals.menu_corner_radius = CornerRadius::same(6);
    style.visuals.widgets.noninteractive.bg_fill = PANEL;
    style.visuals.widgets.noninteractive.weak_bg_fill = PANEL;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    style.visuals.widgets.noninteractive.corner_radius = 4.into();
    style.visuals.widgets.inactive.bg_fill = PANEL2;
    style.visuals.widgets.inactive.weak_bg_fill = PANEL2;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    style.visuals.widgets.inactive.corner_radius = 4.into();
    style.visuals.widgets.hovered.bg_fill = PANEL3;
    style.visuals.widgets.hovered.weak_bg_fill = PANEL3;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.7));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    style.visuals.widgets.hovered.corner_radius = 4.into();
    style.visuals.widgets.active.bg_fill = PANEL3;
    style.visuals.widgets.active.weak_bg_fill = PANEL3;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    style.visuals.widgets.active.corner_radius = 4.into();
    style.visuals.widgets.open = style.visuals.widgets.active;
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    style.spacing.button_padding = Vec2::new(8.0, 3.0);
    style.spacing.interact_size = Vec2::new(32.0, 22.0);
    style.text_styles = [
        (egui::TextStyle::Heading, FontId::proportional(15.0)),
        (egui::TextStyle::Body, FontId::proportional(13.0)),
        (egui::TextStyle::Button, FontId::proportional(12.0)),
        (egui::TextStyle::Small, FontId::proportional(10.5)),
        (egui::TextStyle::Monospace, FontId::monospace(11.0)),
    ]
    .into();
    ctx.set_style(style);
}

pub fn kind_color(kind: crate::model::Kind) -> Color32 {
    match kind {
        crate::model::Kind::In => KIND_IN,
        crate::model::Kind::Play => KIND_PLAY,
        crate::model::Kind::Out => KIND_OUT,
    }
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
}

pub fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    panel_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(title).size(10.5).color(MUTED).strong());
        ui.add_space(6.0);
        add(ui);
    });
    ui.add_space(8.0);
}

pub fn kv(ui: &mut Ui, key: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(key).size(11.0).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value.into()).size(11.0).color(TEXT));
        });
    });
}

pub fn tab_button(ui: &mut Ui, label: &str, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(66.0, 24.0), Sense::click());
    let fill = if active {
        ACCENT
    } else if response.hovered() {
        PANEL3
    } else {
        PANEL2
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    if !active {
        ui.painter()
            .rect_stroke(rect, 4.0, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
    }
    let text_color = if active { Color32::from_rgb(0x10, 0x12, 0x14) } else { TEXT };
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), text_color);
    response
}

pub fn small_toggle(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    label: &str,
    active: bool,
    active_color: Color32,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let fill = if active {
        active_color
    } else if response.hovered() {
        PANEL3
    } else {
        PANEL2
    };
    let text_color = if active { Color32::from_rgb(0x12, 0x14, 0x16) } else { MUTED };
    ui.painter().rect_filled(rect, 3.0, fill);
    ui.painter()
        .rect_stroke(rect, 3.0, Stroke::new(1.0_f32, if active { active_color } else { BORDER }), StrokeKind::Inside);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(9.5),
        text_color,
    );
    response
}

/// Vertical fader. Returns true when the value changed.
pub fn fader(ui: &mut Ui, id: Id, db: &mut Option<f32>, rect: Rect, enabled: bool) -> bool {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

    let track_x = rect.center().x - 4.0;
    let top = rect.top() + 9.0;
    let bot = rect.bottom() - 9.0;
    let groove = Rect::from_min_max(
        Pos2::new(track_x - 2.0, top),
        Pos2::new(track_x + 2.0, bot),
    );
    painter.rect_filled(groove, 2.0, Color32::from_rgb(0x0b, 0x0c, 0x0d));

    // dB ticks
    for db_mark in [6.0, 0.0, -5.0, -10.0, -15.0, -20.0, -30.0, -40.0, -60.0] {
        let y = bot - db2pos(Some(db_mark)) * (bot - top);
        let color = if db_mark == 0.0 { Color32::from_rgb(0xc8, 0xcc, 0xd0) } else { Color32::from_rgb(0x4a, 0x50, 0x57) };
        painter.line_segment(
            [Pos2::new(track_x - 9.0, y), Pos2::new(track_x - 4.0, y)],
            Stroke::new(1.0_f32, color),
        );
    }

    let pos = db2pos(*db).clamp(0.0, 1.0);
    let knob_y = bot - pos * (bot - top);
    if pos > 0.0 {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(track_x - 1.0, knob_y), Pos2::new(track_x + 1.0, bot)),
            1.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 36),
        );
    }

    // cap + shadow
    let cap = Rect::from_center_size(Pos2::new(track_x, knob_y), Vec2::new(15.0, 20.0));
    painter.rect_filled(cap.translate(Vec2::new(0.0, 1.5)), 2.0, Color32::from_black_alpha(90));
    let cap_fill = if !enabled {
        Color32::from_rgb(0x3c, 0x40, 0x45)
    } else if response.hovered() || response.dragged() {
        Color32::from_rgb(0x9a, 0xa0, 0xa8)
    } else {
        Color32::from_rgb(0x79, 0x7f, 0x87)
    };
    painter.rect_filled(cap, 2.0, cap_fill);
    painter.rect_stroke(cap, 2.0, Stroke::new(1.0_f32, Color32::from_rgb(0x14, 0x15, 0x17)), StrokeKind::Inside);
    for dy in [-5.0, 5.0] {
        painter.line_segment(
            [Pos2::new(cap.left() + 3.0, knob_y + dy), Pos2::new(cap.right() - 3.0, knob_y + dy)],
            Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
        );
    }
    painter.line_segment(
        [Pos2::new(cap.left() + 2.0, knob_y), Pos2::new(cap.right() - 2.0, knob_y)],
        Stroke::new(1.2_f32, Color32::WHITE),
    );

    if !enabled {
        return false;
    }
    let mut changed = false;
    if response.dragged() {
        let delta = ui.input(|i| i.pointer.delta().y);
        let fine = ui.input(|i| i.modifiers.ctrl);
        let scale = if fine { 0.18 } else { 1.0 };
        let mut p = pos - delta / (bot - top).max(1.0) * scale;
        p = p.clamp(0.0, 1.0);
        let new = pos2db(p);
        if new != *db {
            *db = new;
            changed = true;
        }
    }
    if response.double_clicked() || (response.clicked() && ui.input(|i| i.modifiers.alt)) {
        if *db != Some(0.0) {
            *db = Some(0.0);
            changed = true;
        }
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let fine = ui.input(|i| i.modifiers.ctrl);
            let step = if fine { 0.1 } else { 1.0 } * scroll.signum();
            let old = db.unwrap_or(-80.0);
            let mut new = old + step;
            if new < -80.0 {
                new = -80.0;
            }
            let new = if new <= -80.0 { None } else { Some(new.clamp(-80.0, 6.0)) };
            if new != *db {
                *db = new;
                changed = true;
            }
        }
    }
    changed
}

/// Pan knob. Returns true when changed.
pub fn knob(ui: &mut Ui, id: Id, value: &mut f32, rect: Rect) -> bool {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);
    let center = rect.center();
    let r = 8.0;
    painter.circle_filled(center, r + 1.0, Color32::from_black_alpha(100));
    painter.circle_filled(
        center,
        r,
        if response.hovered() || response.dragged() {
            Color32::from_rgb(0x5b, 0x62, 0x6a)
        } else {
            Color32::from_rgb(0x44, 0x4a, 0x51)
        },
    );
    painter.circle_stroke(center, r, Stroke::new(1.0_f32, Color32::from_rgb(0x1b, 0x1d, 0x20)));
    let a = -std::f32::consts::FRAC_PI_2 + value.clamp(-1.0, 1.0) * 2.356;
    let tip = center + Vec2::new(a.cos(), a.sin()) * (r - 2.0);
    painter.line_segment([center, tip], Stroke::new(1.8_f32, Color32::WHITE));
    painter.circle_filled(center, 1.5, Color32::from_rgb(0x20, 0x23, 0x26));

    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let new = (*value + scroll.signum() / 32.0).clamp(-1.0, 1.0);
            if new != *value {
                *value = if new.abs() < 0.02 { 0.0 } else { new };
                return true;
            }
        }
    }
    if response.dragged() {
        let d = ui.input(|i| i.pointer.delta());
        let fine = ui.input(|i| i.modifiers.ctrl);
        let scale = if fine { 0.25 } else { 1.0 };
        let mut v = *value + (d.x - d.y) / 80.0 * scale;
        v = v.clamp(-1.0, 1.0);
        if v.abs() < 0.02 {
            v = 0.0;
        }
        if v != *value {
            *value = v;
            return true;
        }
    }
    if response.double_clicked() {
        if *value != 0.0 {
            *value = 0.0;
            return true;
        }
    }
    false
}

pub fn knob_text(pan: f32) -> String {
    pan_text(pan)
}

fn meter_frac(lin: f32) -> f32 {
    let Some(db) = lin2db(lin) else { return 0.0 };
    let db = db.min(0.0);
    let zero = db2pos(Some(0.0)).max(0.001);
    (db2pos(Some(db)) / zero).clamp(0.0, 1.0)
}

fn meter_y(rect: Rect, db: f32) -> f32 {
    let zero = db2pos(Some(0.0)).max(0.001);
    let frac = (db2pos(Some(db.min(0.0))) / zero).clamp(0.0, 1.0);
    rect.bottom() - frac * rect.height()
}

/// One or two meter bars. Returns true when clicked (reset peak).
pub fn meter(
    ui: &mut Ui,
    id: Id,
    levels: &[f32],
    holds: &[f32],
    clips: &[bool],
    rect: Rect,
) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    let painter = ui.painter().with_clip_rect(rect);
    let channels = levels.len().max(1);
    let bar_w = 4.0;
    let gap = 2.0;
    let total_w = channels as f32 * bar_w + (channels - 1) as f32 * gap;
    let x0 = rect.center().x - total_w / 2.0;

    for ch in 0..channels {
        let x = x0 + ch as f32 * (bar_w + gap);
        let bar = Rect::from_min_max(
            Pos2::new(x, rect.top() + 5.0),
            Pos2::new(x + bar_w, rect.bottom() - 3.0),
        );
        painter.rect_filled(bar, 1.0, Color32::from_rgb(0x0c, 0x0d, 0x0e));
        let level = levels.get(ch).copied().unwrap_or(0.0);
        let frac = meter_frac(level);
        let fill_top = bar.bottom() - frac * bar.height();
        if frac > 0.0 {
            let y12 = meter_y(bar, -12.0);
            let y3 = meter_y(bar, -3.0);
            // green: bottom .. min(fill_top, -12 dB)
            let green_top = fill_top.max(y12);
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(bar.left(), green_top), Pos2::new(bar.right(), bar.bottom())),
                1.0,
                GREEN,
            );
            if fill_top < y12 {
                let yellow_top = fill_top.max(y3);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(bar.left(), yellow_top), Pos2::new(bar.right(), y12)),
                    1.0,
                    Color32::from_rgb(0xe3, 0xd1, 0x00),
                );
                if fill_top < y3 {
                    painter.rect_filled(
                        Rect::from_min_max(Pos2::new(bar.left(), fill_top), Pos2::new(bar.right(), y3)),
                        1.0,
                        Color32::from_rgb(0xff, 0x45, 0x3a),
                    );
                }
            }
        }
        let hold = holds.get(ch).copied().unwrap_or(0.0);
        let hfrac = meter_frac(hold);
        if hfrac > 0.0 {
            let y = bar.bottom() - hfrac * bar.height();
            painter.line_segment(
                [Pos2::new(bar.left(), y), Pos2::new(bar.right(), y)],
                Stroke::new(1.2_f32, Color32::WHITE),
            );
        }
        let clip = clips.get(ch).copied().unwrap_or(false);
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x, rect.top() + 1.0),
                Pos2::new(x + bar_w, rect.top() + 3.5),
            ),
            1.0,
            if clip { Color32::from_rgb(0xff, 0x45, 0x3a) } else { Color32::from_rgb(0x3a, 0x3d, 0x41) },
        );
    }
    response.clicked()
}
