//! The editor's look: warm charcoal panels with a copper accent, from the
//! engine's branding.

use egui::{Color32, CornerRadius, Stroke, Theme};

/// Copper: the selected tool, active tab and selected row.
pub const ACCENT: Color32 = Color32::from_rgb(214, 134, 72);
/// The selection outline in the viewport.
pub const SELECTION: Color32 = Color32::from_rgb(255, 196, 96);
/// Panel background.
pub const PANEL: Color32 = Color32::from_rgb(38, 34, 31);
/// Tab bars and the toolbar.
pub const BAR: Color32 = Color32::from_rgb(27, 24, 22);
/// Text fields and tiles.
pub const WELL: Color32 = Color32::from_rgb(24, 21, 19);
pub const TEXT: Color32 = Color32::from_rgb(226, 218, 208);
pub const TEXT_DIM: Color32 = Color32::from_rgb(158, 148, 137);
const LINE: Color32 = Color32::from_rgb(62, 55, 49);
const WIDGET: Color32 = Color32::from_rgb(58, 52, 47);
const WIDGET_HOVER: Color32 = Color32::from_rgb(76, 67, 59);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(96, 80, 66);

/// Applies the theme. The editor is always dark: a bright UI around a dark
/// viewport makes colours hard to judge.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let radius = CornerRadius::same(3);
    ctx.style_mut_of(Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = WELL;
        v.faint_bg_color = Color32::from_rgb(44, 40, 36);
        v.window_stroke = Stroke::new(1.0, LINE);
        v.selection.bg_fill = Color32::from_rgb(132, 78, 38);
        v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(255, 214, 176));
        v.hyperlink_color = ACCENT;
        for (w, bg) in [
            (&mut v.widgets.noninteractive, PANEL),
            (&mut v.widgets.inactive, WIDGET),
            (&mut v.widgets.hovered, WIDGET_HOVER),
            (&mut v.widgets.active, WIDGET_ACTIVE),
            (&mut v.widgets.open, WIDGET_HOVER),
        ] {
            w.bg_fill = bg;
            w.weak_bg_fill = bg;
            w.corner_radius = radius;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
}
