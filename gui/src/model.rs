use eframe::egui::Color32;
use flac_checker_core::{FileReport, Status};
use std::path::PathBuf;

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
    pub const CHOICES: [Filter; 4] = [Filter::All, Filter::Problems, Filter::Genuine, Filter::Lossy];

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

/// Fixed-width tag for the list (text as well as colour, so it does not rely
/// on colour alone).
pub fn outcome_tag(o: Outcome) -> &'static str {
    match o {
        Outcome::Pending => "...    ",
        Outcome::Error => "ERROR  ",
        Outcome::Done(Status::Genuine) => "GENUINE",
        Outcome::Done(Status::Lossy) => "LOSSY  ",
        Outcome::Done(Status::Suspect) => "SUSPECT",
        Outcome::Done(Status::Corrupt) => "CORRUPT",
        Outcome::Done(Status::Fake) => "FAKE   ",
        Outcome::Done(Status::Unchecked) => "UNCHKD ",
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
    fn counts_pending_and_errors() {
        let slots = vec![
            Slot { path: "a".into(), name: "a".into(), result: None },
            Slot { path: "b".into(), name: "b".into(), result: Some(Err("x".into())) },
        ];
        let c = Counts::of(&slots);
        assert_eq!((c.pending, c.errors), (1, 1));
    }
}
