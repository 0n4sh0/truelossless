//! Turns raw measurements into a human-readable verdict.
//!
//! The rules are heuristics. "Fake" is only used when the evidence is strong
//! (a sharp band limit well below what a genuine file would show); softer
//! evidence gives "Suspect".

use crate::analyze::Analysis;
use crate::cutoff::Cutoff;
use crate::flac::{Md5Report, Md5Status};
use crate::probe::FileInfo;
use crate::special::CodecKind;

/// Band limits at or below this (Hz) with a sharp edge indicate a lossy source.
const LOSSY_FAKE_BELOW_HZ: f32 = 17_500.0;
/// Sharp edges below this (Hz) are suspicious but may be a limited master.
const LOSSY_SUSPECT_BELOW_HZ: f32 = 19_500.0;
/// Edge size (dB) that counts as a brick-wall filter rather than roll-off.
pub const SHARP_EDGE_DB: f32 = 25.0;
/// Hi-res files whose content ends below this are 44.1/48 kHz material.
const UPSAMPLED_BELOW_HZ: f32 = 24_500.0;
/// Spectral tests need at least this sample rate to be meaningful.
const MIN_SPECTRAL_RATE: u32 = 40_000;
/// First sample rate considered hi-res for the upsample test.
const HIRES_RATE: u32 = 88_000;
/// Rates from which a 96 kHz -> 192 kHz upsample is plausible.
const ULTRA_RATE: u32 = 170_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Nothing could be checked (no analysis available).
    Unchecked,
    /// Passed every check that ran.
    Genuine,
    /// A real lossy file (MP3/AAC/...) that never claimed to be lossless.
    Lossy,
    /// Some evidence of a transcode/upsample, but not conclusive.
    Suspect,
    /// Audio data is damaged (MD5 mismatch / decode errors).
    Corrupt,
    /// Strong evidence the file is not what it claims to be.
    Fake,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Unchecked => "UNCHECKED",
            Status::Genuine => "GENUINE",
            Status::Lossy => "LOSSY",
            Status::Suspect => "SUSPECT",
            Status::Corrupt => "CORRUPT",
            Status::Fake => "FAKE",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub status: Status,
    /// Findings, most important first.
    pub reasons: Vec<String>,
}

/// Measurements the verdict is based on. Any part may be missing.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    pub cutoff: Option<Cutoff>,
    pub stored_bits: Option<u32>,
    pub effective_bits: Option<u32>,
    pub md5: Option<Md5Report>,
}

impl Evidence {
    pub fn from_analysis(a: &Analysis, md5: Option<Md5Report>) -> Evidence {
        Evidence {
            cutoff: Some(a.cutoff.clone()),
            stored_bits: a.stored_bits,
            effective_bits: a.effective_bits,
            md5,
        }
    }
}

fn khz(hz: f32) -> String {
    format!("{:.1} kHz", hz / 1000.0)
}

/// Rough guess at the lossy encoder that produced a given band limit.
fn lossy_hint(bandwidth_hz: f32) -> &'static str {
    if bandwidth_hz <= 16_500.0 {
        " (resembles MP3 at about 128 kbps)"
    } else if bandwidth_hz <= 18_500.0 {
        " (resembles MP3/AAC at about 160-192 kbps)"
    } else {
        " (resembles MP3/AAC at about 192-256 kbps)"
    }
}

struct Collector {
    status: Status,
    findings: Vec<String>,
    passed: Vec<String>,
}

impl Collector {
    fn flag(&mut self, status: Status, msg: String) {
        self.status = self.status.max(status);
        self.findings.push(msg);
    }
}

pub fn evaluate(info: &FileInfo, ev: &Evidence) -> Verdict {
    let mut c = Collector {
        status: Status::Unchecked,
        findings: Vec::new(),
        passed: Vec::new(),
    };

    // 1. Is the content what the extension and codec claim?
    if info.extension_mismatch {
        let claims_lossless = matches!(
            info.extension.as_str(),
            "flac" | "alac" | "wav" | "aif" | "aiff"
        );
        if !info.lossless && info.header_only != Some(CodecKind::Unknown) && claims_lossless {
            c.flag(
                Status::Fake,
                format!(
                    "lossy {} audio disguised as .{}",
                    info.codec.to_uppercase(),
                    info.extension
                ),
            );
            return finish(c);
        }
        c.flag(
            Status::Suspect,
            format!(
                "extension .{} but content is {}",
                info.extension,
                info.container.name()
            ),
        );
    }
    if let Some(kind) = info.header_only {
        let note = info
            .note
            .as_deref()
            .map(|n| format!(" - {n}"))
            .unwrap_or_default();
        let name = info.codec.as_str();
        match kind {
            CodecKind::Lossy => c.flag(
                Status::Lossy,
                format!("{name}{note}: lossy by design, no authenticity test applies"),
            ),
            CodecKind::Lossless => c.flag(
                Status::Unchecked,
                format!("{name}{note}: lossless codec, but this tool cannot decode it, so authenticity was not checked"),
            ),
            CodecKind::Unknown => c.flag(
                Status::Unchecked,
                format!("{name}{note}: codec not decoded, authenticity not checked"),
            ),
        }
        return finish(c);
    }
    if !info.lossless {
        c.flag(
            Status::Lossy,
            format!("lossy codec ({})", info.codec.to_uppercase()),
        );
        return finish(c);
    }

    // 2. Is the audio intact?
    if let Some(m) = &ev.md5 {
        match m.status {
            Md5Status::Match => c.passed.push("MD5 verified".into()),
            Md5Status::Mismatch => c.flag(
                Status::Corrupt,
                if m.decode_errors > 0 {
                    format!("{} frames failed to decode", m.decode_errors)
                } else {
                    "audio does not match the stored MD5".into()
                },
            ),
            Md5Status::Unavailable => c
                .findings
                .push("no MD5 stored, integrity not verified".into()),
        }
    }

    // 3. Does the spectrum match the claimed quality?
    if let (Some(cut), Some(sr)) = (&ev.cutoff, info.sample_rate) {
        check_bandwidth(&mut c, cut, sr);
    }

    // 4. Is the claimed bit depth real?
    if let (Some(stored), Some(eff)) = (ev.stored_bits, ev.effective_bits) {
        check_bit_depth(&mut c, stored, eff);
    }

    finish(c)
}

fn check_bandwidth(c: &mut Collector, cut: &Cutoff, sr: u32) {
    let b = cut.bandwidth_hz;
    let sharp = cut.cliff_db >= SHARP_EDGE_DB;

    if sr < MIN_SPECTRAL_RATE {
        c.findings
            .push("sample rate too low for the spectral test".into());
        return;
    }

    if sr >= HIRES_RATE {
        if b < LOSSY_SUSPECT_BELOW_HZ && sharp {
            c.flag(
                Status::Fake,
                format!(
                    "{} file but content stops at {}: lossy source upsampled{}",
                    khz(sr as f32),
                    khz(b),
                    lossy_hint(b)
                ),
            );
        } else if b < UPSAMPLED_BELOW_HZ {
            let msg = format!(
                "{} file but content stops at {}: looks upsampled from 44.1/48 kHz",
                khz(sr as f32),
                khz(b)
            );
            c.flag(if sharp { Status::Fake } else { Status::Suspect }, msg);
        } else if sr >= ULTRA_RATE && (44_000.0..49_500.0).contains(&b) && sharp {
            c.flag(
                Status::Suspect,
                format!(
                    "{} file but content stops near {}: may be upsampled from 96 kHz",
                    khz(sr as f32),
                    khz(b)
                ),
            );
        } else {
            c.passed.push(format!("content reaches {}", khz(b)));
        }
        return;
    }

    if b < LOSSY_FAKE_BELOW_HZ {
        if sharp {
            c.flag(
                Status::Fake,
                format!(
                    "content stops at {} with a sharp edge: transcoded from a lossy source{}",
                    khz(b),
                    lossy_hint(b)
                ),
            );
        } else {
            c.flag(
                Status::Suspect,
                format!(
                    "band-limited to {} without a sharp edge (old or filtered recording?)",
                    khz(b)
                ),
            );
        }
    } else if b < LOSSY_SUSPECT_BELOW_HZ && sharp {
        c.flag(
            Status::Suspect,
            format!(
                "sharp cutoff at {}: possibly a lossy source{}",
                khz(b),
                lossy_hint(b)
            ),
        );
    } else {
        c.passed.push(format!("full bandwidth to {}", khz(b)));
    }
}

fn check_bit_depth(c: &mut Collector, stored: u32, eff: u32) {
    let unused = stored.saturating_sub(eff);
    if stored == 24 && eff <= 16 {
        c.flag(
            Status::Fake,
            format!("24-bit file contains only {eff}-bit data (padded)"),
        );
    } else if unused >= 2 {
        c.flag(
            Status::Suspect,
            format!("{stored}-bit file uses only {eff} bits"),
        );
    } else {
        c.passed.push(format!("{eff}-bit data"));
    }
}

fn finish(mut c: Collector) -> Verdict {
    let mut reasons = std::mem::take(&mut c.findings);
    let status = if c.status == Status::Unchecked && !c.passed.is_empty() {
        Status::Genuine
    } else {
        c.status
    };
    if status == Status::Genuine || (reasons.is_empty() && !c.passed.is_empty()) {
        reasons.extend(c.passed);
    }
    if reasons.is_empty() {
        reasons.push("not analysed".into());
    }
    Verdict { status, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Container;
    use std::path::PathBuf;

    fn info(
        ext: &str,
        container: Container,
        codec: &str,
        lossless: bool,
        sr: u32,
        bits: u32,
    ) -> FileInfo {
        FileInfo {
            path: PathBuf::from(format!("x.{ext}")),
            size_bytes: 1,
            extension: ext.into(),
            container,
            codec: codec.into(),
            lossless,
            sample_rate: Some(sr),
            bits_per_sample: Some(bits),
            channels: Some(2),
            duration_secs: Some(100.0),
            extension_mismatch: crate::probe::Container::from_extension(ext) != container
                && Container::from_extension(ext) != Container::Unknown,
            header_only: None,
            note: None,
        }
    }

    fn flac(sr: u32, bits: u32) -> FileInfo {
        info("flac", Container::Flac, "flac", true, sr, bits)
    }

    fn cut(bandwidth_hz: f32, cliff_db: f32) -> Cutoff {
        Cutoff {
            bandwidth_hz,
            cliff_hz: bandwidth_hz,
            cliff_db,
            floor_db: -90.0,
        }
    }

    fn ev(bandwidth_hz: f32, cliff_db: f32, stored: u32, eff: u32) -> Evidence {
        Evidence {
            cutoff: Some(cut(bandwidth_hz, cliff_db)),
            stored_bits: Some(stored),
            effective_bits: Some(eff),
            md5: None,
        }
    }

    #[test]
    fn full_band_cd_flac_is_genuine() {
        let v = evaluate(&flac(44_100, 16), &ev(21_500.0, 12.0, 16, 16));
        assert_eq!(v.status, Status::Genuine, "{v:?}");
    }

    #[test]
    fn sharp_16k_cutoff_is_fake_lossless() {
        let v = evaluate(&flac(44_100, 16), &ev(16_000.0, 60.0, 16, 16));
        assert_eq!(v.status, Status::Fake);
        assert!(v.reasons[0].contains("128 kbps"), "{v:?}");
    }

    #[test]
    fn soft_low_bandwidth_is_only_suspect() {
        let v = evaluate(&flac(44_100, 16), &ev(15_000.0, 8.0, 16, 16));
        assert_eq!(v.status, Status::Suspect);
    }

    #[test]
    fn sharp_19k_cutoff_is_suspect_not_fake() {
        let v = evaluate(&flac(44_100, 16), &ev(19_000.0, 40.0, 16, 16));
        assert_eq!(v.status, Status::Suspect);
    }

    #[test]
    fn upsampled_96k_is_fake_hires() {
        let v = evaluate(&flac(96_000, 24), &ev(22_000.0, 60.0, 24, 24));
        assert_eq!(v.status, Status::Fake);
        assert!(v.reasons[0].contains("upsampled"), "{v:?}");
    }

    #[test]
    fn genuine_96k_reaching_40k_is_genuine() {
        let v = evaluate(&flac(96_000, 24), &ev(40_000.0, 15.0, 24, 24));
        assert_eq!(v.status, Status::Genuine, "{v:?}");
    }

    #[test]
    fn lossy_source_upsampled_to_hires_is_fake() {
        let v = evaluate(&flac(96_000, 24), &ev(16_000.0, 60.0, 24, 24));
        assert_eq!(v.status, Status::Fake);
        assert!(v.reasons[0].contains("lossy source"), "{v:?}");
    }

    #[test]
    fn upsampled_192k_from_96k_is_suspect() {
        let v = evaluate(&flac(192_000, 24), &ev(47_500.0, 50.0, 24, 24));
        assert_eq!(v.status, Status::Suspect);
    }

    #[test]
    fn padded_24_bit_is_fake() {
        let v = evaluate(&flac(96_000, 24), &ev(40_000.0, 10.0, 24, 16));
        assert_eq!(v.status, Status::Fake);
        assert!(v.reasons[0].contains("only 16-bit"), "{v:?}");
    }

    #[test]
    fn slightly_reduced_bit_use_is_suspect() {
        let v = evaluate(&flac(96_000, 24), &ev(40_000.0, 10.0, 24, 20));
        assert_eq!(v.status, Status::Suspect);
    }

    #[test]
    fn mp3_renamed_flac_is_fake() {
        let i = info("flac", Container::Mp3, "mp3", false, 44_100, 0);
        let v = evaluate(&i, &Evidence::default());
        assert_eq!(v.status, Status::Fake);
        assert!(v.reasons[0].contains("disguised"), "{v:?}");
    }

    #[test]
    fn wav_renamed_flac_is_suspect() {
        let i = info("flac", Container::Wav, "pcm_s16le", true, 44_100, 16);
        let v = evaluate(&i, &Evidence::default());
        assert_eq!(v.status, Status::Suspect);
    }

    #[test]
    fn honest_mp3_is_lossy() {
        let i = info("mp3", Container::Mp3, "mp3", false, 44_100, 0);
        let v = evaluate(&i, &Evidence::default());
        assert_eq!(v.status, Status::Lossy);
    }

    #[test]
    fn md5_mismatch_is_corrupt() {
        let mut e = ev(21_500.0, 10.0, 16, 16);
        e.md5 = Some(Md5Report {
            status: Md5Status::Mismatch,
            decode_errors: 0,
        });
        let v = evaluate(&flac(44_100, 16), &e);
        assert_eq!(v.status, Status::Corrupt);
    }

    #[test]
    fn fake_outranks_corrupt() {
        let mut e = ev(16_000.0, 60.0, 16, 16);
        e.md5 = Some(Md5Report {
            status: Md5Status::Mismatch,
            decode_errors: 0,
        });
        assert_eq!(evaluate(&flac(44_100, 16), &e).status, Status::Fake);
    }

    #[test]
    fn md5_match_is_reported_for_genuine_files() {
        let mut e = ev(21_500.0, 10.0, 16, 16);
        e.md5 = Some(Md5Report {
            status: Md5Status::Match,
            decode_errors: 0,
        });
        let v = evaluate(&flac(44_100, 16), &e);
        assert_eq!(v.status, Status::Genuine);
        assert!(
            v.reasons.iter().any(|r| r.contains("MD5 verified")),
            "{v:?}"
        );
    }

    #[test]
    fn no_evidence_is_unchecked() {
        let v = evaluate(&flac(44_100, 16), &Evidence::default());
        assert_eq!(v.status, Status::Unchecked);
    }

    #[test]
    fn low_sample_rate_skips_spectral_test() {
        let v = evaluate(&flac(22_050, 16), &ev(9_000.0, 60.0, 16, 16));
        assert_ne!(v.status, Status::Fake);
    }
    fn header_only(codec: &str, kind: CodecKind, ext: &str) -> FileInfo {
        let mut i = info(
            ext,
            Container::from_extension(ext),
            codec,
            kind == CodecKind::Lossless,
            48_000,
            0,
        );
        i.header_only = Some(kind);
        i.note = Some("Dolby Atmos (JOC, complexity 16)".into());
        i
    }

    #[test]
    fn undecodable_lossy_is_lossy_not_corrupt() {
        let v = evaluate(
            &header_only("E-AC-3", CodecKind::Lossy, "ec3"),
            &Evidence::default(),
        );
        assert_eq!(v.status, Status::Lossy);
        assert!(v.reasons[0].contains("Atmos"), "{v:?}");
    }

    #[test]
    fn undecodable_lossless_is_unchecked() {
        let v = evaluate(
            &header_only("TrueHD", CodecKind::Lossless, "thd"),
            &Evidence::default(),
        );
        assert_eq!(v.status, Status::Unchecked);
    }
}
