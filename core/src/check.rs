//! One-call pipeline: probe, analyse, optionally verify, and judge a file.

use crate::analyze::{analyze_file, Analysis};
use crate::flac::{verify_md5, Md5Report};
use crate::probe::{probe_file, Container, FileInfo, ProbeError};
use crate::verdict::{evaluate, Evidence, Verdict};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct CheckOptions {
    /// Decode excerpts and run the spectrum / bit-depth tests.
    pub analyze: bool,
    /// Fully decode FLAC files and verify the stored MD5 (slow).
    pub verify_md5: bool,
}

impl CheckOptions {
    pub fn standard() -> Self {
        CheckOptions {
            analyze: true,
            verify_md5: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileReport {
    pub info: FileInfo,
    pub analysis: Option<Analysis>,
    pub md5: Option<Md5Report>,
    pub verdict: Verdict,
    /// Why the optional analysis could not run, if it failed.
    pub analysis_error: Option<String>,
}

pub fn check_file(path: &Path, opts: CheckOptions) -> Result<FileReport, ProbeError> {
    let info = probe_file(path)?;

    // Lossy files are judged on their codec alone; skip the expensive work.
    let analysable = info.lossless;

    let (analysis, analysis_error) = if opts.analyze && analysable {
        match analyze_file(path) {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };

    let md5 = if opts.verify_md5 && analysable && info.container == Container::Flac {
        verify_md5(path).ok()
    } else {
        None
    };

    let evidence = Evidence {
        cutoff: analysis.as_ref().map(|a| a.cutoff.clone()),
        stored_bits: analysis.as_ref().and_then(|a| a.stored_bits),
        effective_bits: analysis.as_ref().and_then(|a| a.effective_bits),
        md5,
    };
    let verdict = evaluate(&info, &evidence);

    Ok(FileReport {
        info,
        analysis,
        md5,
        verdict,
        analysis_error,
    })
}

/// Check many files in parallel, preserving input order.
pub fn check_many(
    paths: &[PathBuf],
    opts: CheckOptions,
) -> Vec<(PathBuf, Result<FileReport, ProbeError>)> {
    paths
        .par_iter()
        .map(|p| (p.clone(), check_file(p, opts)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::testflac::write_flac;
    use crate::spectrum::testutil::{bandlimited_noise, write_wav};
    use crate::verdict::Status;

    fn pcm(sample_rate: u32, lowpass_hz: f32, bits: u32, shift: u32) -> Vec<i32> {
        // Quarter scale leaves headroom: low-passed noise peaks at about 2x.
        let amp = (1i64 << (bits - shift - 1)) as f32 * 0.25;
        bandlimited_noise(sample_rate, lowpass_hz, 6.0)
            .iter()
            .map(|s| ((s * amp).round() as i32) << shift)
            .collect()
    }

    fn run(name: &str, f: impl FnOnce(&Path)) -> FileReport {
        let p = std::env::temp_dir().join(name);
        f(&p);
        let r = check_file(
            &p,
            CheckOptions {
                analyze: true,
                verify_md5: true,
            },
        );
        std::fs::remove_file(&p).ok();
        r.unwrap()
    }

    #[test]
    fn full_band_cd_flac_is_genuine_and_md5_verified() {
        let r = run("fc_chk_genuine.flac", |p| {
            write_flac(p, 44_100, 1, 16, &pcm(44_100, 21_800.0, 16, 0), false, false)
        });
        assert_eq!(r.verdict.status, Status::Genuine, "{:?}", r.verdict);
        assert!(r.verdict.reasons.iter().any(|x| x.contains("MD5 verified")));
    }

    #[test]
    fn flac_with_16k_cutoff_is_fake() {
        let r = run("fc_chk_16k.flac", |p| {
            write_flac(p, 44_100, 1, 16, &pcm(44_100, 16_000.0, 16, 0), false, false)
        });
        assert_eq!(r.verdict.status, Status::Fake, "{:?}", r.verdict);
    }

    #[test]
    fn upsampled_96k_24bit_flac_is_fake() {
        let r = run("fc_chk_up96.flac", |p| {
            write_flac(p, 96_000, 1, 24, &pcm(96_000, 22_000.0, 24, 0), false, false)
        });
        assert_eq!(r.verdict.status, Status::Fake, "{:?}", r.verdict);
        assert!(r.verdict.reasons[0].contains("upsampled"), "{:?}", r.verdict);
    }

    #[test]
    fn genuine_looking_96k_flac_with_padded_bits_is_fake() {
        let r = run("fc_chk_pad.flac", |p| {
            write_flac(p, 96_000, 1, 24, &pcm(96_000, 44_000.0, 24, 8), false, false)
        });
        assert_eq!(r.verdict.status, Status::Fake, "{:?}", r.verdict);
        assert!(
            r.verdict.reasons.iter().any(|x| x.contains("only 16-bit")),
            "{:?}",
            r.verdict
        );
    }

    #[test]
    fn genuine_96k_24bit_flac_passes() {
        let r = run("fc_chk_real96.flac", |p| {
            write_flac(p, 96_000, 1, 24, &pcm(96_000, 44_000.0, 24, 0), false, false)
        });
        assert_eq!(r.verdict.status, Status::Genuine, "{:?}", r.verdict);
    }

    #[test]
    fn damaged_flac_is_corrupt() {
        let r = run("fc_chk_corrupt.flac", |p| {
            write_flac(p, 44_100, 1, 16, &pcm(44_100, 21_800.0, 16, 0), true, false)
        });
        assert_eq!(r.verdict.status, Status::Corrupt, "{:?}", r.verdict);
    }

    #[test]
    fn wav_renamed_to_flac_is_flagged() {
        let r = run("fc_chk_wavflac.flac", |p| {
            let s: Vec<f32> = bandlimited_noise(44_100, 21_800.0, 3.0);
            write_wav(p, 44_100, &s)
        });
        assert!(r.info.extension_mismatch);
        assert!(r.verdict.status >= Status::Suspect, "{:?}", r.verdict);
    }
}
