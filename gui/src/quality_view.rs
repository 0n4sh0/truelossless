//! On-demand audio quality analysis: background worker, stats card and a
//! spectrogram with per-channel tabs.

use crate::plot::khz;
use crate::style::{
    alpha, card, card_title, mono, mono_bold, segmented, stat_tile, Stat, ACCENT, AMBER, BORDER,
    GREEN, HIGH, LOWEST, TEXT_DIM,
};
use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, FontId, Layout, Pos2, Rect, RichText,
    Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use truelossless_core::cache;
use truelossless_core::quality::{cancelled, SPEC_DB_MIN, SPEC_WIN};
use truelossless_core::{
    analyze_quality_cancellable, Cutoff, DecoderMode, FileReport, Quality, QualityOptions,
    Toolchain,
};

const KEEP: usize = 8;

pub struct QView {
    q: Quality,
    layer: usize,
    textures: Vec<Option<TextureHandle>>,
}

pub enum QEntry {
    Done(Box<QView>),
    Failed(String),
}

/// A file waiting to be measured ahead of time.
pub struct Pending {
    pub path: PathBuf,
    pub header_only: bool,
    pub hint: Option<f64>,
}

/// Results of quality analyses: one job for the file on screen, plus one
/// background job that measures the rest of the scan into the disk cache.
pub struct QualityStore {
    map: HashMap<PathBuf, QEntry>,
    order: Vec<PathBuf>,
    running: Option<PathBuf>,
    /// Set to stop the on-screen job early.
    fg_cancel: Arc<AtomicBool>,
    /// Files the user chose not to measure; they wait for "Measure now".
    skipped: HashSet<PathBuf>,
    queue: VecDeque<Pending>,
    bg_current: Option<PathBuf>,
    /// Set to stop the background job early.
    bg_cancel: Arc<AtomicBool>,
    /// Files queued in this round, for the progress bar.
    bg_total: usize,
    bg_tx: Sender<()>,
    bg_rx: Receiver<()>,
    ffmpeg: Option<Toolchain>,
    mode: DecoderMode,
    tx: Sender<(PathBuf, Result<Quality, String>)>,
    rx: Receiver<(PathBuf, Result<Quality, String>)>,
}

impl Default for QualityStore {
    fn default() -> Self {
        let (tx, rx) = channel();
        let (bg_tx, bg_rx) = channel();
        QualityStore {
            map: HashMap::new(),
            order: Vec::new(),
            running: None,
            fg_cancel: Arc::new(AtomicBool::new(false)),
            skipped: HashSet::new(),
            queue: VecDeque::new(),
            bg_current: None,
            bg_cancel: Arc::new(AtomicBool::new(false)),
            bg_total: 0,
            bg_tx,
            bg_rx,
            ffmpeg: None,
            mode: DecoderMode::Auto,
            tx,
            rx,
        }
    }
}

impl QualityStore {
    /// Measure these files in the background, one at a time.
    pub fn queue_background(&mut self, files: Vec<Pending>) {
        let unsupported = !self.can_decode_unsupported();
        self.queue = files
            .into_iter()
            .filter(|f| !(f.header_only && unsupported))
            .collect();
        self.bg_total = self.queue.len();
    }

    /// Stop the file being measured ahead; the queue moves on to the next.
    pub fn cancel_current(&mut self) {
        self.bg_cancel.store(true, Ordering::Relaxed);
    }

    /// Stop measuring ahead altogether, including the current file.
    pub fn stop_background(&mut self) {
        self.queue.clear();
        self.cancel_current();
    }

    /// Files still to measure in the background, counting the current one.
    pub fn background_left(&self) -> usize {
        self.queue.len() + self.bg_current.is_some() as usize
    }

    /// Stop measuring `path` and leave it unmeasured until asked again.
    pub fn skip(&mut self, path: &Path) {
        self.skipped.insert(path.to_path_buf());
        if self.running.as_deref() == Some(path) {
            self.fg_cancel.store(true, Ordering::Relaxed);
        }
        if self.bg_current.as_deref() == Some(path) {
            self.cancel_current();
        }
    }

    /// Measure a skipped file after all.
    pub fn unskip(&mut self, path: &Path) {
        self.skipped.remove(path);
    }

    /// Files measured (or skipped) so far, and the round's total.
    pub fn background_progress(&self) -> (usize, usize) {
        let total = self.bg_total.max(self.background_left());
        (total - self.background_left(), total)
    }

    /// The file being measured ahead right now.
    pub fn background_current(&self) -> Option<&Path> {
        self.bg_current.as_deref()
    }

    /// Drop everything known about `path`, in memory and on disk.
    pub fn forget(&mut self, path: &Path) {
        self.map.remove(path);
        self.order.retain(|p| p != path);
        if let Some(d) = crate::settings::quality_dir() {
            cache::forget_quality(&d, path);
        }
    }

    /// Start the next background job when none is running.
    fn pump_background(&mut self, ctx: &egui::Context) {
        if self.bg_rx.try_recv().is_ok() {
            self.bg_current = None;
        }
        if self.bg_current.is_some() {
            return;
        }
        let Some(dir) = crate::settings::quality_dir() else {
            self.queue.clear();
            return;
        };
        while let Some(f) = self.queue.pop_front() {
            if self.map.contains_key(&f.path) || cache::has_quality(&dir, &f.path, self.mode) {
                continue;
            }
            self.bg_current = Some(f.path.clone());
            self.bg_cancel = Arc::new(AtomicBool::new(false));
            let cancel = self.bg_cancel.clone();
            let (tx, ctx, ffmpeg) = (self.bg_tx.clone(), ctx.clone(), self.ffmpeg.clone());
            let opts = QualityOptions {
                mode: self.mode,
                header_only: f.header_only,
                duration_hint: f.hint,
            };
            std::thread::spawn(move || {
                let res = analyze_quality_cancellable(&f.path, ffmpeg.as_ref(), opts, &cancel);
                if let Ok(q) = res {
                    let _ = cache::save_quality(&dir, &f.path, opts.mode, &q);
                }
                let _ = tx.send(());
                ctx.request_repaint();
            });
            return;
        }
    }

    /// Collect finished jobs and keep the background queue moving. Call
    /// every frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.pump_background(ctx);
        let stopped = cancelled().to_string();
        while let Ok((path, res)) = self.rx.try_recv() {
            if self.running.as_ref() == Some(&path) {
                self.running = None;
            }
            if matches!(&res, Err(e) if *e == stopped) {
                continue;
            }
            let entry = match res {
                Ok(q) => QEntry::Done(Box::new(QView {
                    layer: 0,
                    textures: vec![None; q.spectrogram.layers.len()],
                    q,
                })),
                Err(e) => QEntry::Failed(e),
            };
            self.order.retain(|p| p != &path);
            self.order.push(path.clone());
            self.map.insert(path, entry);
            while self.order.len() > KEEP {
                let old = self.order.remove(0);
                self.map.remove(&old);
            }
        }
    }

    /// Change the decoder setup. Earlier results may differ, so drop them.
    pub fn set_backend(&mut self, ffmpeg: Option<Toolchain>, mode: DecoderMode) {
        self.ffmpeg = ffmpeg;
        self.mode = mode;
        self.clear();
        self.queue.clear();
    }

    /// True when a format the built-in decoders lack can be measured.
    fn can_decode_unsupported(&self) -> bool {
        self.ffmpeg.is_some() && self.mode != DecoderMode::Native
    }

    /// Start analysing `path` if it is not known and nothing else is running.
    pub fn ensure(
        &mut self,
        path: &Path,
        ctx: &egui::Context,
        header_only: bool,
        hint: Option<f64>,
    ) {
        // A file the background job is measuring is picked up from the
        // cache once that job ends.
        if self.map.contains_key(path)
            || self.running.is_some()
            || self.skipped.contains(path)
            || self.bg_current.as_deref() == Some(path)
        {
            return;
        }
        self.running = Some(path.to_path_buf());
        self.fg_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.fg_cancel.clone();
        let (tx, ctx, p) = (self.tx.clone(), ctx.clone(), path.to_path_buf());
        let ffmpeg = self.ffmpeg.clone();
        let opts = QualityOptions {
            mode: self.mode,
            header_only,
            duration_hint: hint,
        };
        std::thread::spawn(move || {
            let dir = crate::settings::quality_dir();
            let cached = dir
                .as_deref()
                .and_then(|d| cache::load_quality(d, &p, opts.mode));
            let res = match cached {
                Some(q) => Ok(q),
                None => {
                    let res = analyze_quality_cancellable(&p, ffmpeg.as_ref(), opts, &cancel)
                        .map_err(|e| e.to_string());
                    if let (Some(d), Ok(q)) = (&dir, &res) {
                        let _ = cache::save_quality(d, &p, opts.mode, q);
                    }
                    res
                }
            };
            let _ = tx.send((p, res));
            ctx.request_repaint();
        });
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

/// Draw the levels card for `path`, starting the analysis if needed.
pub fn show_stats(ui: &mut egui::Ui, store: &mut QualityStore, path: &Path, r: &FileReport) {
    let header_only = r.info.header_only.is_some();
    if header_only && !store.can_decode_unsupported() {
        let why = if store.mode == DecoderMode::Native {
            "The decoder is set to built-in only, which cannot read this format. Change it in Settings."
        } else {
            "This format needs ffmpeg for levels and a spectrogram. Choose ffmpeg in Settings."
        };
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            card_title(ui, "📊", "Dynamics & levels");
            ui.label(RichText::new(why).color(TEXT_DIM));
        });
        return;
    }
    if store.skipped.contains(path) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Audio quality was not measured.").color(TEXT_DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Measure now").clicked() {
                        store.unskip(path);
                    }
                });
            });
        });
        return;
    }
    store.ensure(path, ui.ctx(), header_only, r.info.duration_secs);
    let Some(entry) = store.map.get(path) else {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
                ui.label(RichText::new("Measuring audio quality…").color(TEXT_DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if crate::style::icon_click(
                        ui,
                        crate::style::Icon::Skip,
                        "Skip measuring this file",
                    ) {
                        store.skip(path);
                    }
                });
            });
        });
        return;
    };
    match entry {
        QEntry::Failed(e) => {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                card_title(ui, "📊", "Dynamics & levels");
                ui.label(RichText::new(format!("Not available: {e}")).color(TEXT_DIM));
            });
        }
        QEntry::Done(v) => stats_card(ui, &v.q, r),
    }
}

/// Draw the spectrogram for `path` once its analysis is ready.
pub fn show_spectrogram(
    ui: &mut egui::Ui,
    store: &mut QualityStore,
    path: &Path,
    cut: Option<&Cutoff>,
) {
    if let Some(QEntry::Done(v)) = store.map.get_mut(path) {
        spectrogram_card(ui, v, cut);
    }
}

/// Split a level into number and unit for a tile, showing silence as -inf.
fn level(v: f64) -> String {
    if v <= -119.9 {
        "-inf".into()
    } else {
        format!("{v:.2}")
    }
}

fn stats_card(ui: &mut egui::Ui, q: &Quality, r: &FileReport) {
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            card_title(ui, "📊", "Dynamics & levels");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.label(mono(format!("Decoded with {}", q.decoder.name()), 12.0).color(TEXT_DIM));
                crate::style::dot(ui, GREEN);
            });
        });
        ui.add_space(4.0);
        let tp = q.true_peak_dbtp;
        let plain = |label, value, unit, note: &str| Stat {
            label,
            value,
            unit,
            note: note.to_string(),
            color: None,
            note_color: TEXT_DIM,
        };
        let warn = |on: bool| on.then_some(AMBER);
        let tiles = [
            plain(
                "DYN. RANGE",
                level(q.dynamic_range_db),
                "dB",
                "peak to quiet",
            ),
            plain(
                "SAMPLE PEAK",
                level(q.peak_db),
                "dBFS",
                &if q.peak_db < 0.0 {
                    format!("{:.1} dB headroom", -q.peak_db)
                } else {
                    "at full scale".to_string()
                },
            ),
            plain("RMS", level(q.rms_db), "dB", "average level"),
            plain(
                "LOUDNESS",
                q.lufs.map_or("-".into(), |l| format!("{l:.1}")),
                "LUFS",
                "integrated",
            ),
            Stat {
                color: warn(tp > 0.0),
                note_color: warn(tp > 0.0).unwrap_or(TEXT_DIM),
                ..plain(
                    "TRUE PEAK",
                    if tp <= -119.9 {
                        "-inf".into()
                    } else {
                        format!("{tp:+.2}")
                    },
                    "dBTP",
                    if tp > 0.0 {
                        "inter-sample over"
                    } else {
                        "no overs"
                    },
                )
            },
            Stat {
                color: warn(q.clipped > 0),
                note_color: warn(q.clipped > 0).unwrap_or(TEXT_DIM),
                ..plain(
                    "CLIPPING",
                    q.clipped.to_string(),
                    "smp",
                    if q.clipped > 0 {
                        "clipped samples"
                    } else {
                        "none"
                    },
                )
            },
        ];
        ui.columns(tiles.len(), |cols| {
            for (col, s) in cols.iter_mut().zip(&tiles) {
                stat_tile(col, s, LOWEST, 15.0);
            }
        });

        if q.per_channel.len() > 1 {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(alpha(HIGH, 150))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.spacing_mut().interact_size.y = 0.0;
                        let stereo = q.per_channel.len() == 2;
                        for (n, c) in q.per_channel.iter().enumerate().take(8) {
                            if n > 0 {
                                ui.add_space(18.0);
                            }
                            let name = match (stereo, n) {
                                (true, 0) => "Ch 1 (Left):".to_string(),
                                (true, _) => "Ch 2 (Right):".to_string(),
                                _ => format!("Ch {}:", n + 1),
                            };
                            ui.label(mono_bold(name, 13.0).color(ACCENT));
                            for (k, v) in [
                                ("Peak", format!("{:.1} dBFS", c.peak_db)),
                                ("RMS", format!("{:.1} dB", c.rms_db)),
                                ("DR", format!("{:.1}", c.dynamic_range_db)),
                            ] {
                                ui.label(mono(k, 13.0).color(TEXT_DIM));
                                ui.label(mono_bold(v, 13.0));
                            }
                            ui.label(
                                mono(format!("Clips: {}", c.clipped), 13.0)
                                    .color(if c.clipped > 0 { AMBER } else { TEXT_DIM }),
                            );
                        }
                    });
                });
        }

        let mut note = Vec::new();
        if let Some(b) = q.bitrate_bps {
            note.push(format!("{:.2} Mbps", b / 1e6));
        }
        note.push(match q.channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} channels"),
        });
        if r.info.note.as_deref().is_some_and(|n| n.contains("Atmos")) {
            note.push("5.1/7.1 bed only, Atmos objects are not included".into());
        }
        ui.add_space(4.0);
        ui.label(mono(note.join(" · "), 11.5).color(TEXT_DIM));
    });
}

/// Black → blue → purple → red → yellow → white.
fn heat(t: f32) -> Color32 {
    const STOPS: [(f32, [f32; 3]); 6] = [
        (0.0, [4.0, 4.0, 14.0]),
        (0.22, [34.0, 18.0, 130.0]),
        (0.42, [130.0, 24.0, 150.0]),
        (0.62, [225.0, 50.0, 60.0]),
        (0.82, [252.0, 175.0, 25.0]),
        (1.0, [255.0, 252.0, 205.0]),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            let u = (t - a.0) / (b.0 - a.0);
            let c = |k: usize| (a.1[k] + (b.1[k] - a.1[k]) * u) as u8;
            return Color32::from_rgb(c(0), c(1), c(2));
        }
    }
    Color32::WHITE
}

fn build_texture(ctx: &egui::Context, q: &Quality, layer: usize) -> TextureHandle {
    let s = &q.spectrogram;
    let (w, h) = (s.cols, s.rows);
    let mut rgba = vec![0u8; w * h * 4];
    for col in 0..w {
        for row in 0..h {
            let v = s.layers[layer][col * h + row] as f32 / 255.0;
            let c = heat(v);
            let y = h - 1 - row; // low frequencies at the bottom
            let o = (y * w + col) * 4;
            rgba[o..o + 4].copy_from_slice(&[c.r(), c.g(), c.b(), 255]);
        }
    }
    ctx.load_texture(
        format!("spectrogram-{layer}"),
        ColorImage::from_rgba_unmultiplied([w, h], &rgba),
        TextureOptions::LINEAR,
    )
}

fn nice_step(range: f32, max_ticks: f32, steps: &[f32]) -> f32 {
    steps
        .iter()
        .copied()
        .find(|s| range / s <= max_ticks)
        .unwrap_or(*steps.last().unwrap())
}

/// Tab names: the mix, then each channel, with stereo sides named.
fn layer_names(n: usize) -> Vec<String> {
    (0..n)
        .map(|l| match (l, n) {
            (0, 1) => "Mono".to_string(),
            (0, 3) => "Mix (Stereo)".to_string(),
            (0, _) => "Mix".to_string(),
            (1, 3) => "Ch 1 (Left)".to_string(),
            (2, 3) => "Ch 2 (Right)".to_string(),
            _ => format!("Ch {l}"),
        })
        .collect()
}

fn mmss(t: f32) -> String {
    let t = t.max(0.0);
    format!("{:02}:{:05.2}", (t / 60.0) as u32, t % 60.0)
}

/// Horizontal dashed line across `area` at `y` with a boxed label.
fn hline(p: &egui::Painter, area: Rect, y: f32, text: String, color: Color32) {
    p.add(egui::Shape::dashed_line(
        &[Pos2::new(area.left(), y), Pos2::new(area.right(), y)],
        Stroke::new(1.0, color),
        4.0,
        3.0,
    ));
    let galley = p.layout_no_wrap(text, FontId::monospace(11.5), color);
    let size = galley.size() + Vec2::new(10.0, 4.0);
    let top = (y - size.y - 3.0).max(area.top() + 2.0);
    let r = Rect::from_min_size(Pos2::new(area.right() - size.x - 6.0, top), size);
    p.rect_filled(r, CornerRadius::same(2), alpha(LOWEST, 225));
    p.galley(r.min + Vec2::new(5.0, 2.0), galley, color);
}

fn spectrogram_card(ui: &mut egui::Ui, v: &mut QView, cut: Option<&Cutoff>) {
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            card_title(ui, "▦", "Spectrogram");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let names = layer_names(v.q.spectrogram.layers.len());
                segmented(ui, &names, &mut v.layer);
                ui.add_space(12.0);
                let sr = v.q.spectrogram.sample_rate.max(1) as f32;
                ui.label(
                    mono(
                        format!(
                            "Resolution: {SPEC_WIN} pt / {:.0} ms",
                            SPEC_WIN as f32 / sr * 1000.0
                        ),
                        12.0,
                    )
                    .color(ACCENT),
                );
            });
        });
        ui.add_space(4.0);

        // Readout row, filled in once the hover position is known.
        let (readout, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());

        let layer = v.layer.min(v.q.spectrogram.layers.len() - 1);
        if v.textures[layer].is_none() {
            v.textures[layer] = Some(build_texture(ui.ctx(), &v.q, layer));
        }
        let tex = v.textures[layer].as_ref().unwrap();
        let s = &v.q.spectrogram;

        let width = ui.available_width().max(260.0);
        let (resp, painter) = ui.allocate_painter(Vec2::new(width, 330.0), Sense::hover());
        let full = resp.rect;
        let area = Rect::from_min_max(
            Pos2::new(full.left() + 44.0, full.top() + 4.0),
            Pos2::new(full.right() - 6.0, full.bottom() - 46.0),
        );
        painter.rect_filled(area, CornerRadius::same(4), LOWEST);
        painter.image(
            tex.id(),
            area,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        let font = FontId::monospace(11.5);
        let grid = Stroke::new(1.0, alpha(Color32::WHITE, 28));
        let nyq = s.nyquist();
        let fstep = nice_step(nyq, 6.0, &[1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0]);
        let mut f = 0.0;
        while f <= nyq + 1.0 {
            let y = area.bottom() - f / nyq * area.height();
            if f > 0.0 {
                painter.line_segment(
                    [Pos2::new(area.left(), y), Pos2::new(area.right(), y)],
                    grid,
                );
            }
            painter.text(
                Pos2::new(area.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                if f == 0.0 {
                    "0".to_string()
                } else {
                    format!("{:.0}k", f / 1000.0)
                },
                font.clone(),
                TEXT_DIM,
            );
            f += fstep;
        }
        let dur = s.duration_secs as f32;
        let tstep = nice_step(
            dur,
            8.0,
            &[
                1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
            ],
        );
        let mut t = 0.0;
        while t <= dur {
            let x = area.left() + t / dur * area.width();
            if t > 0.0 {
                painter.line_segment(
                    [Pos2::new(x, area.top()), Pos2::new(x, area.bottom())],
                    grid,
                );
            }
            let label = format!("{}:{:02}", t as u32 / 60, t as u32 % 60);
            painter.text(
                Pos2::new(x, area.bottom() + 4.0),
                Align2::CENTER_TOP,
                label,
                font.clone(),
                TEXT_DIM,
            );
            t += tstep;
        }

        // Band markers.
        let y_of = |hz: f32| area.bottom() - (hz / nyq).clamp(0.0, 1.0) * area.height();
        hline(
            &painter,
            area,
            area.top() + 1.0,
            format!("Nyquist {} kHz", khz(nyq)),
            TEXT_DIM,
        );
        if let Some(c) = cut {
            hline(
                &painter,
                area,
                y_of(c.bandwidth_hz),
                format!("content ends {} kHz", khz(c.bandwidth_hz)),
                AMBER,
            );
        }

        // Hover crosshair and readout.
        let rp = ui.painter();
        let mut x = readout.left();
        let mut field = |label: &str, value: String, color: Color32| {
            let g = rp.layout_no_wrap(format!("{label} "), FontId::monospace(12.0), TEXT_DIM);
            let w = g.size().x;
            rp.galley(Pos2::new(x, readout.top()), g, TEXT_DIM);
            x += w;
            let g = rp.layout_no_wrap(
                value,
                FontId::new(12.0, egui::FontFamily::Name("mono-bold".into())),
                color,
            );
            let w = g.size().x;
            rp.galley(Pos2::new(x, readout.top()), g, color);
            x += w + 22.0;
        };
        if let Some(c) = cut {
            field(
                "CUTOFF DELTA",
                format!("-{} kHz vs Nyquist", khz(nyq - c.bandwidth_hz)),
                GREEN,
            );
        }
        match resp.hover_pos().filter(|p| area.contains(*p)) {
            Some(p) => {
                let fx = ((p.x - area.left()) / area.width()).clamp(0.0, 1.0);
                let fy = ((area.bottom() - p.y) / area.height()).clamp(0.0, 1.0);
                let col = ((fx * s.cols as f32) as usize).min(s.cols - 1);
                let row = ((fy * s.rows as f32) as usize).min(s.rows - 1);
                let byte = s.layers[layer][col * s.rows + row] as f32;
                let db = SPEC_DB_MIN * (1.0 - byte / 255.0);
                let cross = Stroke::new(1.0, alpha(Color32::WHITE, 90));
                painter.line_segment(
                    [Pos2::new(p.x, area.top()), Pos2::new(p.x, area.bottom())],
                    cross,
                );
                painter.line_segment(
                    [Pos2::new(area.left(), p.y), Pos2::new(area.right(), p.y)],
                    cross,
                );
                field("TIME", mmss(fx * dur), ACCENT);
                field(
                    "FREQUENCY",
                    format!("{:.0} Hz", fy * nyq),
                    ui.visuals().strong_text_color(),
                );
                field("LEVEL", format!("{db:.1} dBFS"), AMBER);
            }
            None => field(
                "",
                "Hover the spectrogram to read time, frequency and level".into(),
                TEXT_DIM,
            ),
        }

        // Colour scale.
        let bar = Rect::from_min_max(
            Pos2::new(area.left() + 56.0, full.bottom() - 16.0),
            Pos2::new(area.right() - 56.0, full.bottom() - 8.0),
        );
        let slices = 96;
        for k in 0..slices {
            let x0 = bar.left() + bar.width() * k as f32 / slices as f32;
            let x1 = bar.left() + bar.width() * (k + 1) as f32 / slices as f32 + 0.5;
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, bar.top()), Pos2::new(x1, bar.bottom())),
                0.0,
                heat(k as f32 / (slices - 1) as f32),
            );
        }
        painter.rect_stroke(
            bar,
            CornerRadius::same(2),
            Stroke::new(1.0, BORDER),
            egui::StrokeKind::Outside,
        );
        painter.text(
            Pos2::new(bar.left() - 6.0, bar.center().y),
            Align2::RIGHT_CENTER,
            format!("{SPEC_DB_MIN:.0} dBFS"),
            font.clone(),
            TEXT_DIM,
        );
        painter.text(
            Pos2::new(bar.right() + 6.0, bar.center().y),
            Align2::LEFT_CENTER,
            "0 dBFS",
            font,
            TEXT_DIM,
        );

        // Analysis facts.
        ui.add_space(2.0);
        let mut facts = Vec::new();
        if let Some(c) = cut {
            facts.push(format!("Noise floor: {:.1} dB", c.floor_db));
        }
        facts.push(format!("Resolution: {} bins", s.rows));
        if s.cols > 0 {
            facts.push(format!("Hop: {:.0} ms", dur / s.cols as f32 * 1000.0));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(mono(facts.join("  ·  "), 12.0).color(TEXT_DIM));
        });
    });
}
