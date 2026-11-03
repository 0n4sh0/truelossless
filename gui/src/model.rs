use eframe::egui::Color32;
use std::path::PathBuf;
use truelossless_core::{FileInfo, FileReport, Status};

/// One row in the file list. `result` is `None` until the file is checked.
pub struct Slot {
    pub path: PathBuf,
    /// Path shown in the list, relative to the scanned folder.
    pub name: String,
    pub result: Option<Result<FileReport, String>>,
}

/// What a row currently is, for filtering and counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Error,
    Done(Status),
}

impl Slot {
    pub fn outcome(&self) -> Outcome {
        match &self.result {
            None => Outcome::Pending,
            Some(Err(_)) => Outcome::Error,
            Some(Ok(r)) => Outcome::Done(r.verdict.status),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Problems,
    Genuine,
    Lossy,
}

impl Filter {
    pub const CHOICES: [Filter; 4] = [
        Filter::All,
        Filter::Problems,
        Filter::Genuine,
        Filter::Lossy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All files",
            Filter::Problems => "Problems (fake / suspect / corrupt)",
            Filter::Genuine => "Genuine",
            Filter::Lossy => "Lossy",
        }
    }

    pub fn matches(self, o: Outcome) -> bool {
        match (self, o) {
            (Filter::All, _) => true,
            (Filter::Problems, Outcome::Error) => true,
            (Filter::Problems, Outcome::Done(s)) => s >= Status::Suspect,
            (Filter::Genuine, Outcome::Done(s)) => s == Status::Genuine,
            (Filter::Lossy, Outcome::Done(s)) => s == Status::Lossy,
            _ => false,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub pending: usize,
    pub errors: usize,
    pub genuine: usize,
    pub lossy: usize,
    pub suspect: usize,
    pub corrupt: usize,
    pub fake: usize,
    pub unchecked: usize,
}

impl Counts {
    pub fn of(slots: &[Slot]) -> Counts {
        let mut c = Counts::default();
        for s in slots {
            match s.outcome() {
                Outcome::Pending => c.pending += 1,
                Outcome::Error => c.errors += 1,
                Outcome::Done(Status::Genuine) => c.genuine += 1,
                Outcome::Done(Status::Lossy) => c.lossy += 1,
                Outcome::Done(Status::Suspect) => c.suspect += 1,
                Outcome::Done(Status::Corrupt) => c.corrupt += 1,
                Outcome::Done(Status::Fake) => c.fake += 1,
                Outcome::Done(Status::Unchecked) => c.unchecked += 1,
            }
        }
        c
    }
}

pub fn status_color(s: Status) -> Color32 {
    match s {
        Status::Genuine => Color32::from_rgb(78, 222, 163),
        Status::Lossy => Color32::from_rgb(134, 147, 152),
        Status::Suspect => Color32::from_rgb(255, 185, 95),
        Status::Corrupt => Color32::from_rgb(235, 120, 50),
        Status::Fake => Color32::from_rgb(255, 120, 110),
        Status::Unchecked => Color32::GRAY,
    }
}

pub fn outcome_color(o: Outcome) -> Color32 {
    match o {
        Outcome::Pending => Color32::DARK_GRAY,
        Outcome::Error => Color32::from_rgb(235, 120, 50),
        Outcome::Done(s) => status_color(s),
    }
}

/// Text for the status chip (text as well as colour, so it does not rely on
/// colour alone).
pub fn outcome_label(o: Outcome) -> &'static str {
    match o {
        Outcome::Pending => "CHECKING",
        Outcome::Error => "ERROR",
        Outcome::Done(Status::Genuine) => "GENUINE",
        Outcome::Done(Status::Lossy) => "LOSSY",
        Outcome::Done(Status::Suspect) => "SUSPECT",
        Outcome::Done(Status::Corrupt) => "CORRUPT",
        Outcome::Done(Status::Fake) => "FAKE",
        Outcome::Done(Status::Unchecked) => "UNCHECKED",
    }
}

/// "44.1 kHz" / "96 kHz".
pub fn fmt_rate(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", hz as f32 / 1000.0)
    }
}

/// "24-bit", "n/a" for lossy codecs (they have no fixed bit depth), or "-".
pub fn bit_depth_text(info: &FileInfo) -> String {
    match info.bits_per_sample {
        Some(b) => format!("{b}-bit"),
        None if !info.lossless => "n/a".into(),
        None => "-".into(),
    }
}

/// Second line of a file-list row, e.g.
/// "M4A · bit n/a · 48 kHz · 6 ch". The codec and the lossless or lossy
/// label are left out: the status chip and the details panel carry them.
pub fn row_summary(info: &FileInfo) -> String {
    let ext = if info.extension.is_empty() {
        info.container.name().to_string()
    } else {
        info.extension.to_uppercase()
    };
    let bits = match info.bits_per_sample {
        Some(b) => format!("{b}-bit"),
        None if !info.lossless => "bit n/a".into(),
        None => "-".into(),
    };
    let mut parts = vec![ext, bits];
    if let Some(sr) = info.sample_rate {
        parts.push(fmt_rate(sr));
    }
    if let Some(ch) = info.channels.filter(|&c| c > 2) {
        parts.push(format!("{ch} ch"));
    }
    parts.join(" \u{b7} ")
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_filter_keeps_suspect_corrupt_fake_and_errors() {
        let f = Filter::Problems;
        assert!(f.matches(Outcome::Done(Status::Fake)));
        assert!(f.matches(Outcome::Done(Status::Corrupt)));
        assert!(f.matches(Outcome::Done(Status::Suspect)));
        assert!(f.matches(Outcome::Error));
        assert!(!f.matches(Outcome::Done(Status::Genuine)));
        assert!(!f.matches(Outcome::Done(Status::Lossy)));
        assert!(!f.matches(Outcome::Pending));
    }

    #[test]
    fn specific_filters_match_only_their_status() {
        assert!(Filter::Genuine.matches(Outcome::Done(Status::Genuine)));
        assert!(!Filter::Genuine.matches(Outcome::Done(Status::Fake)));
        assert!(Filter::Lossy.matches(Outcome::Done(Status::Lossy)));
        assert!(!Filter::Lossy.matches(Outcome::Pending));
    }

    #[test]
    fn all_filter_shows_pending_rows_too() {
        assert!(Filter::All.matches(Outcome::Pending));
    }

    #[test]
    fn formats_rates_and_durations() {
        assert_eq!(fmt_rate(44_100), "44.1 kHz");
        assert_eq!(fmt_rate(96_000), "96 kHz");
        assert_eq!(fmt_duration(65.0), "1:05");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
    }

    #[test]
    fn counts_pending_and_errors() {
        let slots = vec![
            Slot {
                path: "a".into(),
                name: "a".into(),
                result: None,
            },
            Slot {
                path: "b".into(),
                name: "b".into(),
                result: Some(Err("x".into())),
            },
        ];
        let c = Counts::of(&slots);
        assert_eq!((c.pending, c.errors), (1, 1));
    }

    fn info(lossless: bool, bits: Option<u32>, ch: usize) -> FileInfo {
        FileInfo {
            path: std::path::PathBuf::from("x.m4a"),
            size_bytes: 1,
            extension: "m4a".into(),
            container: truelossless_core::Container::Mp4,
            codec: "E-AC-3".into(),
            lossless,
            sample_rate: Some(48_000),
            bits_per_sample: bits,
            channels: Some(ch),
            duration_secs: None,
            extension_mismatch: false,
            header_only: None,
            note: None,
        }
    }

    #[test]
    fn atmos_row_says_bit_depth_is_not_applicable() {
        let i = info(false, None, 6);
        assert_eq!(bit_depth_text(&i), "n/a");
        assert_eq!(
            row_summary(&i),
            "M4A \u{b7} bit n/a \u{b7} 48 kHz \u{b7} 6 ch"
        );
    }

    #[test]
    fn lossless_row_shows_bits_and_hides_stereo_channels() {
        let i = info(true, Some(24), 2);
        assert_eq!(bit_depth_text(&i), "24-bit");
        assert_eq!(row_summary(&i), "M4A \u{b7} 24-bit \u{b7} 48 kHz");
        assert_eq!(bit_depth_text(&info(true, None, 2)), "-");
    }
}
