#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod model;
mod plot;
mod worker;

use eframe::egui::{self, RichText};
use flac_checker_core::{CheckOptions, FileReport, Md5Status};
use model::{outcome_color, outcome_tag, status_color, Counts, Filter, Outcome, Slot};
use std::path::{Path, PathBuf};
use worker::{start_scan, Msg, ScanHandle};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 680.0])
            .with_min_inner_size([760.0, 440.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "FLAC Checker",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}

#[derive(Default)]
struct App {
    root: Option<PathBuf>,
    slots: Vec<Slot>,
    done: usize,
    scan: Option<ScanHandle>,
    selected: Option<usize>,
    filter: Option<Filter>,
    verify_md5: bool,
}

impl App {
    fn filter(&self) -> Filter {
        self.filter.unwrap_or(Filter::All)
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
        let pos = self
            .selected
            .and_then(|s| vis.iter().position(|&v| v == s));
        let next = match (pos, down) {
            (None, _) => 0,
            (Some(p), true) => (p + 1).min(vis.len() - 1),
            (Some(p), false) => p.saturating_sub(1),
        };
        self.selected = Some(vis[next]);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui.button("Open folder…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.start(dir, &ctx);
                }
            }
            if ui.button("Open file…").clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("Audio", flac_checker_core::scan::AUDIO_EXTENSIONS)
                    .pick_file()
                {
                    self.start(file, &ctx);
                }
            }
            ui.checkbox(&mut self.verify_md5, "Verify FLAC MD5 (slow)")
                .on_hover_text("Fully decodes each FLAC file to prove the audio is intact.");

            if let Some(root) = self.root.clone() {
                if ui
                    .add_enabled(!self.scanning(), egui::Button::new("Rescan"))
                    .clicked()
                {
                    self.start(root, &ctx);
                }
            }
            if let Some(h) = &self.scan {
                if ui.button("Stop").clicked() {
                    h.cancel();
                }
            }

            ui.separator();
            let current = self.filter();
            egui::ComboBox::from_id_salt("filter")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for f in Filter::CHOICES {
                        if ui.selectable_label(current == f, f.label()).clicked() {
                            self.filter = Some(f);
                        }
                    }
                });
        });

        if self.scanning() {
            let total = self.slots.len().max(1);
            ui.add(
                egui::ProgressBar::new(self.done as f32 / total as f32)
                    .text(format!("{} / {}", self.done, self.slots.len()))
                    .desired_height(14.0),
            );
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let c = Counts::of(&self.slots);
        ui.horizontal(|ui| {
            ui.label(format!("{} files", self.slots.len()));
            ui.separator();
            ui.label(RichText::new(format!("{} genuine", c.genuine)).color(status_color(flac_checker_core::Status::Genuine)));
            ui.label(RichText::new(format!("{} suspect", c.suspect)).color(status_color(flac_checker_core::Status::Suspect)));
            ui.label(RichText::new(format!("{} fake", c.fake)).color(status_color(flac_checker_core::Status::Fake)));
            ui.label(RichText::new(format!("{} corrupt", c.corrupt)).color(status_color(flac_checker_core::Status::Corrupt)));
            ui.label(RichText::new(format!("{} lossy", c.lossy)).color(status_color(flac_checker_core::Status::Lossy)));
            if c.errors > 0 {
                ui.label(format!("{} unreadable", c.errors));
            }
        });
    }

    fn file_list(&mut self, ui: &mut egui::Ui) {
        if self.slots.is_empty() {
            ui.add_space(12.0);
            ui.label(if self.scanning() {
                "Looking for audio files…"
            } else {
                "Open a folder, or drop one here."
            });
            return;
        }
        let vis = self.visible();
        if vis.is_empty() {
            ui.add_space(12.0);
            ui.label("No files match this filter.");
            return;
        }
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 6.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
            ui,
            row_h,
            vis.len(),
            |ui, range| {
                for &i in &vis[range] {
                    let slot = &self.slots[i];
                    let o = slot.outcome();
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        outcome_tag(o),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::monospace(12.0),
                            color: outcome_color(o),
                            ..Default::default()
                        },
                    );
                    job.append(
                        &format!("  {}", slot.name),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(13.0),
                            color: ui.visuals().text_color(),
                            ..Default::default()
                        },
                    );
                    let resp = ui.selectable_label(self.selected == Some(i), job);
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    if self.selected == Some(i) {
                        resp.scroll_to_me(None);
                    }
                }
            },
        );
    }

    fn details(&self, ui: &mut egui::Ui) {
        let Some(i) = self.selected else {
            ui.add_space(12.0);
            ui.label("Select a file to see its details.");
            return;
        };
        let slot = &self.slots[i];
        match &slot.result {
            None => {
                ui.heading(&slot.name);
                ui.label("Analysing…");
            }
            Some(Err(e)) => {
                ui.heading(&slot.name);
                ui.label(RichText::new("Could not read this file").color(outcome_color(Outcome::Error)));
                ui.label(e);
            }
            Some(Ok(r)) => report_view(ui, slot, r),
        }
    }
}

fn report_view(ui: &mut egui::Ui, slot: &Slot, r: &FileReport) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.label(RichText::new(&slot.name).strong());
        ui.label(RichText::new(slot.path.display().to_string()).weak().small());
        ui.add_space(4.0);

        ui.label(
            RichText::new(r.verdict.status.label())
                .size(26.0)
                .strong()
                .color(status_color(r.verdict.status)),
        );
        for reason in &r.verdict.reasons {
            ui.label(format!("•  {reason}"));
        }
        if let Some(e) = &r.analysis_error {
            ui.label(RichText::new(format!("•  analysis failed: {e}")).weak());
        }
        ui.add_space(8.0);

        if let Some(a) = &r.analysis {
            plot::spectrum_plot(ui, &a.spectrum, &a.cutoff);
            ui.add_space(8.0);
        }

        let i = &r.info;
        egui::Grid::new("details").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(RichText::new(k).weak());
                ui.label(v);
                ui.end_row();
            };
            row("Container", format!("{} (extension .{})", i.container.name(), i.extension));
            row("Codec", format!("{}{}", i.codec, if i.lossless { "  (lossless)" } else { "  (lossy)" }));
            row("Sample rate", i.sample_rate.map_or("-".into(), |v| format!("{v} Hz")));
            row(
                "Bit depth",
                match (i.bits_per_sample, r.analysis.as_ref().and_then(|a| a.effective_bits)) {
                    (Some(b), Some(e)) if e != b => format!("{b}-bit declared, {e}-bit used"),
                    (Some(b), _) => format!("{b}-bit"),
                    (None, Some(e)) => format!("{e}-bit used"),
                    (None, None) => "-".into(),
                },
            );
            row("Channels", i.channels.map_or("-".into(), |v| v.to_string()));
            row(
                "Duration",
                i.duration_secs
                    .map_or("-".into(), |d| format!("{}:{:02}", d as u64 / 60, d as u64 % 60)),
            );
            row("File size", format!("{:.1} MB", i.size_bytes as f64 / 1_048_576.0));
            if let Some(a) = &r.analysis {
                row("Content ends at", format!("{:.1} kHz", a.cutoff.bandwidth_hz / 1000.0));
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_messages();
        self.handle_drop(&ctx);
        self.handle_keys(&ctx);

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(2.0);
            self.status_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::left("files")
            .resizable(true)
            .default_size(430.0)
            .size_range(260.0..=720.0)
            .show(ui, |ui| self.file_list(ui));
        egui::CentralPanel::default().show(ui, |ui| self.details(ui));
    }
}
