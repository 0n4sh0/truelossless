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
    fmt_duration, fmt_rate, outcome_color, outcome_label, status_color, Counts, Filter, Outcome,
    Slot,
};
use quality_view::QualityStore;
use settings::{FfmpegState, Settings};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use style::{alpha, card, chip, tile, tinted_card, ACCENT, BG, PANEL, TEXT_DIM};
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
        (0..self.slots.len())
            .filter(|&i| f.matches(self.slots[i].outcome()))
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("TrueLossless").size(18.0).strong());
            ui.add_space(10.0);

            let open =
                egui::Button::new(RichText::new("Open folder").color(BG).strong()).fill(ACCENT);
            if ui.add(open).clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.start(dir, &ctx);
                }
            }
            if ui.button("Open file").clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("Audio", truelossless_core::scan::AUDIO_EXTENSIONS)
                    .pick_file()
                {
                    self.start(file, &ctx);
                }
            }
            ui.checkbox(&mut self.verify_md5, "Verify FLAC MD5")
                .on_hover_text("Fully decodes each FLAC file to prove the audio is intact. Slow.");

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let has_results = self.slots.iter().any(|s| s.result.is_some());
                if ui
                    .add_enabled(has_results, egui::Button::new("Export CSV"))
                    .clicked()
                {
                    self.export_csv();
                }

                let current = self.filter();
                egui::ComboBox::from_id_salt("filter")
                    .selected_text(current.label())
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for f in Filter::CHOICES {
                            if ui.selectable_label(current == f, f.label()).clicked() {
                                self.filter = Some(f);
                            }
                        }
                    });

                if let Some(h) = &self.scan {
                    if ui.button("Stop").clicked() {
                        h.cancel();
                    }
                } else if let Some(root) = self.root.clone() {
                    if ui.button("Rescan").clicked() {
                        self.start(root, &ctx);
                    }
                }
            });
        });

        if self.scanning() {
            let total = self.slots.len().max(1);
            ui.add(
                egui::ProgressBar::new(self.done as f32 / total as f32)
                    .text(format!("Checking {} / {}", self.done, self.slots.len()))
                    .fill(ACCENT)
                    .desired_height(16.0)
                    .corner_radius(CornerRadius::same(8)),
            );
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let c = Counts::of(&self.slots);
        if matches!(&self.notice, Some((_, t)) if t.elapsed() > Duration::from_secs(8)) {
            self.notice = None;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} files", self.slots.len())).color(TEXT_DIM));
            for (n, label, status) in [
                (c.genuine, "genuine", Status::Genuine),
                (c.suspect, "suspect", Status::Suspect),
                (c.fake, "fake", Status::Fake),
                (c.corrupt, "corrupt", Status::Corrupt),
                (c.lossy, "lossy", Status::Lossy),
            ] {
                ui.label(RichText::new(format!("●  {n} {label}")).color(status_color(status)));
            }
            if c.errors > 0 {
                ui.label(
                    RichText::new(format!("●  {} unreadable", c.errors))
                        .color(outcome_color(Outcome::Error)),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("About").clicked() {
                    self.about_open = true;
                }
                if ui.small_button("Settings").clicked() {
                    self.settings_open = true;
                }
                ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
                if let Some((msg, _)) = &self.notice {
                    ui.label(RichText::new(msg).color(TEXT_DIM));
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

    fn file_list(&mut self, ui: &mut egui::Ui) {
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

        const ROW_H: f32 = 46.0;
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_H, vis.len(), |ui, range| {
                for &i in &vis[range] {
                    let slot = &self.slots[i];
                    let o = slot.outcome();
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), ROW_H),
                        Sense::click(),
                    );
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    let selected = self.selected == Some(i);
                    if selected && scroll_to {
                        resp.scroll_to_me(None);
                    }

                    let painter = ui.painter();
                    let bg = rect.shrink2(egui::vec2(0.0, 1.0));
                    if selected {
                        painter.rect_filled(bg, CornerRadius::same(8), alpha(ACCENT, 34));
                    } else if resp.hovered() {
                        painter.rect_filled(bg, CornerRadius::same(8), alpha(Color32::WHITE, 10));
                    }

                    let cy = rect.center().y;
                    let chip_w = chip(
                        painter,
                        egui::pos2(rect.left() + 10.0, cy),
                        outcome_label(o),
                        outcome_color(o),
                    );
                    let text_x = rect.left() + 10.0 + chip_w + 12.0;

                    // Right-aligned format summary.
                    let mut meta_left = rect.right() - 10.0;
                    if let Some(Ok(r)) = &slot.result {
                        let meta = [
                            r.info.sample_rate.map(fmt_rate),
                            r.info.bits_per_sample.map(|b| format!("{b}-bit")),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                        let g = painter.layout_no_wrap(meta, FontId::proportional(11.5), TEXT_DIM);
                        meta_left = rect.right() - 10.0 - g.size().x;
                        painter.galley(egui::pos2(meta_left, cy - g.size().y / 2.0), g, TEXT_DIM);
                    }

                    let clip = egui::Rect::from_min_max(
                        egui::pos2(text_x, rect.top()),
                        egui::pos2((meta_left - 10.0).max(text_x), rect.bottom()),
                    );
                    let p = painter.with_clip_rect(clip);
                    let path = Path::new(&slot.name);
                    let file = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| slot.name.clone());
                    let dir = path
                        .parent()
                        .map(|d| d.display().to_string())
                        .filter(|d| !d.is_empty());
                    let text_color = ui.visuals().text_color();
                    if let Some(dir) = dir {
                        p.text(
                            egui::pos2(text_x, cy - 8.0),
                            Align2::LEFT_CENTER,
                            file,
                            FontId::proportional(14.0),
                            text_color,
                        );
                        p.text(
                            egui::pos2(text_x, cy + 9.0),
                            Align2::LEFT_CENTER,
                            dir,
                            FontId::proportional(11.0),
                            TEXT_DIM,
                        );
                    } else {
                        p.text(
                            egui::pos2(text_x, cy),
                            Align2::LEFT_CENTER,
                            file,
                            FontId::proportional(14.0),
                            text_color,
                        );
                    }
                }
            });
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

fn report_view(ui: &mut egui::Ui, slot: &Slot, r: &FileReport, quality: &mut QualityStore) {
    let color = status_color(r.verdict.status);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);
            let file = Path::new(&slot.name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| slot.name.clone());
            ui.label(RichText::new(file).size(20.0).strong());
            ui.label(
                RichText::new(slot.path.display().to_string())
                    .small()
                    .color(TEXT_DIM),
            );
            ui.add_space(4.0);

            tinted_card(color).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(r.verdict.status.label())
                        .size(28.0)
                        .strong()
                        .color(color),
                );
                ui.add_space(2.0);
                for reason in &r.verdict.reasons {
                    ui.label(RichText::new(format!("•  {reason}")).size(14.5));
                }
                if let Some(e) = &r.analysis_error {
                    ui.label(RichText::new(format!("•  analysis failed: {e}")).color(TEXT_DIM));
                }
            });

            let i = &r.info;
            ui.horizontal_wrapped(|ui| {
                tile(
                    ui,
                    "SAMPLE RATE",
                    &i.sample_rate.map_or("-".into(), fmt_rate),
                );
                let used = r.analysis.as_ref().and_then(|a| a.effective_bits);
                tile(
                    ui,
                    "BIT DEPTH",
                    &match (i.bits_per_sample, used) {
                        (Some(b), Some(e)) if e != b => format!("{b}-bit ({e} used)"),
                        (Some(b), _) => format!("{b}-bit"),
                        (None, Some(e)) => format!("{e}-bit used"),
                        (None, None) => "-".into(),
                    },
                );
                if let Some(a) = &r.analysis {
                    tile(
                        ui,
                        "CONTENT ENDS",
                        &format!("{:.1} kHz", a.cutoff.bandwidth_hz / 1000.0),
                    );
                }
                tile(
                    ui,
                    "DURATION",
                    &i.duration_secs.map_or("-".into(), fmt_duration),
                );
                tile(
                    ui,
                    "SIZE",
                    &format!("{:.1} MB", i.size_bytes as f64 / 1_048_576.0),
                );
            });

            if let Some(a) = &r.analysis {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("AVERAGE SPECTRUM").small().color(TEXT_DIM));
                    ui.add_space(4.0);
                    plot::spectrum_plot(ui, &a.spectrum, &a.cutoff);
                });
            }

            quality_view::show(ui, quality, &slot.path, r);

            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("details")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        let mut row = |k: &str, v: String| {
                            ui.label(RichText::new(k).color(TEXT_DIM));
                            ui.label(v);
                            ui.end_row();
                        };
                        row(
                            "Container",
                            format!("{} (extension .{})", i.container.name(), i.extension),
                        );
                        row(
                            "Codec",
                            format!(
                                "{} ({})",
                                i.codec,
                                if i.lossless { "lossless" } else { "lossy" }
                            ),
                        );
                        row("Channels", i.channels.map_or("-".into(), |v| v.to_string()));
                        if let Some(a) = &r.analysis {
                            row(
                                "Steepest edge",
                                format!(
                                    "{:.0} dB at {:.1} kHz",
                                    a.cutoff.cliff_db,
                                    a.cutoff.cliff_hz / 1000.0
                                ),
                            );
                        }
                        if let Some(m) = &r.md5 {
                            row(
                                "FLAC MD5",
                                match m.status {
                                    Md5Status::Match => "verified".into(),
                                    Md5Status::Mismatch => "MISMATCH".into(),
                                    Md5Status::Unavailable => "not stored in file".into(),
                                },
                            );
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
        egui::Panel::top("top")
            .frame(bar(12, 12))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(bar(6, 6))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("files")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(8, 8)),
            )
            .resizable(true)
            .default_size(440.0)
            .size_range(300.0..=720.0)
            .show(ui, |ui| self.file_list(ui));
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
