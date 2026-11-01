//! Average-spectrum plot drawn directly with the egui painter.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use flac_checker_core::{Cutoff, Spectrum};

const DB_MIN: f32 = -120.0;
const DB_MAX: f32 = 0.0;
const HEIGHT: f32 = 260.0;

fn freq_step(nyquist_hz: f32) -> f32 {
    if nyquist_hz > 60_000.0 {
        10_000.0
    } else if nyquist_hz > 30_000.0 {
        5_000.0
    } else {
        2_000.0
    }
}

pub fn spectrum_plot(ui: &mut egui::Ui, spec: &Spectrum, cut: &Cutoff) {
    let width = ui.available_width().max(200.0);
    let (resp, painter) = ui.allocate_painter(Vec2::new(width, HEIGHT), Sense::hover());
    let full = resp.rect;
    // Room for axis labels on the left and bottom.
    let plot = Rect::from_min_max(
        Pos2::new(full.left() + 38.0, full.top() + 6.0),
        Pos2::new(full.right() - 8.0, full.bottom() - 20.0),
    );

    let visuals = ui.visuals();
    let grid = visuals.widgets.noninteractive.bg_stroke.color;
    let text = visuals.weak_text_color();
    let line = visuals.hyperlink_color;
    let font = FontId::proportional(11.0);

    painter.rect_filled(plot, 2.0, visuals.extreme_bg_color);

    let nyq = spec.nyquist();
    let x_of = |hz: f32| plot.left() + (hz / nyq).clamp(0.0, 1.0) * plot.width();
    let y_of = |db: f32| {
        plot.bottom() - ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0) * plot.height()
    };

    // dB grid.
    let mut db = DB_MIN;
    while db <= DB_MAX {
        let y = y_of(db);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0, grid),
        );
        painter.text(
            Pos2::new(plot.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{db:.0}"),
            font.clone(),
            text,
        );
        db += 20.0;
    }

    // Frequency grid.
    let step = freq_step(nyq);
    let mut hz = 0.0;
    while hz <= nyq {
        let x = x_of(hz);
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.0, grid),
        );
        painter.text(
            Pos2::new(x, plot.bottom() + 3.0),
            Align2::CENTER_TOP,
            format!("{:.0}k", hz / 1000.0),
            font.clone(),
            text,
        );
        hz += step;
    }

    // Spectrum curve.
    let points: Vec<Pos2> = spec
        .db
        .iter()
        .enumerate()
        .map(|(i, &d)| Pos2::new(x_of(i as f32 * spec.bin_hz), y_of(d)))
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.2, line)));

    // Detected band limit.
    let band_color = Color32::from_rgb(230, 170, 40);
    let bx = x_of(cut.bandwidth_hz);
    painter.line_segment(
        [Pos2::new(bx, plot.top()), Pos2::new(bx, plot.bottom())],
        Stroke::new(1.5, band_color),
    );
    let label_left = bx > plot.center().x;
    painter.text(
        Pos2::new(if label_left { bx - 4.0 } else { bx + 4.0 }, plot.top() + 4.0),
        if label_left { Align2::RIGHT_TOP } else { Align2::LEFT_TOP },
        format!("content ends {:.1} kHz", cut.bandwidth_hz / 1000.0),
        font.clone(),
        band_color,
    );

    // Upper limit of human hearing, for orientation.
    if nyq > 20_000.0 {
        let x = x_of(20_000.0);
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.0, text.gamma_multiply(0.6)),
        );
    }

    painter.text(
        Pos2::new(plot.right(), full.bottom()),
        Align2::RIGHT_BOTTOM,
        "dB re peak  |  frequency (Hz)",
        font,
        text,
    );
}
