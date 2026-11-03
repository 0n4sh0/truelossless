//! Theme, fonts and small reusable widgets.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    RichText, Stroke, TextStyle, Theme, ThemePreference, Ui,
};
use std::sync::Arc;

pub const ACCENT: Color32 = Color32::from_rgb(76, 214, 251);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0, 54, 66);
pub const GREEN: Color32 = Color32::from_rgb(78, 222, 163);
pub const AMBER: Color32 = Color32::from_rgb(255, 185, 95);
pub const LOWEST: Color32 = Color32::from_rgb(9, 14, 26);
pub const BG: Color32 = Color32::from_rgb(14, 19, 31);
pub const PANEL: Color32 = Color32::from_rgb(22, 27, 40);
pub const CARD: Color32 = Color32::from_rgb(26, 31, 44);
pub const HIGH: Color32 = Color32::from_rgb(37, 42, 55);
pub const BORDER: Color32 = Color32::from_rgb(48, 53, 66);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 243);
pub const TEXT_DIM: Color32 = Color32::from_rgb(160, 172, 178);

const BOLD: &str = "bold";
const MONO_BOLD: &str = "mono-bold";

pub fn apply(ctx: &egui::Context) {
    load_fonts(ctx);
    ctx.set_theme(ThemePreference::Dark);
    ctx.set_visuals_of(Theme::Dark, visuals());
    ctx.style_mut_of(Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.scroll.bar_width = 8.0;
        s.text_styles
            .insert(TextStyle::Heading, FontId::new(20.0, bold_family()));
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(13.5));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(13.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.0));
        s.text_styles
            .insert(TextStyle::Monospace, FontId::monospace(12.0));
    });
}

/// Segoe UI for text and Consolas for numbers on Windows; egui's own fonts
/// fill in anywhere a file is missing. The bold families always exist so
/// text using them never panics.
fn load_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let prop = fonts.families[&FontFamily::Proportional].clone();
    let mono = fonts.families[&FontFamily::Monospace].clone();
    let mut add = |name: &str, file: &str| -> Option<String> {
        let bytes = std::fs::read(format!(r"C:\Windows\Fonts\{file}")).ok()?;
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
        Some(name.to_string())
    };
    let text = add("system-ui", "segoeui.ttf");
    let text_bold = add("system-ui-bold", "segoeuib.ttf");
    let code = add("code", "consola.ttf");
    let code_bold = add("code-bold", "consolab.ttf");
    let with = |first: Option<String>, rest: &[String]| -> Vec<String> {
        first.into_iter().chain(rest.iter().cloned()).collect()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with(text, &prop));
    fonts.families.insert(bold_family(), with(text_bold, &prop));
    fonts
        .families
        .insert(FontFamily::Monospace, with(code, &mono));
    fonts
        .families
        .insert(mono_bold_family(), with(code_bold, &mono));
    ctx.set_fonts(fonts);
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = LOWEST;
    v.faint_bg_color = CARD;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(76, 214, 251, 60);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    let radius = CornerRadius::same(4);
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
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = HIGH;
    v.widgets.inactive.weak_bg_fill = HIGH;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_fill = Color32::from_rgb(48, 53, 66);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 53, 66);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 78, 95));
    v.widgets.active.bg_fill = Color32::from_rgb(58, 64, 80);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(58, 64, 80);
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(8);
    v
}

pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

pub fn mono_bold_family() -> FontFamily {
    FontFamily::Name(MONO_BOLD.into())
}

/// Bold proportional text, for titles.
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(bold_family()).size(size)
}

/// Monospace text, for numbers, paths and technical details.
pub fn mono(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(FontFamily::Monospace).size(size)
}

pub fn mono_bold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(mono_bold_family()).size(size)
}

/// Small dim uppercase label that sits above a value.
pub fn caption(text: impl Into<String>) -> RichText {
    mono(text, 11.5).color(TEXT_DIM)
}

/// Rounded surface used for grouped content.
pub fn card() -> Frame {
    Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::same(12))
}

/// A card tinted with `color`, used for errors.
pub fn tinted_card(color: Color32) -> Frame {
    Frame::new()
        .fill(alpha(color, 26))
        .stroke(Stroke::new(1.0, alpha(color, 130)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(16))
}

/// Icon and title at the top of a card.
pub fn card_title(ui: &mut Ui, icon: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if !icon.is_empty() {
            ui.label(RichText::new(icon).size(17.0).color(ACCENT));
        }
        ui.label(heading(text, 19.0));
    });
}

/// Square-cornered tag such as "GENUINE" or "6 FILES".
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let w = ui
        .painter()
        .layout_no_wrap(text.to_string(), chip_font(), color)
        .size()
        .x
        + 14.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    chip(ui.painter(), rect.left_center(), text, color);
}

/// One headline number: a caption, the value with its unit and a note below.
pub struct Stat {
    pub label: &'static str,
    pub value: String,
    pub unit: &'static str,
    pub note: String,
    pub color: Option<Color32>,
    pub note_color: Color32,
}

pub fn stat_tile(ui: &mut Ui, s: &Stat, fill: Color32, size: f32) {
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.spacing_mut().interact_size.y = 0.0;
            ui.label(caption(s.label));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut v = mono_bold(&s.value, size);
                if let Some(c) = s.color {
                    v = v.color(c);
                }
                ui.label(v);
                if !s.unit.is_empty() {
                    ui.label(mono(s.unit, 12.5).color(TEXT_DIM));
                }
            });
            if !s.note.is_empty() {
                ui.label(mono(&s.note, 11.5).color(s.note_color));
            }
        });
}

/// Full-width fact with a leading tick, for verdict reasons.
pub fn reason_chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(alpha(HIGH, 160))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new("✔").size(13.5).color(color));
                ui.label(RichText::new(text).size(14.0).color(TEXT_DIM));
            });
        });
}

/// Pill row of tabs with one selected; works in either layout direction.
pub fn segmented(ui: &mut Ui, names: &[String], selected: &mut usize) {
    let order: Vec<usize> = if ui.layout().prefer_right_to_left() {
        (0..names.len()).rev().collect()
    } else {
        (0..names.len()).collect()
    };
    Frame::new()
        .fill(LOWEST)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().button_padding = egui::vec2(10.0, 2.0);
                for i in order {
                    let on = *selected == i;
                    let text = if on {
                        mono_bold(&names[i], 11.5).color(ACCENT_DARK)
                    } else {
                        mono(&names[i], 11.5).color(TEXT_DIM)
                    };
                    let b = egui::Button::new(text)
                        .fill(if on { ACCENT } else { Color32::TRANSPARENT })
                        .frame(on)
                        .min_size(egui::vec2(0.0, 22.0));
                    if ui.add(b).clicked() {
                        *selected = i;
                    }
                }
            });
        });
}

/// Like [`chip`], but anchored by its right edge; returns its width.
pub fn chip_right(
    painter: &egui::Painter,
    right_center: egui::Pos2,
    text: &str,
    color: Color32,
) -> f32 {
    let w = painter
        .layout_no_wrap(text.to_string(), chip_font(), color)
        .size()
        .x
        + 14.0;
    chip(
        painter,
        egui::pos2(right_center.x - w, right_center.y),
        text,
        color,
    )
}

fn chip_font() -> FontId {
    FontId::new(11.5, mono_bold_family())
}

/// Square-cornered status tag drawn with a painter; returns its width.
pub fn chip(painter: &egui::Painter, left_center: egui::Pos2, text: &str, color: Color32) -> f32 {
    let galley = painter.layout_no_wrap(text.to_string(), chip_font(), color);
    let (w, h) = (galley.size().x + 14.0, 20.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(left_center.x, left_center.y - h / 2.0),
        egui::vec2(w, h),
    );
    painter.rect_filled(rect, CornerRadius::same(2), alpha(color, 36));
    painter.galley(
        egui::pos2(
            rect.left() + 7.0,
            rect.center().y - galley.size().y / 2.0 + 1.0,
        ),
        galley,
        color,
    );
    w
}

/// Line icons drawn with the painter, so they look the same on every system.
#[derive(Clone, Copy)]
pub enum Icon {
    Folder,
    File,
    Check,
    Download,
    Minimize,
    Maximize,
    Restore,
    Close,
    Search,
    Sync,
}

/// Draw `icon` in a 14 px box centred on `c`.
pub fn paint_icon(p: &egui::Painter, c: egui::Pos2, icon: Icon, color: Color32) {
    let s = Stroke::new(1.4, color);
    let at = |x: f32, y: f32| egui::pos2(c.x + x, c.y + y);
    let rect =
        |x0: f32, y0: f32, x1: f32, y1: f32| egui::Rect::from_min_max(at(x0, y0), at(x1, y1));
    match icon {
        Icon::Folder => {
            p.rect_stroke(rect(-7.0, -3.5, 7.0, 5.5), 1.5, s, egui::StrokeKind::Middle);
            p.line(
                vec![
                    at(-7.0, -3.5),
                    at(-7.0, -6.0),
                    at(-2.0, -6.0),
                    at(0.0, -3.5),
                ],
                s,
            );
        }
        Icon::File => {
            p.line(
                vec![
                    at(1.5, -7.0),
                    at(-5.0, -7.0),
                    at(-5.0, 7.0),
                    at(5.0, 7.0),
                    at(5.0, -3.5),
                    at(1.5, -7.0),
                    at(1.5, -3.5),
                    at(5.0, -3.5),
                ],
                s,
            );
        }
        Icon::Check => {
            p.line(vec![at(-5.5, 0.0), at(-1.5, 4.0), at(5.5, -4.5)], s);
        }
        Icon::Download => {
            p.line_segment([at(0.0, -6.5), at(0.0, 3.0)], s);
            p.line(vec![at(-3.5, -0.5), at(0.0, 3.0), at(3.5, -0.5)], s);
            p.line_segment([at(-6.0, 6.5), at(6.0, 6.5)], s);
        }
        Icon::Minimize => {
            p.line_segment([at(-5.0, 0.0), at(5.0, 0.0)], s);
        }
        Icon::Maximize => {
            p.rect_stroke(rect(-5.0, -5.0, 5.0, 5.0), 1.0, s, egui::StrokeKind::Middle);
        }
        Icon::Restore => {
            p.rect_stroke(rect(-5.0, -3.0, 3.0, 5.0), 1.0, s, egui::StrokeKind::Middle);
            p.line(
                vec![
                    at(-3.0, -3.0),
                    at(-3.0, -5.0),
                    at(5.0, -5.0),
                    at(5.0, 3.0),
                    at(3.0, 3.0),
                ],
                s,
            );
        }
        Icon::Search => {
            p.circle_stroke(at(-1.5, -1.5), 4.5, s);
            p.line_segment([at(1.8, 1.8), at(6.0, 6.0)], s);
        }
        Icon::Sync => {
            let arc: Vec<egui::Pos2> = (0..=12)
                .map(|k| {
                    let a = (40.0 + k as f32 * 23.0_f32).to_radians();
                    at(5.5 * a.cos(), 5.5 * a.sin())
                })
                .collect();
            let end = *arc.last().unwrap();
            p.line(arc, s);
            p.line(
                vec![
                    egui::pos2(end.x - 3.5, end.y - 0.5),
                    end,
                    egui::pos2(end.x + 0.5, end.y + 3.5),
                ],
                s,
            );
        }
        Icon::Close => {
            p.line_segment([at(-5.0, -5.0), at(5.0, 5.0)], s);
            p.line_segment([at(-5.0, 5.0), at(5.0, -5.0)], s);
        }
    }
}

/// The app mark: five waveform bars.
pub fn paint_logo(p: &egui::Painter, c: egui::Pos2, color: Color32) {
    for (k, h) in [5.0, 10.0, 14.0, 8.0, 4.0].into_iter().enumerate() {
        let x = c.x - 6.0 + k as f32 * 3.0;
        p.line_segment(
            [egui::pos2(x, c.y - h / 2.0), egui::pos2(x, c.y + h / 2.0)],
            Stroke::new(1.6, color),
        );
    }
}

/// Toolbar button with a painted icon before its text.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    text: &str,
    primary: bool,
    enabled: bool,
) -> egui::Response {
    let (fg, fill) = if primary {
        (ACCENT_DARK, ACCENT)
    } else {
        (Color32::from_rgb(222, 226, 243), HIGH)
    };
    let fg = if enabled { fg } else { alpha(fg, 90) };
    // Leading spaces leave room for the icon painted over the padding.
    let b = egui::Button::new(RichText::new(format!("      {text}")).size(13.0).color(fg))
        .fill(fill)
        .min_size(egui::vec2(0.0, 30.0));
    let resp = ui.add_enabled(enabled, b);
    paint_icon(
        ui.painter(),
        egui::pos2(resp.rect.left() + 20.0, resp.rect.center().y),
        icon,
        fg,
    );
    resp
}

/// Filled dot, centred on the text line.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 3.5, color);
}

/// Frameless icon button with a hover highlight and tooltip.
pub fn icon_click(ui: &mut Ui, icon: Icon, tip: &str) -> bool {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let color = if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(4), HIGH);
        ACCENT
    } else {
        TEXT_DIM
    };
    paint_icon(ui.painter(), r.center(), icon, color);
    resp.on_hover_text(tip).clicked()
}

/// Draw one line of text left-aligned at `left_center`, ending in "…" if it
/// is wider than `max_width`.
pub fn fit_text(
    p: &egui::Painter,
    left_center: egui::Pos2,
    text: String,
    font: FontId,
    color: Color32,
    max_width: f32,
) {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = p.layout_job(job);
    let pos = egui::pos2(left_center.x, left_center.y - galley.size().y / 2.0);
    p.galley(pos, galley, color);
}
