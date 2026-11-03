#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod model;
mod plot;
mod quality_view;
mod settings;
mod style;
mod worker;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense,
};
use model::{
    bit_depth_text, fmt_duration, outcome_color, outcome_label, row_summary, status_color, Counts,
    Filter, Outcome, Slot,
};
use quality_view::QualityStore;
use settings::{FfmpegState, Settings};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use style::{alpha, card, chip_right, tinted_card, ACCENT, BG, GREEN, LOWEST, PANEL, TEXT_DIM};
use truelossless_core::{
    csv_line, CheckOptions, DecoderMode, FileReport, Md5Status, Status, CSV_HEADER,
};
use worker::{start_scan, Msg, ScanHandle};

fn main() -> eframe::Result {
    // Optional first argument: a folder or file to scan on start-up.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 720.0])
        .with_min_inner_size([820.0, 480.0])
        .with_decorations(false)
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "TrueLossless",
        options,
        Box::new(move |cc| {
            style::apply(&cc.egui_ctx);
            let settings = Settings::load();
            let ffmpeg = settings.resolve();
            let mut app = App {
                pending_open: initial,
                settings,
                ffmpeg,
                ..App::default()
            };
            app.apply_backend();
            Ok(Box::new(app))
        }),
    )
}

#[derive(Default)]
struct App {
    root: Option<PathBuf>,
    pending_open: Option<PathBuf>,
    slots: Vec<Slot>,
    done: usize,
    scan: Option<ScanHandle>,
    selected: Option<usize>,
    scroll_to_selected: bool,
    filter: Option<Filter>,
    search: String,
    verify_md5: bool,
    notice: Option<(String, Instant)>,
    quality: QualityStore,
    about_open: bool,
    settings_open: bool,
    settings: Settings,
    ffmpeg: FfmpegState,
}

impl App {
    fn filter(&self) -> Filter {
        self.filter.unwrap_or(Filter::All)
    }

    /// Tell the quality worker about the current ffmpeg and decoder choice.
    fn apply_backend(&mut self) {
        self.quality
            .set_backend(self.ffmpeg.toolchain(), self.settings.mode);
    }

    fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    fn start(&mut self, root: PathBuf, ctx: &egui::Context) {
        let opts = CheckOptions {
            analyze: true,
            verify_md5: self.verify_md5,
        };
        self.scan = Some(start_scan(root.clone(), opts, ctx.clone()));
        self.root = Some(root);
        self.slots.clear();
        self.quality.clear();
        self.selected = None;
        self.done = 0;
    }

    fn display_name(&self, path: &Path) -> String {
        match &self.root {
            Some(root) if root.is_dir() => path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
            _ => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }

    fn pump_messages(&mut self) {
        let msgs: Vec<Msg> = match &self.scan {
            Some(h) => h.rx.try_iter().collect(),
            None => return,
        };
        for msg in msgs {
            match msg {
                Msg::Files(files) => {
                    self.slots = files
                        .into_iter()
                        .map(|path| Slot {
                            name: self.display_name(&path),
                            path,
                            result: None,
                        })
                        .collect();
                }
                Msg::Done(i, result) => {
                    if let Some(slot) = self.slots.get_mut(i) {
                        slot.result = Some(result);
                        self.done += 1;
                    }
                }
                Msg::Finished => {
                    self.scan = None;
                    if self.selected.is_none() {
                        self.selected = self.first_visible();
                    }
                }
            }
        }
    }

    fn visible(&self) -> Vec<usize> {
        let f = self.filter();
        let q = self.search.trim().to_lowercase();
        (0..self.slots.len())
            .filter(|&i| {
                f.matches(self.slots[i].outcome())
                    && (q.is_empty() || self.slots[i].name.to_lowercase().contains(&q))
            })
            .collect()
    }

    fn first_visible(&self) -> Option<usize> {
        self.visible().first().copied()
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.into_iter().find_map(|f| f.path) {
            self.start(path, ctx);
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (down, up) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
            )
        });
        if !(down || up) {
            return;
        }
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let pos = self.selected.and_then(|s| vis.iter().position(|&v| v == s));
        let next = match (pos, down) {
            (None, _) => 0,
            (Some(p), true) => (p + 1).min(vis.len() - 1),
            (Some(p), false) => p.saturating_sub(1),
        };
        self.selected = Some(vis[next]);
        self.scroll_to_selected = true;
    }

    fn say(&mut self, msg: String) {
        self.notice = Some((msg, Instant::now()));
    }

    fn export_csv(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("truelossless-report.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
        else {
            return;
        };
        let mut out = String::from(CSV_HEADER);
        out.push('\n');
        for s in &self.slots {
            match &s.result {
                Some(Ok(r)) => out.push_str(&csv_line(&s.path, Ok(r))),
                Some(Err(e)) => out.push_str(&csv_line(&s.path, Err(e))),
                None => continue,
            }
            out.push('\n');
        }
        let msg = match std::fs::write(&path, out) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Could not save CSV: {e}"),
        };
        self.say(msg);
    }

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        use style::{paint_icon, Icon};
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let (rect, drag) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 34.0),
            Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(10.0);
        let (logo, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
        style::paint_logo(ui.painter(), logo.center(), ACCENT);
        ui.label(style::heading("TRUELOSSLESS", 12.5));
        style::badge(&mut ui, concat!("v", env!("CARGO_PKG_VERSION")), TEXT_DIM);
        ui.add_space(12.0);
        let (txt, col) = if self.scanning() {
            ("Scanning", ACCENT)
        } else {
            ("Ready", GREEN)
        };
        ui.spacing_mut().item_spacing.x = 4.0;
        style::dot(&mut ui, col);
        ui.label(style::mono(txt, 11.5).color(TEXT_DIM));

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let btn = |ui: &mut egui::Ui, icon: Icon, hover: Color32| -> bool {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(46.0, 34.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::ZERO, hover);
                }
                paint_icon(
                    ui.painter(),
                    r.center(),
                    icon,
                    Color32::from_rgb(222, 226, 243),
                );
                resp.clicked()
            };
            if btn(ui, Icon::Close, Color32::from_rgb(196, 43, 28)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let max_icon = if maximized {
                Icon::Restore
            } else {
                Icon::Maximize
            };
            if btn(ui, max_icon, style::HIGH) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            if btn(ui, Icon::Minimize, style::HIGH) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        use style::{icon_button, Icon};
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if icon_button(ui, Icon::Folder, "Open folder", true, true).clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.start(dir, &ctx);
                }
            }
            if icon_button(ui, Icon::File, "Open file", false, true).clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("Audio", truelossless_core::scan::AUDIO_EXTENSIONS)
                    .pick_file()
                {
                    self.start(file, &ctx);
                }
            }
            let md5 = icon_button(ui, Icon::Check, "Verify FLAC MD5", self.verify_md5, true)
                .on_hover_text("Fully decodes each FLAC file to prove the audio is intact. Slow.");
            if md5.clicked() {
                self.verify_md5 = !self.verify_md5;
            }
            let has_results = self.slots.iter().any(|s| s.result.is_some());
            if icon_button(ui, Icon::Download, "Export CSV", false, has_results).clicked() {
                self.export_csv();
            }
        });

        if self.scanning() {
            let total = self.slots.len().max(1);
            ui.add(
                egui::ProgressBar::new(self.done as f32 / total as f32)
                    .text(style::mono(
                        format!("Checking {} / {}", self.done, self.slots.len()),
                        11.0,
                    ))
                    .fill(ACCENT)
                    .desired_height(14.0)
                    .corner_radius(CornerRadius::same(4)),
            );
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let c = Counts::of(&self.slots);
        if matches!(&self.notice, Some((_, t)) if t.elapsed() > Duration::from_secs(8)) {
            self.notice = None;
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let count = |ui: &mut egui::Ui, n: usize, label: &str, color: Color32| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    style::dot(ui, color);
                    ui.label(style::mono_bold(n.to_string(), 11.5));
                    ui.label(style::mono(label, 11.5).color(TEXT_DIM));
                });
            };
            for (n, label, status) in [
                (c.genuine, "Genuine", Status::Genuine),
                (c.suspect, "Suspect", Status::Suspect),
                (c.fake, "Fake", Status::Fake),
                (c.corrupt, "Corrupt", Status::Corrupt),
                (c.lossy, "Lossy", Status::Lossy),
            ] {
                count(ui, n, label, status_color(status));
            }
            if c.errors > 0 {
                count(ui, c.errors, "Unreadable", outcome_color(Outcome::Error));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let link = |ui: &mut egui::Ui, text: &str| {
                    ui.add(egui::Button::new(style::mono(text, 11.5).color(TEXT_DIM)).frame(false))
                        .clicked()
                };
                if link(ui, "ℹ About") {
                    self.about_open = true;
                }
                if link(ui, "⚙ Settings") {
                    self.settings_open = true;
                }
                if let Some((msg, _)) = &self.notice {
                    ui.label(style::mono(msg, 11.0).color(TEXT_DIM));
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                }
            });
        });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.about_open;
        egui::Window::new("About TrueLossless")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("TrueLossless").size(22.0).strong());
                ui.label(
                    RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM),
                );
                ui.add_space(6.0);
                ui.label("Checks whether lossless and hi-res audio files are the real thing.");
                ui.add_space(6.0);
                ui.label("Created by n4sh");
                ui.hyperlink_to(
                    "github.com/0n4sh0/truelossless",
                    env!("CARGO_PKG_REPOSITORY"),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("MIT License. Audio decoding by Symphonia, UI by egui.")
                        .small()
                        .color(TEXT_DIM),
                );
            });
        self.about_open = open;
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        enum Act {
            PickFile,
            PickFolder,
            Clear,
            Recheck,
        }
        let mut open = self.settings_open;
        let mut act: Option<Act> = None;
        let mut mode_changed = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("ffmpeg").strong());
                ui.label(
                    RichText::new(
                        "Used for formats TrueLossless cannot decode itself (Dolby Atmos, DTS, \
                         TrueHD and more), and as a fallback when the built-in decoder fails.",
                    )
                    .small()
                    .color(TEXT_DIM),
                );
                ui.add_space(4.0);
                match &self.ffmpeg {
                    FfmpegState::Found(t) => {
                        ui.label(
                            RichText::new(format!(
                                "●  ffmpeg {} found ({})",
                                t.version,
                                t.source.label()
                            ))
                            .color(status_color(Status::Genuine)),
                        );
                        ui.label(RichText::new(t.ffmpeg.display().to_string()).small());
                        let probe = match &t.ffprobe {
                            Some(p) => format!("ffprobe: {}", p.display()),
                            None => "ffprobe: not found (needed later for tags and cover art)"
                                .to_string(),
                        };
                        ui.label(RichText::new(probe).small().color(TEXT_DIM));
                    }
                    FfmpegState::Missing => {
                        ui.label(
                            RichText::new(
                                "●  Not found. Looked in the chosen path, TRUELOSSLESS_FFMPEG, \
                                 next to the program and PATH.",
                            )
                            .color(status_color(Status::Suspect)),
                        );
                    }
                    FfmpegState::Broken(e) => {
                        ui.label(
                            RichText::new(format!("●  {e}")).color(status_color(Status::Fake)),
                        );
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Choose file…").clicked() {
                        act = Some(Act::PickFile);
                    }
                    if ui.button("Choose folder…").clicked() {
                        act = Some(Act::PickFolder);
                    }
                    if ui
                        .add_enabled(
                            self.settings.ffmpeg_path.is_some(),
                            egui::Button::new("Clear choice"),
                        )
                        .clicked()
                    {
                        act = Some(Act::Clear);
                    }
                    if ui.button("Check again").clicked() {
                        act = Some(Act::Recheck);
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(RichText::new("Decoder for levels and spectrogram").strong());
                for (m, label) in [
                    (
                        DecoderMode::Auto,
                        "Automatic: built-in first, ffmpeg if it fails",
                    ),
                    (DecoderMode::Native, "Built-in only"),
                    (DecoderMode::Ffmpeg, "ffmpeg only"),
                ] {
                    if ui.radio_value(&mut self.settings.mode, m, label).changed() {
                        mode_changed = true;
                    }
                }
            });
        self.settings_open = open;

        let mut changed = mode_changed;
        match act {
            Some(Act::PickFile) => {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Choose ffmpeg")
                    .pick_file()
                {
                    self.settings.ffmpeg_path = Some(p);
                    changed = true;
                }
            }
            Some(Act::PickFolder) => {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Choose the folder that contains ffmpeg")
                    .pick_folder()
                {
                    self.settings.ffmpeg_path = Some(p);
                    changed = true;
                }
            }
            Some(Act::Clear) => {
                self.settings.ffmpeg_path = None;
                changed = true;
            }
            Some(Act::Recheck) => changed = true,
            None => {}
        }
        if changed {
            self.ffmpeg = self.settings.resolve();
            self.apply_backend();
            self.settings.save();
        }
    }

    fn queue_header(&mut self, ui: &mut egui::Ui) {
        use style::Icon;
        let ctx = ui.ctx().clone();
        let c = Counts::of(&self.slots);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(style::heading("Queue", 18.0).color(ui.visuals().strong_text_color()));
                style::badge(ui, &format!("{} FILES", self.slots.len()), ACCENT);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(h) = &self.scan {
                        if style::icon_click(ui, Icon::Close, "Stop scanning") {
                            h.cancel();
                        }
                    } else if let Some(root) = self.root.clone() {
                        if style::icon_click(ui, Icon::Sync, "Rescan folder") {
                            self.start(root, &ctx);
                        }
                    }
                });
            });
            egui::Frame::new()
                .fill(LOWEST)
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.spacing_mut().interact_size.y = 18.0;
                        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), Sense::hover());
                        style::paint_icon(ui.painter(), r.center(), Icon::Search, TEXT_DIM);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text(style::mono("Filter file name…", 11.5))
                                .font(egui::FontId::monospace(11.5))
                                .frame(egui::Frame::NONE)
                                .margin(Margin::ZERO)
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let problems = c.suspect + c.fake + c.corrupt + c.errors;
                style::badge(ui, &format!("{} OK", c.genuine + c.lossy), GREEN);
                if problems > 0 {
                    let word = if problems == 1 { "PROBLEM" } else { "PROBLEMS" };
                    style::badge(ui, &format!("{problems} {word}"), style::AMBER);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().interact_size.y = 22.0;
                    ui.spacing_mut().button_padding = egui::vec2(8.0, 2.0);
                    let short = |f: Filter| match f {
                        Filter::All => "All files",
                        Filter::Problems => "Problems",
                        Filter::Genuine => "Genuine",
                        Filter::Lossy => "Lossy",
                    };
                    let current = self.filter();
                    egui::ComboBox::from_id_salt("filter")
                        .selected_text(style::mono(short(current), 11.5))
                        .width(118.0)
                        .show_ui(ui, |ui| {
                            for f in Filter::CHOICES {
                                if ui.selectable_label(current == f, f.label()).clicked() {
                                    self.filter = Some(f);
                                }
                            }
                        });
                });
            });
        });
        ui.add_space(6.0);
    }

    fn file_list(&mut self, ui: &mut egui::Ui) {
        if self.root.is_some() {
            self.queue_header(ui);
        }
        if self.slots.is_empty() {
            ui.add_space(16.0);
            ui.label(
                RichText::new(if self.scanning() {
                    "Looking for audio files…"
                } else if self.root.is_some() {
                    "No audio files found."
                } else {
                    "Files appear here."
                })
                .color(TEXT_DIM),
            );
            return;
        }
        let vis = self.visible();
        if vis.is_empty() {
            ui.add_space(16.0);
            ui.label(RichText::new("No files match this filter.").color(TEXT_DIM));
            return;
        }

        // The selected row is taller: it adds the folder and file size.
        const ROW_H: f32 = 56.0;
        const SEL_H: f32 = 74.0;
        let heights: Vec<f32> = vis
            .iter()
            .map(|&i| {
                if self.selected == Some(i) {
                    SEL_H
                } else {
                    ROW_H
                }
            })
            .collect();
        let total: f32 = heights.iter().sum();
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let w = ui.available_width();
                let (area, _) = ui.allocate_exact_size(egui::vec2(w, total), Sense::hover());
                let mut y = 0.0;
                for (&i, &h) in vis.iter().zip(&heights) {
                    let local = egui::Rect::from_min_size(egui::pos2(0.0, y), egui::vec2(w, h));
                    let rect = local.translate(area.min.to_vec2());
                    y += h;
                    let selected = self.selected == Some(i);
                    if selected && scroll_to {
                        ui.scroll_to_rect(rect, None);
                    }
                    if !viewport.intersects(local) {
                        continue;
                    }
                    let resp = ui.interact(rect, ui.id().with(("row", i)), Sense::click());
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    self.paint_row(ui, i, rect, selected, resp.hovered());
                    resp.on_hover_text(self.slots[i].name.clone());
                }
            });
    }

    fn paint_row(&self, ui: &egui::Ui, i: usize, rect: egui::Rect, selected: bool, hovered: bool) {
        use style::{fit_text, paint_icon, Icon};
        let slot = &self.slots[i];
        let o = slot.outcome();
        let painter = ui.painter();
        let bg = rect.shrink2(egui::vec2(0.0, 3.0));
        let fill = if selected {
            style::HIGH
        } else if hovered {
            Color32::from_rgb(31, 36, 50)
        } else {
            style::CARD
        };
        painter.rect_filled(bg, CornerRadius::same(6), fill);
        if selected {
            painter.rect_filled(
                egui::Rect::from_min_size(bg.min, egui::vec2(3.0, bg.height())),
                CornerRadius {
                    nw: 6,
                    sw: 6,
                    ne: 0,
                    se: 0,
                },
                ACCENT,
            );
        }

        let problem =
            matches!(o, Outcome::Error) || matches!(o, Outcome::Done(s) if s >= Status::Suspect);
        let left = bg.left() + 12.0;
        let right = bg.right() - 10.0;
        let lines = [bg.top() + 16.0, bg.top() + 35.0, bg.top() + 53.0];
        let strong = ui.visuals().strong_text_color();
        let mono = FontId::monospace(11.0);

        // Line 1: icon, file name and verdict tag.
        let (icon, icon_color) = if problem {
            (Icon::Warning, outcome_color(o))
        } else if selected {
            (Icon::File, ACCENT)
        } else {
            (Icon::File, TEXT_DIM)
        };
        paint_icon(painter, egui::pos2(left + 5.0, lines[0]), icon, icon_color);
        let chip_w = chip_right(
            painter,
            egui::pos2(right, lines[0]),
            outcome_label(o),
            outcome_color(o),
        );
        let file = Path::new(&slot.name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| slot.name.clone());
        let name_font = if selected {
            FontId::new(13.5, style::bold_family())
        } else {
            FontId::proportional(13.5)
        };
        let name_left = left + 20.0;
        fit_text(
            painter,
            egui::pos2(name_left, lines[0]),
            file,
            name_font,
            strong,
            right - chip_w - 8.0 - name_left,
        );

        // Line 2: format, or a short reason for a problem, and the duration.
        let (detail, detail_color, duration) = match &slot.result {
            Some(Ok(r)) => {
                let detail = if problem {
                    let nyquist = r.info.sample_rate.map_or(0.0, |s| s as f32 / 2.0);
                    match &r.analysis {
                        Some(a) if a.cutoff.bandwidth_hz < nyquist * 0.98 => {
                            format!("Cutoff @ {} kHz", khz(a.cutoff.bandwidth_hz))
                        }
                        _ => r.verdict.reasons.first().cloned().unwrap_or_default(),
                    }
                } else {
                    row_summary(&r.info)
                };
                (
                    detail,
                    if problem { outcome_color(o) } else { TEXT_DIM },
                    r.info.duration_secs.map(fmt_duration).unwrap_or_default(),
                )
            }
            Some(Err(_)) => (
                "could not be read".to_string(),
                outcome_color(o),
                String::new(),
            ),
            None => ("checking…".to_string(), TEXT_DIM, String::new()),
        };
        let dur_w = painter
            .text(
                egui::pos2(right, lines[1]),
                Align2::RIGHT_CENTER,
                duration,
                FontId::new(11.0, style::mono_bold_family()),
                strong,
            )
            .width();
        fit_text(
            painter,
            egui::pos2(left, lines[1]),
            detail,
            mono.clone(),
            detail_color,
            right - dur_w - 10.0 - left,
        );

        // Line 3, selected only: full path and file size.
        if selected {
            let size = match &slot.result {
                Some(Ok(r)) => format!("{:.1} MB", r.info.size_bytes as f64 / 1_048_576.0),
                _ => String::new(),
            };
            let size_w = painter
                .text(
                    egui::pos2(right, lines[2]),
                    Align2::RIGHT_CENTER,
                    size,
                    mono.clone(),
                    ACCENT,
                )
                .width();
            fit_text(
                painter,
                egui::pos2(left, lines[2]),
                slot.path.display().to_string(),
                FontId::monospace(10.5),
                TEXT_DIM,
                right - size_w - 10.0 - left,
            );
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.22).max(24.0));
            card().show(ui, |ui| {
                ui.set_width(420.0);
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Are your files really lossless?")
                        .size(22.0)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Drop a folder here, or pick one. TrueLossless looks for lossy \
                         transcodes, upsampled hi-res and padded bit depth.",
                    )
                    .color(TEXT_DIM),
                );
                ui.add_space(14.0);
                let open =
                    egui::Button::new(RichText::new("Open folder").color(BG).strong().size(15.0))
                        .fill(ACCENT)
                        .min_size(egui::vec2(160.0, 38.0));
                if ui.add(open).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.start(dir, &ctx);
                    }
                }
                ui.add_space(8.0);
            });
        });
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        if self.root.is_none() {
            self.welcome(ui);
            return;
        }
        let Some(i) = self.selected.filter(|&i| i < self.slots.len()) else {
            ui.add_space(16.0);
            ui.label(RichText::new("Select a file to see its details.").color(TEXT_DIM));
            return;
        };
        let slot = &self.slots[i];
        match &slot.result {
            None => {
                ui.add_space(8.0);
                ui.heading(&slot.name);
                ui.label(RichText::new("Analysing…").color(TEXT_DIM));
            }
            Some(Err(e)) => {
                ui.add_space(8.0);
                ui.heading(&slot.name);
                tinted_card(outcome_color(Outcome::Error)).show(ui, |ui| {
                    ui.label(
                        RichText::new("Could not read this file")
                            .size(18.0)
                            .strong(),
                    );
                    ui.label(e);
                });
            }
            Some(Ok(r)) => report_view(ui, slot, r, &mut self.quality),
        }
    }
}

/// "44.1", "48", "22.05": kHz without trailing zeros.
fn khz(hz: f32) -> String {
    let s = format!("{:.2}", hz / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 13812672 -> "13,812,672".
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (k, ch) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn report_view(ui: &mut egui::Ui, slot: &Slot, r: &FileReport, quality: &mut QualityStore) {
    use style::{mono, mono_bold, Stat};
    let color = status_color(r.verdict.status);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            let i = &r.info;
            let file = Path::new(&slot.name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| slot.name.clone());

            // Identity: name and path on the left, format tags on the right.
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(style::heading(file, 22.0));
                    ui.label(mono(slot.path.display().to_string(), 11.0).color(TEXT_DIM));
                });
                ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if i.lossless {
                        style::badge(ui, "LOSSLESS", GREEN);
                    } else {
                        style::badge(ui, "LOSSY", status_color(Status::Lossy));
                    }
                    style::badge(ui, &i.codec.to_uppercase(), ACCENT);
                });
            });

            // Verdict.
            let used = r.analysis.as_ref().and_then(|a| a.effective_bits);
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Frame::new()
                    .fill(LOWEST)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (icon, _) =
                                ui.allocate_exact_size(egui::vec2(44.0, 44.0), Sense::hover());
                            ui.painter()
                                .rect_filled(icon, CornerRadius::same(6), alpha(color, 40));
                            let glyph = match r.verdict.status {
                                Status::Genuine => "✔",
                                Status::Lossy | Status::Unchecked => "ℹ",
                                Status::Suspect => "⚠",
                                Status::Corrupt | Status::Fake => "✕",
                            };
                            ui.painter().text(
                                icon.center(),
                                Align2::CENTER_CENTER,
                                glyph,
                                FontId::proportional(22.0),
                                color,
                            );
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                let mut title = r.verdict.status.label().to_uppercase();
                                if r.verdict.status == Status::Genuine && i.lossless {
                                    title.push_str(" LOSSLESS");
                                }
                                ui.label(style::heading(title, 22.0).color(color));
                                if let Some(first) = r.verdict.reasons.first() {
                                    ui.label(RichText::new(first).size(12.5).color(TEXT_DIM));
                                }
                            });
                            if let Some(a) = &r.analysis {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = 24.0;
                                    if let (Some(b), Some(e)) = (i.bits_per_sample, used) {
                                        ui.vertical(|ui| {
                                            ui.label(style::caption("BIT DEPTH USED"));
                                            ui.label(
                                                mono_bold(format!("{e} / {b}"), 15.0).color(
                                                    if e >= b { GREEN } else { style::AMBER },
                                                ),
                                            );
                                        });
                                    }
                                    ui.vertical(|ui| {
                                        ui.label(style::caption("CONTENT ENDS"));
                                        ui.label(mono_bold(
                                            format!("{} kHz", khz(a.cutoff.bandwidth_hz)),
                                            15.0,
                                        ));
                                    });
                                });
                            }
                        });
                    });

                let mut chips: Vec<String> = r.verdict.reasons.iter().skip(1).cloned().collect();
                if let Some(e) = &r.analysis_error {
                    chips.push(format!("analysis failed: {e}"));
                }
                for row in chips.chunks(3) {
                    ui.add_space(2.0);
                    ui.columns(3, |c| {
                        for (col, text) in c.iter_mut().zip(row) {
                            style::reason_chip(col, text, color);
                        }
                    });
                }
            });

            // Headline numbers.
            let rate = i.sample_rate;
            let dur = i.duration_secs;
            let kbps = dur
                .filter(|&d| d > 0.0)
                .map(|d| i.size_bytes as f64 * 8.0 / d / 1000.0);
            let tiles = [
                Stat {
                    label: "SAMPLE RATE",
                    value: rate.map_or("-".into(), |hz| khz(hz as f32)),
                    unit: "kHz",
                    note: rate.map_or(String::new(), |hz| {
                        format!("Nyquist: {} kHz", khz(hz as f32 / 2.0))
                    }),
                    color: None,
                    note_color: ACCENT,
                },
                {
                    let padded = matches!((i.bits_per_sample, used), (Some(b), Some(e)) if e < b);
                    Stat {
                        label: "BIT DEPTH",
                        value: i
                            .bits_per_sample
                            .map_or_else(|| bit_depth_text(i), |b| b.to_string()),
                        unit: if i.bits_per_sample.is_some() {
                            "bit"
                        } else {
                            ""
                        },
                        note: match (i.bits_per_sample, used) {
                            (Some(_), Some(e)) if padded => format!("only {e}-bit used"),
                            (Some(b), _) => format!("{:.1} dB dyn. window", b as f64 * 6.02),
                            _ => String::new(),
                        },
                        color: None,
                        note_color: if padded { style::AMBER } else { TEXT_DIM },
                    }
                },
                Stat {
                    label: "CONTENT ENDS",
                    value: r
                        .analysis
                        .as_ref()
                        .map_or("-".into(), |a| khz(a.cutoff.bandwidth_hz)),
                    unit: "kHz",
                    note: r.analysis.as_ref().map_or(String::new(), |a| {
                        format!(
                            "Edge: {:.0} dB @ {}k",
                            a.cutoff.cliff_db,
                            khz(a.cutoff.cliff_hz)
                        )
                    }),
                    color: r.analysis.as_ref().map(|_| color),
                    note_color: TEXT_DIM,
                },
                Stat {
                    label: "DURATION",
                    value: dur.map_or("-".into(), fmt_duration),
                    unit: "min",
                    note: match (dur, rate) {
                        (Some(d), Some(hz)) => {
                            format!("{} samples", thousands((d * hz as f64).round() as u64))
                        }
                        _ => String::new(),
                    },
                    color: None,
                    note_color: TEXT_DIM,
                },
                Stat {
                    label: "SIZE",
                    value: format!("{:.1}", i.size_bytes as f64 / 1_048_576.0),
                    unit: "MB",
                    note: kbps.map_or(String::new(), |k| {
                        format!("{} kbps", thousands(k.round() as u64))
                    }),
                    color: Some(ACCENT),
                    note_color: TEXT_DIM,
                },
            ];
            ui.columns(tiles.len(), |c| {
                for (col, s) in c.iter_mut().zip(&tiles) {
                    style::stat_tile(col, s, style::CARD, 17.0);
                }
            });

            quality_view::show_stats(ui, quality, &slot.path, r);

            if let Some(a) = &r.analysis {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        style::card_title(ui, "📈", "Average spectrum");
                        style::badge(
                            ui,
                            &format!("FFT {} · PEAK HOLD", truelossless_core::spectrum::FFT_SIZE),
                            TEXT_DIM,
                        );
                    });
                    ui.add_space(4.0);
                    plot::spectrum_plot(ui, &a.spectrum, &a.cutoff);
                });
            }

            quality_view::show_spectrogram(ui, quality, &slot.path);

            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut cells: Vec<(&str, String, Option<Color32>)> = vec![
                    (
                        "CONTAINER",
                        format!("{} (.{})", i.container.name(), i.extension),
                        None,
                    ),
                    (
                        "CODEC",
                        format!(
                            "{} ({})",
                            i.codec,
                            if i.lossless { "lossless" } else { "lossy" }
                        ),
                        None,
                    ),
                    (
                        "CHANNELS",
                        i.channels.map_or("-".into(), |v| v.to_string()),
                        None,
                    ),
                ];
                if let Some(a) = &r.analysis {
                    cells.push((
                        "STEEPEST EDGE",
                        format!(
                            "{:.0} dB @ {} kHz",
                            a.cutoff.cliff_db,
                            khz(a.cutoff.cliff_hz)
                        ),
                        Some(color),
                    ));
                }
                if let Some(m) = &r.md5 {
                    cells.push(match m.status {
                        Md5Status::Match => ("FLAC MD5", "verified".into(), Some(GREEN)),
                        Md5Status::Mismatch => (
                            "FLAC MD5",
                            "MISMATCH".into(),
                            Some(status_color(Status::Fake)),
                        ),
                        Md5Status::Unavailable => ("FLAC MD5", "not stored in file".into(), None),
                    });
                }
                ui.columns(cells.len(), |c| {
                    for (col, (k, v, tint)) in c.iter_mut().zip(&cells) {
                        col.spacing_mut().item_spacing.y = 3.0;
                        col.label(style::caption(*k));
                        let mut t = mono_bold(v, 12.5);
                        if let Some(tc) = tint {
                            t = t.color(*tc);
                        }
                        col.label(t);
                    }
                });
            });
            ui.add_space(8.0);
        });
}

fn drop_overlay(ctx: &egui::Context) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop_overlay"),
    ));
    painter.rect_filled(screen, CornerRadius::ZERO, alpha(BG, 225));
    painter.rect_stroke(
        screen.shrink(18.0),
        CornerRadius::same(16),
        egui::Stroke::new(2.0, ACCENT),
        egui::StrokeKind::Inside,
    );
    painter.text(
        screen.center(),
        Align2::CENTER_CENTER,
        "Drop to scan",
        FontId::proportional(32.0),
        ACCENT,
    );
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(path) = self.pending_open.take() {
            self.start(path, &ctx);
        }
        self.pump_messages();
        self.quality.poll();
        self.handle_drop(&ctx);
        self.handle_keys(&ctx);

        let bar = |top: i8, bottom: i8| {
            egui::Frame::new().fill(PANEL).inner_margin(Margin {
                left: 16,
                right: 16,
                top,
                bottom,
            })
        };
        egui::Panel::top("title")
            .frame(egui::Frame::new().fill(LOWEST))
            .show_separator_line(false)
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::top("top")
            .frame(bar(10, 10))
            .show_separator_line(false)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(bar(6, 6).fill(LOWEST))
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("files")
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(8, 8)),
            )
            .resizable(true)
            .show_separator_line(false)
            .default_size(440.0)
            .size_range(300.0..=720.0)
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(6, 0))
                    .show(ui, |ui| self.file_list(ui));
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(20, 12)),
            )
            .show(ui, |ui| self.details(ui));

        self.about_window(&ctx);
        self.settings_window(&ctx);
        drop_overlay(&ctx);
    }
}
