use clap::Parser;
use flac_checker_core::{check_many, find_audio_files, CheckOptions, FileReport, Status};
use std::path::PathBuf;
use std::process::ExitCode;

/// Check whether audio files are really lossless / hi-res.
///
/// Exit code: 0 = no problems, 1 = nothing to scan, 2 = at least one
/// fake, corrupt or unreadable file was found.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Folder (scanned recursively) or a single audio file.
    path: PathBuf,

    /// Only read headers; skip the spectrum and bit-depth analysis (fast).
    #[arg(short = 'p', long)]
    probe_only: bool,

    /// Fully decode FLAC files and verify the stored MD5 (slow).
    #[arg(short, long)]
    verify: bool,

    /// List every file, including genuine ones (default hides their reasons).
    #[arg(short, long)]
    all: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let files = find_audio_files(&args.path);
    if files.is_empty() {
        eprintln!("No audio files found under {}", args.path.display());
        return ExitCode::from(1);
    }

    let opts = CheckOptions {
        analyze: !args.probe_only,
        verify_md5: args.verify,
    };
    eprintln!("Scanning {} files...", files.len());
    let results = check_many(&files, opts);

    println!(
        "{:<9} {:<38} {:<5} {:>9} {:>4} {:>6} {:>8}",
        "STATUS", "FILE", "CODEC", "RATE", "BITS", "DUR", "BANDW"
    );

    let mut counts = [0usize; 6];
    let mut errors = 0usize;
    let mut bad = false;

    for (path, res) in &results {
        let rel = path.strip_prefix(&args.path).unwrap_or(path);
        let name = truncate(&rel.display().to_string(), 38);
        match res {
            Ok(r) => {
                counts[r.verdict.status as usize] += 1;
                bad |= matches!(r.verdict.status, Status::Fake | Status::Corrupt);
                print_report(&name, r);
                if args.all || r.verdict.status != Status::Genuine {
                    for reason in &r.verdict.reasons {
                        println!("          - {reason}");
                    }
                    if let Some(e) = &r.analysis_error {
                        println!("          - analysis failed: {e}");
                    }
                }
            }
            Err(e) => {
                errors += 1;
                bad = true;
                println!("{:<9} {name:<38} {e}", "ERROR");
            }
        }
    }

    println!(
        "\n{} files: {} genuine, {} suspect, {} fake, {} corrupt, {} lossy, {} unchecked, {} unreadable",
        results.len(),
        counts[Status::Genuine as usize],
        counts[Status::Suspect as usize],
        counts[Status::Fake as usize],
        counts[Status::Corrupt as usize],
        counts[Status::Lossy as usize],
        counts[Status::Unchecked as usize],
        errors
    );
    if bad {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_report(name: &str, r: &FileReport) {
    let i = &r.info;
    println!(
        "{:<9} {:<38} {:<5} {:>9} {:>4} {:>6} {:>8}",
        r.verdict.status.label(),
        name,
        i.codec,
        i.sample_rate.map_or("-".into(), |r| format!("{r} Hz")),
        i.bits_per_sample.map_or("-".into(), |b| b.to_string()),
        i.duration_secs
            .map_or("-".into(), |d| format!("{}:{:02}", d as u64 / 60, d as u64 % 60)),
        r.analysis
            .as_ref()
            .map_or("-".into(), |a| format!("{:.1}k", a.cutoff.bandwidth_hz / 1000.0)),
    );
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let tail: String = s.chars().skip(count - (max - 1)).collect();
        format!("…{tail}")
    }
}
