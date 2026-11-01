use crate::spectrum::Spectrum;

/// Smoothing applied to the dB curve before analysis.
const SMOOTH_HZ: f32 = 150.0;
/// Width of each side of the edge detector.
const EDGE_HZ: f32 = 400.0;
/// Content must exceed the noise floor by this much to count as "present".
const PRESENCE_DB: f32 = 10.0;
/// Only look for cliffs/floor above this frequency (or 40% of Nyquist).
const SEARCH_FROM_HZ: f32 = 8_000.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Cutoff {
    /// Highest frequency with energy clearly above the noise floor.
    pub bandwidth_hz: f32,
    /// Frequency of the steepest downward edge, if any was found.
    pub cliff_hz: f32,
    /// Size of that edge in dB (large = brick-wall low-pass, typical of lossy
    /// encoders and resampling; small = natural roll-off).
    pub cliff_db: f32,
    /// Estimated noise floor (dB re peak).
    pub floor_db: f32,
}

fn moving_average(v: &[f32], width: usize) -> Vec<f32> {
    let half = width / 2;
    let mut prefix = vec![0f64; v.len() + 1];
    for (i, x) in v.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *x as f64;
    }
    (0..v.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(v.len());
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

/// Locate the band limit of a spectrum. Returns `None` if the spectrum is too
/// coarse/short to analyse.
pub fn detect_cutoff(s: &Spectrum) -> Option<Cutoff> {
    let n = s.db.len();
    let width = ((SMOOTH_HZ / s.bin_hz) as usize).max(1) | 1;
    let edge = ((EDGE_HZ / s.bin_hz) as usize).max(3);
    let from = s.bin_for_hz(SEARCH_FROM_HZ.min(s.nyquist() * 0.4));
    if n < from + 2 * edge + 2 {
        return None;
    }

    let sm = moving_average(&s.db, width);

    // Noise floor: 2nd percentile of the upper spectrum.
    let mut upper: Vec<f32> = sm[from..].to_vec();
    upper.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor_db = upper[upper.len() / 50];

    // Highest bin clearly above the floor.
    let threshold = floor_db + PRESENCE_DB;
    // If nothing rises above the floor the upper spectrum is flat: there is no
    // band limit, so report the full range rather than the search start.
    let top = (from..n)
        .rev()
        .find(|&i| sm[i] > threshold)
        .unwrap_or(n - 1);

    // Steepest edge: mean level just below minus mean level just above.
    let mut prefix = vec![0f64; n + 1];
    for (i, x) in sm.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *x as f64;
    }
    let mean = |a: usize, b: usize| (prefix[b] - prefix[a]) / (b - a) as f64;
    let (mut best_i, mut best_drop) = (from + edge, f64::MIN);
    for i in (from + edge)..=(n - edge) {
        let drop = mean(i - edge, i) - mean(i, i + edge);
        if drop > best_drop {
            best_drop = drop;
            best_i = i;
        }
    }

    Some(Cutoff {
        bandwidth_hz: top as f32 * s.bin_hz,
        cliff_hz: best_i as f32 * s.bin_hz,
        cliff_db: best_drop.max(0.0) as f32,
        floor_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::{average_spectrum, testutil::bandlimited_noise};

    fn cutoff_of(sample_rate: u32, lowpass_hz: f32) -> Cutoff {
        let sig = bandlimited_noise(sample_rate, lowpass_hz, 4.0);
        let spec = average_spectrum(&[sig], sample_rate).unwrap();
        detect_cutoff(&spec).unwrap()
    }

    #[test]
    fn detects_mp3_128k_style_cutoff() {
        let c = cutoff_of(44_100, 16_000.0);
        assert!((c.bandwidth_hz - 16_000.0).abs() < 1_000.0, "{c:?}");
        assert!((c.cliff_hz - 16_000.0).abs() < 1_000.0, "{c:?}");
        assert!(c.cliff_db > 30.0, "{c:?}");
    }

    #[test]
    fn detects_upsampled_hires_cutoff() {
        // A "96 kHz" file whose content stops at the old 44.1 kHz Nyquist.
        let c = cutoff_of(96_000, 22_050.0);
        assert!((c.bandwidth_hz - 22_050.0).abs() < 1_500.0, "{c:?}");
        assert!(c.cliff_db > 30.0, "{c:?}");
    }

    #[test]
    fn full_band_signal_reaches_nyquist() {
        let c = cutoff_of(44_100, 22_000.0);
        assert!(c.bandwidth_hz > 20_500.0, "{c:?}");
    }
}
