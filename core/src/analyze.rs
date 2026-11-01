use crate::cutoff::{detect_cutoff, Cutoff};
use crate::decode::decode_windows;
use crate::probe::ProbeError;
use crate::spectrum::{average_spectrum, Spectrum};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// Excerpts sampled per file and their length.
pub const WINDOWS: usize = 4;
pub const WINDOW_SECS: f64 = 5.0;

#[derive(Debug, Clone)]
pub struct Analysis {
    pub spectrum: Spectrum,
    pub cutoff: Cutoff,
}

/// Decode excerpts of `path` and measure its spectral bandwidth.
pub fn analyze_file(path: &Path) -> Result<Analysis, ProbeError> {
    let decoded = decode_windows(path, WINDOWS, WINDOW_SECS)?;
    let spectrum = average_spectrum(&decoded.windows, decoded.sample_rate)
        .ok_or_else(|| ProbeError::Decode("too short or silent to analyse".into()))?;
    let cutoff = detect_cutoff(&spectrum)
        .ok_or_else(|| ProbeError::Decode("sample rate too low to analyse".into()))?;
    Ok(Analysis { spectrum, cutoff })
}

/// Analyse many files in parallel, preserving input order.
pub fn analyze_many(paths: &[PathBuf]) -> Vec<Result<Analysis, ProbeError>> {
    paths.par_iter().map(|p| analyze_file(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::testutil::{bandlimited_noise, write_wav};

    #[test]
    fn wav_file_end_to_end_finds_16k_cutoff() {
        let p = std::env::temp_dir().join("fc_e2e_16k.wav");
        // 16-bit quantisation adds a realistic noise floor.
        let sig: Vec<f32> = bandlimited_noise(44_100, 16_000.0, 12.0)
            .iter()
            .map(|s| s * 0.5)
            .collect();
        write_wav(&p, 44_100, &sig);
        let a = analyze_file(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert!((a.cutoff.bandwidth_hz - 16_000.0).abs() < 1_000.0, "{:?}", a.cutoff);
        assert!(a.cutoff.cliff_db > 25.0, "{:?}", a.cutoff);
    }

    #[test]
    fn silent_file_reports_error_not_panic() {
        let p = std::env::temp_dir().join("fc_e2e_silent.wav");
        write_wav(&p, 44_100, &vec![0.0; 44_100 * 3]);
        let r = analyze_file(&p);
        std::fs::remove_file(&p).ok();
        assert!(r.is_err());
    }
}
