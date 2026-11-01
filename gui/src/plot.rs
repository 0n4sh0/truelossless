//! Average-spectrum plot drawn directly with the egui painter.

use crate::style::{alpha, ACCENT, BG, BORDER, TEXT_DIM};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use truelossless_core::{Cutoff, Spectrum};

const DB_MIN: f32 = -120.0;
const DB_MAX: f32 = 0.0;
const HEIGHT: f32 = 270.0;
const AMBER: Color32 = Color32::from_rgb(245, 180, 50);

fn freq_step(nyquist_hz: f32) -> f32 {
    if nyquist_hz > 60_000.0 {
        10_000.0
    } else if nyquist_hz > 30_000.0 {
        5_000.0
    } else {
        2_000.0
    }
}

/// Reduce the spectrum to one point per pixel column, keeping each column's
/// peak so narrow features are not lost.
fn peak_hold(spec: &Spectrum, columns: usize) -> Vec<f32> {
    let n = spec.db.len();
    (0..columns)
        .map(|c| {
            let lo = c * n / columns;
            let hi = (((c + 1) * n) / columns).max(lo + 1).min(n);
            spec.db[lo..hi].iter().cloned().fold(f32::MIN, f32::max)
        })
        .collect()
}

pub fn spectrum_plot(ui: &mut egui::Ui, spec: &Spectrum, cut: &Cutoff) {
    let width = ui.available_width().max(240.0);
    let (resp, painter) = ui.allocate_painter(Vec2::new(width, HEIGHT), Sense::hover());
    let full = resp.rect;
    let plot = Rect::from_min_max(
        Pos2::new(full.left() + 40.0, full.top() + 8.0),
        Pos2::new(full.right() - 10.0, full.bottom() - 24.0),
    );
    let font = FontId::proportional(11.0);

    painter.rect_filled(plot, CornerRadius::same(6), BG);

    let nyq = spec.nyquist();
    let x_of = |hz: f32| plot.left() + (hz / nyq).clamp(0.0, 1.0) * plot.width();
    let y_of = |db: f32| {
        plot.bottom() - ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0) * plot.height()
    };
    let grid = Stroke::new(1.0, alpha(BORDER, 150));

    let mut db = DB_MIN;
    while db <= DB_MAX {
        let y = y_of(db);
        painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], grid);
        painter.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            format!("{db:.0}"),
            font.clone(),
            TEXT_DIM,
        );
        db += 20.0;
    }

    let step = freq_step(nyq);
    let mut hz = 0.0;
    while hz <= nyq {
        let x = x_of(hz);
        painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], grid);
        painter.text(
            Pos2::new(x, plot.bottom() + 5.0),
            Align2::CENTER_TOP,
            format!("{:.0}k", hz / 1000.0),
            font.clone(),
            TEXT_DIM,
        );
        hz += step;
    }

    // Spectrum: filled area fading downward, plus the outline.
    let cols = (plot.width() as usize).clamp(2, spec.db.len());
    let levels = peak_hold(spec, cols);
    let pts: Vec<Pos2> = levels
        .iter()
        .enumerate()
        .map(|(c, &d)| {
            let x = plot.left() + c as f32 / (cols - 1) as f32 * plot.width();
            Pos2::new(x, y_of(d))
        })
        .collect();

    let top = alpha(ACCENT, 85);
    let bottom = alpha(ACCENT, 0);
    let mut mesh = egui::Mesh::default();
    for w in pts.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], top);
        mesh.colored_vertex(w[1], top);
        mesh.colored_vertex(Pos2::new(w[1].x, plot.bottom()), bottom);
        mesh.colored_vertex(Pos2::new(w[0].x, plot.bottom()), bottom);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, ACCENT)));

    // 20 kHz: the usual upper limit of hearing, for orientation.
    if nyq > 20_000.0 {
        let x = x_of(20_000.0);
        painter.add(egui::Shape::dashed_line(
            &[Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.0, alpha(TEXT_DIM, 150)),
            4.0,
            4.0,
        ));
    }

    // Detected band limit with a label chip.
    let bx = x_of(cut.bandwidth_hz);
    painter.line_segment(
        [Pos2::new(bx, plot.top()), Pos2::new(bx, plot.bottom())],
        Stroke::new(1.5, AMBER),
    );
    let label = format!("content ends {:.1} kHz", cut.bandwidth_hz / 1000.0);
    let galley = painter.layout_no_wrap(label, font.clone(), AMBER);
    let on_left = bx > plot.center().x;
    let lx = if on_left { bx - 8.0 - galley.size().x - 12.0 } else { bx + 8.0 };
    let chip = Rect::from_min_size(
        Pos2::new(lx, plot.top() + 8.0),
        Vec2::new(galley.size().x + 12.0, galley.size().y + 6.0),
    );
    painter.rect_filled(chip, CornerRadius::same(5), alpha(BG, 230));
    painter.rect_stroke(chip, CornerRadius::same(5), Stroke::new(1.0, alpha(AMBER, 160)), egui::StrokeKind::Inside);
    painter.galley(Pos2::new(chip.left() + 6.0, chip.top() + 3.0), galley, AMBER);
}
