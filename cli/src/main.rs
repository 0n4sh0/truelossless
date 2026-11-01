use clap::Parser;
use flac_checker_core::{analyze_many, find_audio_files, probe_many, Analysis, FileInfo};
use std::path::PathBuf;

/// Inspect audio files and report their real format.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Folder (scanned recursively) or a single audio file.
    path: PathBuf,

    /// Also decode excerpts and measure the spectral bandwidth (slower).
    #[arg(short, long)]
    analyze: bool,
}

fn main() {
    let args = Args::parse();
    let files = find_audio_files(&args.path);
    if files.is_empty() {
        eprintln!("No audio files found under {}", args.path.display());
        std::process::exit(1);
    }

    let probes = probe_many(&files);
    let analyses = if args.analyze {
        Some(analyze_many(&files))
    } else {
        None
    };

    print!(
        "{:<40} {:<5} {:<6} {:>9} {:>4} {:>3} {:>6}",
        "FILE", "CONT", "CODEC", "RATE", "BITS", "CH", "DUR"
    );
    if args.analyze {
        print!(" {:>8} {:>13}", "BANDW", "EDGE");
    }
    println!("  NOTES");

    let mut errors = 0;
    for (idx, (path, res)) in probes.iter().enumerate() {
        let rel = path.strip_prefix(&args.path).unwrap_or(path);
        let name = truncate(&rel.display().to_string(), 40);
        match res {
            Ok(info) => {
                print_row(&name, info);
                if let Some(a) = &analyses {
                    print_analysis(&a[idx]);
                }
                println!("  {}", notes(info));
            }
            Err(e) => {
                errors += 1;
                println!("{name:<40} ERROR: {e}");
            }
        }
    }
    println!("\n{} files, {} unreadable", files.len(), errors);
}

fn notes(i: &FileInfo) -> String {
    let mut notes = Vec::new();
    if i.extension_mismatch {
        notes.push(format!(
            "extension .{} but content is {}",
            i.extension,
            i.container.name()
        ));
    }
    if !i.lossless {
        notes.push("lossy codec".to_string());
    }
    notes.join("; ")
}

fn print_row(name: &str, i: &FileInfo) {
    print!(
        "{:<40} {:<5} {:<6} {:>9} {:>4} {:>3} {:>6}",
        name,
        i.container.name(),
        i.codec,
        i.sample_rate.map_or("-".into(), |r| format!("{r} Hz")),
        i.bits_per_sample.map_or("-".into(), |b| b.to_string()),
        i.channels.map_or("-".into(), |c| c.to_string()),
        i.duration_secs
            .map_or("-".into(), |d| format!("{}:{:02}", d as u64 / 60, d as u64 % 60)),
    );
}

fn print_analysis(a: &Result<Analysis, flac_checker_core::ProbeError>) {
    match a {
        Ok(a) => print!(
            " {:>5.1} kHz {:>6.0} dB@{:>4.1}k",
            a.cutoff.bandwidth_hz / 1000.0,
            a.cutoff.cliff_db,
            a.cutoff.cliff_hz / 1000.0
        ),
        Err(_) => print!(" {:>8} {:>13}", "n/a", "n/a"),
    }
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
