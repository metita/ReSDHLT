//! Dark theme: palette, widget styling and text scale.
//!
//! Everything visual lives here so the rest of the UI only talks about roles
//! (text, muted, accent, ok/warn/err) and never about raw colours.

use eframe::egui;
use egui::{Color32, Rounding, Stroke};

// ---------------------------------------------------------------- palette

pub const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x15);
pub const PANEL: Color32 = Color32::from_rgb(0x14, 0x17, 0x1D);
pub const CARD: Color32 = Color32::from_rgb(0x1A, 0x1E, 0x26);
pub const CARD_HI: Color32 = Color32::from_rgb(0x22, 0x27, 0x31);
pub const LINE: Color32 = Color32::from_rgb(0x2A, 0x30, 0x3B);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xEF);
pub const MUTED: Color32 = Color32::from_rgb(0x8B, 0x93, 0xA1);
pub const FAINT: Color32 = Color32::from_rgb(0x60, 0x68, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0x5A, 0xA9, 0xFF);
pub const ACCENT_DEEP: Color32 = Color32::from_rgb(0x1E, 0x4C, 0x84);
pub const OK: Color32 = Color32::from_rgb(0x4E, 0xD0, 0x8A);
pub const WARN: Color32 = Color32::from_rgb(0xF0, 0xB4, 0x29);
pub const ERR: Color32 = Color32::from_rgb(0xF0, 0x6B, 0x6B);

/// Content is never stretched past this. On a 4K monitor a 3000 px wide row of
/// sliders is unreadable; capping it keeps every tab the same shape at any
/// resolution and lets the panel centre what is left over.
pub const CONTENT_MAX_W: f32 = 1000.0;

pub const ROUND: f32 = 8.0;

// ---------------------------------------------------------------- style

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    v.panel_fill = PANEL;
    v.window_fill = BG;
    v.extreme_bg_color = BG; // text edit / scroll background
    v.faint_bg_color = CARD_HI; // striped rows
    v.code_bg_color = CARD;
    v.hyperlink_color = ACCENT;
    v.window_rounding = Rounding::same(ROUND);
    v.menu_rounding = Rounding::same(ROUND);
    v.window_stroke = Stroke::new(1.0_f32, LINE);
    v.popup_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 4.0),
        blur: 16.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    };
    v.window_shadow = v.popup_shadow;

    v.selection.bg_fill = ACCENT_DEEP;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.noninteractive.rounding = Rounding::same(ROUND);

    w.inactive.bg_fill = CARD_HI;
    w.inactive.weak_bg_fill = CARD;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, LINE);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.inactive.rounding = Rounding::same(ROUND);
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = Color32::from_rgb(0x2B, 0x32, 0x3E);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x2B, 0x32, 0x3E);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.linear_multiply(0.7));
    w.hovered.fg_stroke = Stroke::new(1.2_f32, Color32::WHITE);
    w.hovered.rounding = Rounding::same(ROUND);
    w.hovered.expansion = 1.0;

    w.active.bg_fill = ACCENT_DEEP;
    w.active.weak_bg_fill = ACCENT_DEEP;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke = Stroke::new(1.2_f32, Color32::WHITE);
    w.active.rounding = Rounding::same(ROUND);
    w.active.expansion = 1.0;

    w.open.bg_fill = CARD_HI;
    w.open.bg_stroke = Stroke::new(1.0_f32, ACCENT.linear_multiply(0.6));
    w.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.open.rounding = Rounding::same(ROUND);

    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(8.0);
    style.spacing.interact_size = egui::vec2(28.0, 26.0);
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.floating = false;
    style.spacing.combo_width = 220.0;
    style.spacing.tooltip_width = 460.0;
    style.visuals.striped = false;

    use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::new(19.0, Proportional)),
        (TextStyle::Body, FontId::new(13.5, Proportional)),
        (TextStyle::Button, FontId::new(13.5, Proportional)),
        (TextStyle::Small, FontId::new(11.5, Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, Monospace)),
    ]
    .into();

    ctx.set_style(style);
}
