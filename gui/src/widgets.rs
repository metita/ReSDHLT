//! Reusable pieces of the interface.
//!
//! The point of this module is symmetry: every option in every tab is drawn
//! through `row`, which allocates the same three columns (label, control,
//! badge) with widths derived once per frame from the available space. That is
//! what keeps the sliders, checkboxes and combos of all five tabs on the same
//! vertical lines at any window size.

use eframe::egui;
use egui::{Align, Color32, Layout, Response, RichText, Sense, Ui, Vec2};

use crate::theme::*;

/// Column widths for one panel, recomputed each frame from its width.
#[derive(Clone, Copy)]
pub struct Metrics {
    pub label_w: f32,
    pub ctrl_w: f32,
    pub badge_w: f32,
    pub row_h: f32,
}

impl Metrics {
    pub fn for_width(avail: f32) -> Self {
        // Narrow windows give the label less room and drop the badge column
        // entirely rather than squeezing the control into nothing.
        // The badge is a one-word hint, so it gets the minimum that fits it and
        // the controls keep the rest: a wide badge column reads as a hole
        // between the buttons and the edge of the card.
        let badge_w = if avail < 470.0 {
            0.0
        } else {
            (avail * 0.12).clamp(84.0, 128.0)
        };
        let label_w = (avail * 0.28).clamp(120.0, 220.0);
        let spacing = 20.0;
        let ctrl_w = (avail - label_w - badge_w - spacing).max(140.0);
        Self {
            label_w,
            ctrl_w,
            badge_w,
            row_h: 26.0,
        }
    }
}

/// A labelled option row: `label  [control]  (badge)`.
pub fn row<R>(
    ui: &mut Ui,
    m: &Metrics,
    label: &str,
    help: &str,
    badge: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let out = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;

            // Label column. The whole cell is the hover target for the help, so
            // the user does not have to find a tiny icon.
            // `allocate_ui_with_layout` shrinks to its content, which would let
            // every row pick its own column widths; `set_min_size` pins the cell
            // to the width the metrics asked for.
            let cell = ui.allocate_ui_with_layout(
                Vec2::new(m.label_w, m.row_h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_size(Vec2::new(m.label_w, m.row_h));
                    // The tooltip goes on the widgets themselves: egui only
                    // hovers the topmost widget, so a tooltip hung on the
                    // surrounding cell never fires over the text.
                    let text = ui.add(
                        egui::Label::new(RichText::new(label).color(TEXT))
                            .truncate()
                            .sense(Sense::hover()),
                    );
                    if !help.is_empty() {
                        text.on_hover_text(help);
                        ui.add(
                            egui::Label::new(RichText::new("?").color(FAINT).small())
                                .sense(Sense::hover()),
                        )
                        .on_hover_text(help);
                    }
                },
            );
            // Empty space in the cell still answers, for the rows whose label is
            // shorter than the column.
            if !help.is_empty() {
                ui.interact(
                    cell.response.rect,
                    cell.response.id.with("help"),
                    Sense::hover(),
                )
                .on_hover_text(help);
            }

            // Control column: fixed width, so every widget lines up.
            let inner = ui.allocate_ui_with_layout(
                Vec2::new(m.ctrl_w, m.row_h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_size(Vec2::new(m.ctrl_w, m.row_h));
                    add(ui)
                },
            );

            // Badge column, right aligned.
            if m.badge_w > 0.0 {
                ui.allocate_ui_with_layout(
                    Vec2::new(m.badge_w, m.row_h),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        ui.set_min_size(Vec2::new(m.badge_w, m.row_h));
                        if let Some(b) = badge {
                            chip(ui, b, ACCENT);
                        }
                    },
                );
            }
            inner.inner
        })
        .inner;
    ui.add_space(1.0);
    out
}

/// A line of explanation under a row, aligned with the controls instead of the
/// labels, so it reads as belonging to the option above it.
pub fn hint(ui: &mut Ui, m: &Metrics, text: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_space(m.label_w + 10.0);
        ui.add(egui::Label::new(RichText::new(text).color(color).small()).wrap());
    });
    ui.add_space(2.0);
}

/// Small rounded tag used for "recomendado", stage names and counters.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    egui::Frame::none()
        .fill(color.linear_multiply(0.16))
        .stroke(egui::Stroke::new(1.0_f32, color.linear_multiply(0.5)))
        .rounding(999.0)
        .inner_margin(egui::Margin::symmetric(8.0, 2.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).small());
        })
        .response
}

/// A titled card. Every group of options sits in one, which is what gives the
/// tabs their consistent rhythm.
pub fn card<R>(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let out = egui::Frame::none()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0_f32, LINE))
        .rounding(ROUND + 2.0)
        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).color(TEXT).strong().size(14.5));
                    if !subtitle.is_empty() {
                        ui.label(RichText::new(subtitle).color(MUTED).small());
                    }
                });
                ui.add_space(6.0);
                let line = ui.available_rect_before_wrap();
                ui.painter().hline(
                    line.x_range(),
                    line.top(),
                    egui::Stroke::new(1.0_f32, LINE),
                );
                ui.add_space(6.0);
            }
            body(ui)
        })
        .inner;
    ui.add_space(10.0);
    out
}

/// iOS-style switch. Reads much faster than a checkbox in a long list of
/// on/off options, which is most of this UI.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(38.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how = ui.ctx().animate_bool(resp.id, *on);
    let hovered = resp.hovered();

    let bg = if *on {
        ACCENT.linear_multiply(0.55 + 0.2 * hovered as u8 as f32)
    } else if hovered {
        CARD_HI
    } else {
        Color32::from_rgb(0x25, 0x2A, 0x34)
    };
    let stroke = egui::Stroke::new(1.0_f32, if *on { ACCENT } else { LINE });
    let p = ui.painter();
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r, bg);
    p.rect_stroke(rect, r, stroke);
    let cx = egui::lerp((rect.left() + r)..=(rect.right() - r), how);
    p.circle_filled(
        egui::pos2(cx, rect.center().y),
        r - 3.0,
        if *on { Color32::WHITE } else { MUTED },
    );
    resp
}

/// Toggle row: switch plus the state in words, so nothing depends on colour
/// alone.
pub fn toggle_row(ui: &mut Ui, m: &Metrics, label: &str, help: &str, badge: Option<&str>, on: &mut bool) {
    row(ui, m, label, help, badge, |ui| {
        let changed = toggle(ui, on).changed();
        ui.label(
            RichText::new(if *on { "activado" } else { "desactivado" })
                .color(if *on { OK } else { MUTED })
                .small(),
        );
        changed
    });
}

/// Slider that always fills the control column, with the number box at a fixed
/// place on the right.
pub fn slider_u32(ui: &mut Ui, m: &Metrics, value: &mut u32, range: std::ops::RangeInclusive<u32>, step: f64) {
    ui.spacing_mut().slider_width = (m.ctrl_w - 78.0).max(90.0);
    let mut s = egui::Slider::new(value, range);
    if step > 0.0 {
        s = s.step_by(step);
    }
    ui.add(s);
}

pub fn slider_f32(ui: &mut Ui, m: &Metrics, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.spacing_mut().slider_width = (m.ctrl_w - 78.0).max(90.0);
    ui.add(egui::Slider::new(value, range));
}

/// Path field plus its buttons, sized so the buttons always end at the same x.
pub fn path_row(
    ui: &mut Ui,
    m: &Metrics,
    value: &mut String,
    state: Option<bool>,
    browse: &str,
    on_browse: impl FnOnce() -> Option<String>,
    clearable: bool,
) {
    // The button area is reserved whether or not this row has a Limpiar, so
    // every "Buscar" in the tab starts at the same x.
    let dot_w = 16.0;
    let btn_w = 156.0;
    let field_w = (m.ctrl_w - btn_w - dot_w - 12.0).max(80.0);

    // Status dot: green when the path resolves, red when it does not.
    let (rect, _) = ui.allocate_exact_size(Vec2::new(dot_w, m.row_h), Sense::hover());
    let color = match state {
        Some(true) => OK,
        Some(false) => ERR,
        None => FAINT,
    };
    ui.painter().circle_filled(rect.center(), 4.0, color);

    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(field_w)
            .hint_text("sin definir"),
    );
    if ui.button(browse).clicked() {
        if let Some(p) = on_browse() {
            *value = p;
        }
    }
    if clearable && ui.button("Limpiar").clicked() {
        value.clear();
    }
}

/// Evenly divided tab strip: every tab gets the same width, so the header stays
/// symmetric no matter how long the labels are.
pub fn tab_strip<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: &mut T,
    tabs: &[(T, &str)],
) {
    let n = tabs.len() as f32;
    let spacing = ui.spacing().item_spacing.x;
    let w = ((ui.available_width() - spacing * (n - 1.0)) / n).max(60.0);
    ui.horizontal(|ui| {
        for (tab, label) in tabs {
            let selected = *current == *tab;
            let text = if selected {
                RichText::new(*label).color(Color32::WHITE).strong()
            } else {
                RichText::new(*label).color(MUTED)
            };
            let btn = egui::Button::new(text)
                .min_size(Vec2::new(w, 28.0))
                .rounding(ROUND)
                .fill(if selected { ACCENT_DEEP } else { Color32::TRANSPARENT })
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    if selected { ACCENT } else { LINE },
                ));
            if ui.add(btn).clicked() {
                *current = *tab;
            }
        }
    });
}

/// Centres a column of at most `max_w` inside the panel. Option pages want a
/// readable line length (`CONTENT_MAX_W`); a management screen full of lists
/// wants the room.
pub fn centered_column_w<R>(ui: &mut Ui, max_w: f32, body: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_width();
    let w = avail.min(max_w);
    let pad = ((avail - w) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.allocate_ui_with_layout(
            Vec2::new(w, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(w);
                body(ui)
            },
        )
        .inner
    })
    .inner
}
