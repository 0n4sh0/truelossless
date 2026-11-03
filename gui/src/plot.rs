//! Average-spectrum plot drawn directly with the egui painter.

use crate::style::{alpha, ACCENT, AMBER, BORDER, HIGH, LOWEST, TEXT_DIM};
use eframe::egui::{self, Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use truelossless_core::{Cutoff, Spectrum};

const DB_MIN: f32 = -120.0;
const DB_MAX: f32 = 0.0;
const HEIGHT: f32 = 270.0;

fn freq_step(nyquist_hz: f32) -> f32 {
    if nyquist_hz > 60_000.0 {
        10_000.0
    } else if nyquist_hz > 30_000.0 {
        5_000.0
    } else {
        2_500.0
    }
}

/// Points along the curve, one per `POINT_PX` pixels.
const POINT_PX: f32 = 6.0;

/// Reduce the spectrum to `points` values by averaging the power in each
/// slice, which gives a smooth curve without hiding the overall shape.
fn smoothed(spec: &Spectrum, points: usize) -> Vec<f32> {
    let n = spec.db.len();
    (0..points)
        .map(|c| {
            let lo = c * n / points;
            let hi = (((c + 1) * n) / points).max(lo + 1).min(n);
            let s = &spec.db[lo..hi];
            let p = s.iter().map(|d| 10f32.powf(d / 10.0)).sum::<f32>() / s.len() as f32;
            10.0 * p.max(1e-30).log10()
        })
        .collect()
}

/// A vertical dashed marker.
fn marker(painter: &egui::Painter, x: f32, plot: Rect, color: egui::Color32) {
    painter.add(egui::Shape::dashed_line(
        &[Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
        Stroke::new(1.0, color),
        4.0,
        3.0,
    ));
}

/// A boxed label beside a marker, on whichever side has room.
fn tag(painter: &egui::Painter, x: f32, y: f32, plot: Rect, text: &str, color: egui::Color32) {
    let font = FontId::new(11.5, egui::FontFamily::Name("mono-bold".into()));
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size() + Vec2::new(12.0, 6.0);
    let left = if x + 6.0 + size.x > plot.right() {
        x - 6.0 - size.x
    } else {
        x + 6.0
    };
    let r = Rect::from_min_size(Pos2::new(left, y), size);
    painter.rect_filled(r, CornerRadius::same(2), alpha(HIGH, 235));
    painter.galley(r.min + Vec2::new(6.0, 3.0), galley, color);
}

pub fn spectrum_plot(ui: &mut egui::Ui, spec: &Spectrum, cut: &Cutoff) {
    let width = ui.available_width().max(240.0);
    let (resp, painter) = ui.allocate_painter(Vec2::new(width, HEIGHT), Sense::hover());
    let full = resp.rect;
    let plot = Rect::from_min_max(full.min, Pos2::new(full.right(), full.bottom() - 22.0));
    let font = FontId::monospace(11.5);

    painter.rect_filled(plot, CornerRadius::same(4), LOWEST);

    let nyq = spec.nyquist();
    let x_of = |hz: f32| plot.left() + (hz / nyq).clamp(0.0, 1.0) * plot.width();
    let y_of = |db: f32| {
        plot.bottom() - ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0) * plot.height()
    };
    let grid = Stroke::new(1.0, alpha(BORDER, 110));

    // Level grid, labelled inside the plot.
    let mut db = DB_MIN;
    while db <= DB_MAX {
        let y = y_of(db);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            grid,
        );
        let at = if db >= DB_MAX { y + 3.0 } else { y - 2.0 };
        let align = if db >= DB_MAX {
            Align2::LEFT_TOP
        } else {
            Align2::LEFT_BOTTOM
        };
        if db > DB_MIN {
            painter.text(
                Pos2::new(plot.left() + 6.0, at),
                align,
                format!("{db:.0} dB"),
                font.clone(),
                alpha(TEXT_DIM, 120),
            );
        }
        db += 20.0;
    }

    // Spectrum: filled area fading downward, plus the outline.
    let points = ((plot.width() / POINT_PX) as usize).clamp(2, spec.db.len());
    let levels = smoothed(spec, points);
    let pts: Vec<Pos2> = levels
        .iter()
        .enumerate()
        .map(|(c, &d)| {
            let x = plot.left() + c as f32 / (points - 1) as f32 * plot.width();
            Pos2::new(x, y_of(d))
        })
        .collect();

    let top = alpha(ACCENT, 60);
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

    // Markers: where content ends, and the Nyquist limit.
    let bx = x_of(cut.bandwidth_hz);
    marker(&painter, bx, plot, AMBER);
    tag(
        &painter,
        bx,
        plot.top() + 8.0,
        plot,
        &format!("content ends {:.1} kHz", cut.bandwidth_hz / 1000.0),
        AMBER,
    );
    let nx = plot.right() - 1.0;
    marker(&painter, nx, plot, alpha(TEXT_DIM, 150));
    tag(
        &painter,
        nx,
        plot.bottom() - 30.0,
        plot,
        &format!("Nyquist limit {} kHz", khz(nyq)),
        TEXT_DIM,
    );

    // Frequency axis, with the cut marked in place of any tick it covers.
    let cut_label = format!("{}k Cut", khz(cut.bandwidth_hz));
    let cut_w = cut_label.len() as f32 * 7.0;
    let label_y = plot.bottom() + 5.0;
    let step = freq_step(nyq);
    let mut hz = 0.0;
    while hz < nyq - step * 0.4 {
        let x = x_of(hz);
        if (x - bx).abs() > cut_w * 0.5 + 34.0 {
            let align = if hz == 0.0 {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            };
            let text = if hz == 0.0 {
                "0 Hz".to_owned()
            } else {
                format!("{:.1} kHz", hz / 1000.0)
            };
            painter.text(Pos2::new(x, label_y), align, text, font.clone(), TEXT_DIM);
        }
        hz += step;
    }
    painter.text(
        Pos2::new(plot.right(), label_y),
        Align2::RIGHT_TOP,
        format!("{} kHz", khz(nyq)),
        font.clone(),
        TEXT_DIM,
    );
    painter.text(
        Pos2::new(bx, label_y),
        Align2::CENTER_TOP,
        cut_label,
        font,
        AMBER,
    );
}

/// Kilohertz with up to two decimals and no trailing zeros.
pub fn khz(hz: f32) -> String {
    let s = format!("{:.2}", hz / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}
