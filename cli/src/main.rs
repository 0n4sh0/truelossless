use clap::Parser;
use flac_checker_core::{find_audio_files, probe_many, FileInfo};
use std::path::PathBuf;

/// Inspect audio files and report their real format.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Folder (scanned recursively) or a single audio file.
    path: PathBuf,
}

fn main() {
    let args = Args::parse();
    let files = find_audio_files(&args.path);
    if files.is_empty() {
        eprintln!("No audio files found under {}", args.path.display());
        std::process::exit(1);
    }

    println!(
        "{:<44} {:<5} {:<6} {:>9} {:>4} {:>3} {:>8}  NOTES",
        "FILE", "CONT", "CODEC", "RATE", "BITS", "CH", "DUR"
    );
    let mut errors = 0;
    for (path, res) in probe_many(&files) {
        let rel = path.strip_prefix(&args.path).unwrap_or(&path);
        let name = rel.display().to_string();
        match res {
            Ok(info) => print_row(&name, &info),
            Err(e) => {
                errors += 1;
                println!("{:<44} ERROR: {e}", truncate(&name, 44));
            }
        }
    }
    println!("\n{} files, {} unreadable", files.len(), errors);
}

fn print_row(name: &str, i: &FileInfo) {
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
    println!(
        "{:<44} {:<5} {:<6} {:>9} {:>4} {:>3} {:>8}  {}",
        truncate(name, 44),
        i.container.name(),
        i.codec,
        i.sample_rate.map_or("-".into(), |r| format!("{r} Hz")),
        i.bits_per_sample.map_or("-".into(), |b| b.to_string()),
        i.channels.map_or("-".into(), |c| c.to_string()),
        i.duration_secs
            .map_or("-".into(), |d| format!("{}:{:02}", d as u64 / 60, d as u64 % 60)),
        notes.join("; ")
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
