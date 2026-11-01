use eframe::egui::Color32;
use std::path::PathBuf;
use truelossless_core::{FileReport, Status};

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
        Status::Genuine => Color32::from_rgb(70, 190, 110),
        Status::Lossy => Color32::from_rgb(120, 150, 200),
        Status::Suspect => Color32::from_rgb(230, 170, 40),
        Status::Corrupt => Color32::from_rgb(235, 120, 50),
        Status::Fake => Color32::from_rgb(230, 70, 70),
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
}
