//! Theme, fonts and small reusable widgets.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    RichText, Stroke, TextStyle, Theme, ThemePreference, Ui,
};
use std::sync::Arc;

pub const ACCENT: Color32 = Color32::from_rgb(56, 189, 248);
pub const BG: Color32 = Color32::from_rgb(15, 17, 22);
pub const PANEL: Color32 = Color32::from_rgb(21, 24, 31);
pub const CARD: Color32 = Color32::from_rgb(28, 32, 41);
pub const BORDER: Color32 = Color32::from_rgb(44, 50, 63);
pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 147, 166);

pub fn apply(ctx: &egui::Context) {
    load_fonts(ctx);
    ctx.set_theme(ThemePreference::Dark);
    ctx.set_visuals_of(Theme::Dark, visuals());
    ctx.style_mut_of(Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 7.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.scroll.bar_width = 8.0;
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(20.0));
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(14.5));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.5));
    });
}

/// Use the system UI font on Windows for a native look; fall back to egui's.
fn load_fonts(ctx: &egui::Context) {
    let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("system-ui".into(), Arc::new(FontData::from_owned(bytes)));
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.insert(0, "system-ui".into());
    }
    ctx.set_fonts(fonts);
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(56, 189, 248, 60);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    let radius = CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = Color32::from_rgb(38, 43, 55);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(38, 43, 55);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_fill = Color32::from_rgb(52, 59, 75);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 59, 75);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.bg_fill = Color32::from_rgb(64, 73, 93);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(64, 73, 93);
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(12);
    v
}

pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Rounded surface used for grouped content.
pub fn card() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
}

/// A card tinted with `color`, used for the verdict.
pub fn tinted_card(color: Color32) -> Frame {
    Frame::new()
        .fill(alpha(color, 26))
        .stroke(Stroke::new(1.0, alpha(color, 130)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
}

/// Small "label over value" tile.
pub fn tile(ui: &mut Ui, label: &str, value: &str) {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(96.0);
            ui.label(RichText::new(label).small().color(TEXT_DIM));
            ui.label(RichText::new(value).size(17.0).strong());
        });
}

/// Rounded status pill with centred text; returns its width.
pub fn chip(painter: &egui::Painter, left_center: egui::Pos2, text: &str, color: Color32) -> f32 {
    let font = FontId::proportional(10.5);
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    let (w, h) = (galley.size().x + 16.0, 20.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(left_center.x, left_center.y - h / 2.0),
        egui::vec2(w, h),
    );
    painter.rect_filled(rect, CornerRadius::same(10), alpha(color, 34));
    painter.galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    w
}
