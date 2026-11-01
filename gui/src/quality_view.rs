//! On-demand audio quality analysis: background worker, stats card and a
//! spectrogram with per-channel tabs.

use crate::model::{fmt_duration, fmt_rate};
use crate::style::{alpha, card, BG, BORDER, TEXT_DIM};
use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Vec2,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use truelossless_core::quality::SPEC_DB_MIN;
use truelossless_core::{analyze_quality, FileReport, Quality};

const AMBER: Color32 = Color32::from_rgb(245, 180, 50);
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

    /// Start analysing `path` if it is not known and nothing else is running.
    pub fn ensure(&mut self, path: &Path, ctx: &egui::Context) {
        if self.map.contains_key(path) || self.running.is_some() {
            return;
        }
        self.running = Some(path.to_path_buf());
        let (tx, ctx, p) = (self.tx.clone(), ctx.clone(), path.to_path_buf());
        std::thread::spawn(move || {
            let res = analyze_quality(&p).map_err(|e| e.to_string());
            let _ = tx.send((p, res));
            ctx.request_repaint();
        });
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

/// Draw the quality card and the spectrogram for `path`.
pub fn show(ui: &mut egui::Ui, store: &mut QualityStore, path: &Path, r: &FileReport) {
    store.ensure(path, ui.ctx());
    let Some(entry) = store.map.get_mut(path) else {
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
                ui.label(
                    RichText::new("AUDIO QUALITY ANALYSIS")
                        .small()
                        .color(TEXT_DIM),
                );
                ui.label(RichText::new(format!("Not available: {e}")).color(TEXT_DIM));
            });
        }
        QEntry::Done(v) => {
            stats_card(ui, &v.q, r);
            spectrogram_card(ui, v);
        }
    }
}

fn db(v: f64) -> String {
    if v <= -119.9 {
        "-inf dB".into()
    } else {
        format!("{v:.2} dB")
    }
}

fn stats_card(ui: &mut egui::Ui, q: &Quality, r: &FileReport) {
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new("AUDIO QUALITY ANALYSIS")
                .small()
                .color(TEXT_DIM),
        );
        ui.add_space(6.0);
        let i = &r.info;
        let stored = i.bits_per_sample.map_or("-".into(), |b| format!("{b}-bit"));
        let layout = match q.channels {
            1 => "1 (mono)".to_string(),
            2 => "2 (stereo)".to_string(),
            n => n.to_string(),
        };
        let cutoff = r
            .analysis
            .as_ref()
            .map_or("Not available".to_string(), |a| {
                if a.cutoff.bandwidth_hz >= q.sample_rate as f32 / 2.0 * 0.98 {
                    "Not detected".to_string()
                } else {
                    format!("{:.1} kHz", a.cutoff.bandwidth_hz / 1000.0)
                }
            });
        let mut rows: Vec<(&str, String, Option<Color32>)> = vec![
            ("Codec", i.codec.clone(), None),
            ("Sample rate", fmt_rate(q.sample_rate), None),
            ("Bit depth", stored, None),
            (
                "Bitrate",
                q.bitrate_bps
                    .map_or("-".into(), |b| format!("{:.2} Mbps", b / 1e6)),
                None,
            ),
            ("Channels", layout, None),
            ("Duration", fmt_duration(q.duration_secs), None),
            (
                "Nyquist",
                format!("{:.1} kHz", q.sample_rate as f32 / 2000.0),
                None,
            ),
            (
                "Size",
                format!("{:.1} MB", i.size_bytes as f64 / 1_048_576.0),
                None,
            ),
        ];
        grid(ui, "q_basic", &rows);
        rows.clear();
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
        let clip = (q.clipped > 0).then_some(AMBER);
        rows.extend([
            ("Dynamic range", db(q.dynamic_range_db), None),
            ("Peak", db(q.peak_db), None),
            ("RMS", db(q.rms_db), None),
            (
                "Loudness",
                q.lufs.map_or("-".into(), |l| format!("{l:.1} LUFS")),
                None,
            ),
            (
                "True peak",
                if q.true_peak_dbtp <= -119.9 {
                    "-inf dBTP".into()
                } else {
                    format!("{:.2} dBTP", q.true_peak_dbtp)
                },
                (q.true_peak_dbtp > 0.0).then_some(AMBER),
            ),
            ("Clipping", q.clipped.to_string(), clip),
            ("Spectral cutoff", cutoff, None),
            (
                "Samples",
                format!(
                    "{:.1}M",
                    q.duration_secs * q.sample_rate as f64 * q.channels as f64 / 1e6
                ),
                None,
            ),
        ]);
        grid(ui, "q_levels", &rows);

        if q.per_channel.len() > 1 {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(RichText::new("Per-channel").small().color(TEXT_DIM));
            for (n, c) in q.per_channel.iter().enumerate().take(8) {
                ui.label(
                    RichText::new(format!(
                        "Ch {}:  Peak {:.1}  /  RMS {:.1}  /  DR {:.1}  /  Clip {}",
                        n + 1,
                        c.peak_db,
                        c.rms_db,
                        c.dynamic_range_db,
                        c.clipped
                    ))
                    .monospace()
                    .size(12.5),
                );
            }
        }
    });
}

fn grid(ui: &mut egui::Ui, id: &str, rows: &[(&str, String, Option<Color32>)]) {
    egui::Grid::new(id)
        .num_columns(4)
        .spacing([18.0, 7.0])
        .min_col_width(70.0)
        .show(ui, |ui| {
            for (n, (k, v, color)) in rows.iter().enumerate() {
                ui.label(RichText::new(*k).color(TEXT_DIM));
                let mut text = RichText::new(v).strong();
                if let Some(c) = color {
                    text = text.color(*c);
                }
                ui.label(text);
                if n % 2 == 1 {
                    ui.end_row();
                }
            }
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
            ui.label(RichText::new("SPECTROGRAM").small().color(TEXT_DIM));
            ui.add_space(8.0);
            let n = v.q.spectrogram.layers.len();
            for l in 0..n {
                let name = if l == 0 {
                    if n == 1 {
                        "Mono".to_string()
                    } else {
                        "Mix".to_string()
                    }
                } else {
                    format!("Ch {l}")
                };
                if ui.selectable_label(v.layer == l, name).clicked() {
                    v.layer = l;
                }
            }
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
        painter.rect_filled(area, CornerRadius::same(4), BG);
        painter.image(
            tex.id(),
            area,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        let font = FontId::proportional(11.0);
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
