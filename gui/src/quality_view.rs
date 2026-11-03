//! On-demand audio quality analysis: background worker, stats card and a
//! spectrogram with per-channel tabs.

use crate::style::{
    alpha, card, card_title, mono, mono_bold, segmented, stat_tile, Stat, ACCENT, AMBER, BORDER,
    GREEN, HIGH, LOWEST, TEXT_DIM,
};
use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, FontId, Layout, Pos2, Rect, RichText,
    Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use truelossless_core::cache;
use truelossless_core::quality::SPEC_DB_MIN;
use truelossless_core::{
    analyze_quality_with, DecoderMode, FileReport, Quality, QualityOptions, Toolchain,
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

/// Results of quality analyses, one background job at a time.
pub struct QualityStore {
    map: HashMap<PathBuf, QEntry>,
    order: Vec<PathBuf>,
    running: Option<PathBuf>,
    ffmpeg: Option<Toolchain>,
    mode: DecoderMode,
    tx: Sender<(PathBuf, Result<Quality, String>)>,
    rx: Receiver<(PathBuf, Result<Quality, String>)>,
}

impl Default for QualityStore {
    fn default() -> Self {
        let (tx, rx) = channel();
        QualityStore {
            map: HashMap::new(),
            order: Vec::new(),
            running: None,
            ffmpeg: None,
            mode: DecoderMode::Auto,
            tx,
            rx,
        }
    }
}

impl QualityStore {
    /// Collect finished jobs. Call every frame.
    pub fn poll(&mut self) {
        while let Ok((path, res)) = self.rx.try_recv() {
            if self.running.as_ref() == Some(&path) {
                self.running = None;
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
        if self.map.contains_key(path) || self.running.is_some() {
            return;
        }
        self.running = Some(path.to_path_buf());
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
                    let res =
                        analyze_quality_with(&p, ffmpeg.as_ref(), opts).map_err(|e| e.to_string());
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
    store.ensure(path, ui.ctx(), header_only, r.info.duration_secs);
    let Some(entry) = store.map.get(path) else {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Measuring audio quality…").color(TEXT_DIM));
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
pub fn show_spectrogram(ui: &mut egui::Ui, store: &mut QualityStore, path: &Path) {
    if let Some(QEntry::Done(v)) = store.map.get_mut(path) {
        spectrogram_card(ui, v);
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
                ui.label(mono(format!("Decoded with {}", q.decoder.name()), 10.5).color(TEXT_DIM));
                ui.label(RichText::new("●").size(8.0).color(GREEN));
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
                stat_tile(col, s, LOWEST, 13.5);
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
                            ui.label(mono_bold(name, 11.5).color(ACCENT));
                            for (k, v) in [
                                ("Peak", format!("{:.1} dBFS", c.peak_db)),
                                ("RMS", format!("{:.1} dB", c.rms_db)),
                                ("DR", format!("{:.1}", c.dynamic_range_db)),
                            ] {
                                ui.label(mono(k, 11.5).color(TEXT_DIM));
                                ui.label(mono_bold(v, 11.5));
                            }
                            ui.label(
                                mono(format!("Clips: {}", c.clipped), 11.5)
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
        ui.label(mono(note.join(" · "), 10.5).color(TEXT_DIM));
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

fn spectrogram_card(ui: &mut egui::Ui, v: &mut QView) {
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            card_title(ui, "▦", "Spectrogram");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = v.q.spectrogram.layers.len();
                let names: Vec<String> = (0..n)
                    .map(|l| match (l, n) {
                        (0, 1) => "Mono".to_string(),
                        (0, _) => "Mix".to_string(),
                        _ => format!("Ch {l}"),
                    })
                    .collect();
                segmented(ui, &names, &mut v.layer);
            });
        });
        ui.add_space(4.0);

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

        let font = FontId::monospace(10.5);
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
    });
}
