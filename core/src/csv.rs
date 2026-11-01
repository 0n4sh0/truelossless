//! CSV export of check results.

use crate::check::FileReport;
use crate::flac::Md5Status;
use std::path::Path;

pub const CSV_HEADER: &str = "path,status,reasons,container,codec,lossless,sample_rate_hz,\
bits_declared,bits_used,channels,duration_secs,size_bytes,content_ends_khz,edge_db,md5";

/// Quote a field for CSV. Fields that start with a spreadsheet formula
/// character get a leading apostrophe so Excel/Sheets never run a crafted
/// file name as a formula.
fn field(s: &str) -> String {
    let mut text = s.to_string();
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        text.insert(0, '\'');
    }
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

/// One CSV line (no trailing newline) for a checked file or a read error.
pub fn csv_line(path: &Path, result: Result<&FileReport, &str>) -> String {
    let p = field(&path.display().to_string());
    let r = match result {
        Ok(r) => r,
        Err(e) => return format!("{p},ERROR,{},,,,,,,,,,,,", field(e)),
    };
    let i = &r.info;
    let a = r.analysis.as_ref();
    let md5 = r.md5.map(|m| match m.status {
        Md5Status::Match => "verified",
        Md5Status::Mismatch => "mismatch",
        Md5Status::Unavailable => "not_stored",
    });
    [
        p,
        r.verdict.status.label().to_string(),
        field(&r.verdict.reasons.join("; ")),
        i.container.name().to_string(),
        field(&i.codec),
        i.lossless.to_string(),
        opt(i.sample_rate),
        opt(i.bits_per_sample),
        opt(a.and_then(|a| a.effective_bits)),
        opt(i.channels),
        opt(i.duration_secs.map(|d| format!("{d:.1}"))),
        i.size_bytes.to_string(),
        opt(a.map(|a| format!("{:.2}", a.cutoff.bandwidth_hz / 1000.0))),
        opt(a.map(|a| format!("{:.0}", a.cutoff.cliff_db))),
        md5.unwrap_or("").to_string(),
    ]
    .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unquoted() {
        assert_eq!(field("song.flac"), "song.flac");
    }

    #[test]
    fn commas_quotes_and_newlines_are_quoted() {
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("a\nb"), "\"a\nb\"");
    }

    #[test]
    fn formula_prefixes_are_neutralised() {
        assert_eq!(field("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(field("@cmd"), "'@cmd");
        assert_eq!(field("-1+1"), "'-1+1");
    }

    #[test]
    fn header_and_error_rows_have_matching_column_counts() {
        let cols = CSV_HEADER.split(',').count();
        let row = csv_line(Path::new("bad.flac"), Err("broken"));
        assert_eq!(row.split(',').count(), cols);
        assert!(row.contains("ERROR"));
    }

    #[test]
    fn report_row_has_matching_column_count() {
        use crate::check::{check_file, CheckOptions};
        use crate::spectrum::testutil::{bandlimited_noise, write_wav};
        let p = std::env::temp_dir().join("fc_csv.wav");
        write_wav(&p, 44_100, &bandlimited_noise(44_100, 21_000.0, 6.0));
        let r = check_file(&p, CheckOptions::standard()).unwrap();
        std::fs::remove_file(&p).ok();
        let row = csv_line(&p, Ok(&r));
        // The reasons field is quoted but contains no commas here.
        assert_eq!(row.split(',').count(), CSV_HEADER.split(',').count(), "{row}");
    }
}
