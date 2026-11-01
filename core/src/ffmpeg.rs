//! Locating and checking an external `ffmpeg` / `ffprobe`.
//!
//! The tool never bundles ffmpeg. It looks in a fixed list of places, in this
//! order, and never in the current working directory (a planted `ffmpeg.exe`
//! in a music folder must not run when that folder is scanned):
//!
//! 1. the path given by the user (a file or a folder),
//! 2. the `TRUELOSSLESS_FFMPEG` environment variable (same rules),
//! 3. the folder of this program, or its `ffmpeg/` subfolder,
//! 4. the directories listed in `PATH` (absolute entries only).

use crate::probe::ProbeError;
use crate::quality::{Meter, Quality};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub const ENV_VAR: &str = "TRUELOSSLESS_FFMPEG";
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    #[error(
        "ffmpeg not found (looked in the chosen path, {ENV_VAR}, next to the program, and PATH)"
    )]
    NotFound,
    #[error("{0} does not look like a working ffmpeg: {1}")]
    Broken(PathBuf, String),
}

/// A checked ffmpeg installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub ffmpeg: PathBuf,
    /// `None` when ffprobe is missing; decoding still works, details do not.
    pub ffprobe: Option<PathBuf>,
    /// e.g. "7.1".
    pub version: String,
    /// Where it was found, for display.
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Chosen,
    Environment,
    NextToProgram,
    SystemPath,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Chosen => "chosen path",
            Source::Environment => "environment variable",
            Source::NextToProgram => "next to the program",
            Source::SystemPath => "system PATH",
        }
    }
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// `path` may be the executable itself or a folder that contains it.
fn resolve_entry(path: &Path, base: &str) -> Option<PathBuf> {
    if path.is_dir() {
        let p = path.join(exe_name(base));
        return p.is_file().then_some(p);
    }
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    None
}

/// Sibling executable (`ffprobe` next to `ffmpeg`).
fn sibling(ffmpeg: &Path, base: &str) -> Option<PathBuf> {
    let p = ffmpeg.parent()?.join(exe_name(base));
    p.is_file().then_some(p)
}

/// First `name` in the absolute directories of `path_var`.
fn search_path(path_var: &std::ffi::OsStr, base: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(exe_name(base)))
        .find(|p| p.is_file())
}

/// Candidate ffmpeg executables in lookup order.
fn candidates(chosen: Option<&Path>) -> Vec<(PathBuf, Source)> {
    let mut out = Vec::new();
    if let Some(p) = chosen.and_then(|p| resolve_entry(p, "ffmpeg")) {
        out.push((p, Source::Chosen));
    }
    if let Some(p) = std::env::var_os(ENV_VAR)
        .map(PathBuf::from)
        .and_then(|p| resolve_entry(&p, "ffmpeg"))
    {
        out.push((p, Source::Environment));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        for d in [dir.clone(), dir.join("ffmpeg")] {
            if let Some(p) = resolve_entry(&d, "ffmpeg") {
                out.push((p, Source::NextToProgram));
            }
        }
    }
    if let Some(p) = std::env::var_os("PATH").and_then(|v| search_path(&v, "ffmpeg")) {
        out.push((p, Source::SystemPath));
    }
    out
}

/// Find and check ffmpeg. An explicitly chosen path that is broken is an
/// error; it is not silently replaced by another installation.
pub fn locate(chosen: Option<&Path>) -> Result<Toolchain, FfmpegError> {
    if let Some(c) = chosen {
        if let Some(p) = resolve_entry(c, "ffmpeg") {
            return build(p, Source::Chosen);
        }
        return Err(FfmpegError::Broken(
            c.to_path_buf(),
            "no ffmpeg executable there".into(),
        ));
    }
    let mut last = None;
    for (p, src) in candidates(None) {
        match build(p, src) {
            Ok(t) => return Ok(t),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or(FfmpegError::NotFound))
}

fn build(ffmpeg: PathBuf, source: Source) -> Result<Toolchain, FfmpegError> {
    let version = ffmpeg_version(&ffmpeg).map_err(|e| FfmpegError::Broken(ffmpeg.clone(), e))?;
    let ffprobe = sibling(&ffmpeg, "ffprobe").or_else(|| {
        std::env::var_os("PATH")
            .and_then(|v| search_path(&v, "ffprobe"))
            .filter(|_| source == Source::SystemPath)
    });
    Ok(Toolchain {
        ffmpeg,
        ffprobe,
        version,
        source,
    })
}

/// Run `ffmpeg -version` with a timeout and return the version number.
fn ffmpeg_version(exe: &Path) -> Result<String, String> {
    let exe = exe.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = Command::new(&exe)
            .arg("-version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let _ = tx.send(r);
    });
    let out = rx
        .recv_timeout(VERSION_TIMEOUT)
        .map_err(|_| "no answer to -version within 5 seconds".to_string())?
        .map_err(|e| e.to_string())?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "output is not from ffmpeg".to_string())
}

/// "ffmpeg version 7.1-full_build-www.gyan.dev ..." -> "7.1-full_build-www.gyan.dev".
fn parse_version(text: &str) -> Option<String> {
    let line = text.lines().next()?;
    let rest = line.strip_prefix("ffmpeg version ")?;
    let v = rest.split_whitespace().next()?;
    Some(v.to_string())
}

// ---------------------------------------------------------------------------
// Decoding through ffmpeg
// ---------------------------------------------------------------------------

/// Longest stream we will decode (hours of audio), to bound memory and time.
const MAX_HOURS: u64 = 4;
/// Bytes read from ffmpeg per call.
const READ_CHUNK: usize = 256 * 1024;

/// What ffmpeg reports about the first audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_secs: Option<f64>,
}

fn abs_path(path: &Path) -> PathBuf {
    // An absolute path never starts with '-', so it cannot be read as an option.
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Ask ffmpeg to describe `path` (its banner goes to stderr).
pub fn probe_stream(tc: &Toolchain, path: &Path) -> Result<StreamInfo, ProbeError> {
    let out = Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-protocol_whitelist",
            "file",
            "-i",
        ])
        .arg(abs_path(path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(ProbeError::Io)?;
    parse_banner(&String::from_utf8_lossy(&out.stderr))
        .ok_or_else(|| ProbeError::Decode("ffmpeg found no audio stream".into()))
}

/// Pull the rate, channel count and duration out of ffmpeg's banner text.
pub fn parse_banner(text: &str) -> Option<StreamInfo> {
    let line = text
        .lines()
        .find(|l| l.contains("Stream #") && l.contains(" Audio:"))?;
    let after = &line[line.find(" Audio:")? + 7..];
    let fields: Vec<&str> = after.split(',').map(str::trim).collect();
    let sample_rate = fields.iter().find_map(|f| {
        f.strip_suffix(" Hz")
            .and_then(|n| n.trim().parse::<u32>().ok())
    })?;
    let channels = fields.iter().find_map(|f| parse_layout(f))?;
    let duration_secs = text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("Duration:")?;
        parse_hms(rest.split(',').next()?.trim())
    });
    Some(StreamInfo {
        sample_rate,
        channels,
        duration_secs,
    })
}

/// "mono", "stereo", "5.1(side)", "7.1", "6 channels" -> channel count.
fn parse_layout(f: &str) -> Option<usize> {
    let f = f.split('(').next()?.trim();
    match f {
        "mono" => return Some(1),
        "stereo" => return Some(2),
        "quad" => return Some(4),
        "hexagonal" => return Some(6),
        "octagonal" => return Some(8),
        _ => {}
    }
    if let Some(n) = f.strip_suffix(" channels") {
        return n.trim().parse().ok().filter(|&c| c > 0);
    }
    // "5.1", "7.1.4", "2.1": sum the dot-separated parts.
    if f.chars().all(|c| c.is_ascii_digit() || c == '.') && f.contains('.') {
        let total: usize = f
            .split('.')
            .map(|p| p.parse::<usize>().ok())
            .sum::<Option<usize>>()?;
        return (total > 0).then_some(total);
    }
    None
}

/// "00:04:19.50" -> seconds.
fn parse_hms(t: &str) -> Option<f64> {
    let mut it = t.split(':');
    let h: f64 = it.next()?.parse().ok()?;
    let m: f64 = it.next()?.parse().ok()?;
    let s: f64 = it.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Move whole frames from `bytes` (little-endian f32) into the meter and keep
/// any partial frame for the next call.
fn feed(meter: &mut Meter, bytes: &mut Vec<u8>, ch: usize) {
    let frame_bytes = ch * 4;
    let usable = bytes.len() / frame_bytes * frame_bytes;
    if usable == 0 {
        return;
    }
    let (words, _) = bytes[..usable].as_chunks::<4>();
    let samples: Vec<f32> = words.iter().map(|b| f32::from_le_bytes(*b)).collect();
    meter.push(&samples, ch);
    bytes.drain(..usable);
}

/// Levels, loudness and spectrogram for a file Symphonia cannot decode.
///
/// ffmpeg decodes the first audio stream to 32-bit float PCM in its native
/// channel order. For Dolby Atmos this is the 5.1/7.1 bed only.
pub fn analyze_quality_ffmpeg(
    tc: &Toolchain,
    path: &Path,
    duration_hint: Option<f64>,
) -> Result<Quality, ProbeError> {
    let info = probe_stream(tc, path)?;
    let size = std::fs::metadata(path).map(|m| m.len()).ok();
    let n_frames = duration_hint
        .filter(|d| *d > 0.0)
        .or(info.duration_secs)
        .map(|d| (d * info.sample_rate as f64) as u64);
    let mut meter = Meter::new(info.sample_rate, 24, n_frames);
    let max_frames = info.sample_rate as u64 * 3600 * MAX_HOURS;

    let mut child = Command::new(&tc.ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-i",
        ])
        .arg(abs_path(path))
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(ProbeError::Io)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProbeError::Decode("could not read ffmpeg output".into()))?;

    let mut buf = vec![0u8; READ_CHUNK];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                feed(&mut meter, &mut pending, info.channels);
                if meter.frames() >= max_frames {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // Stop ffmpeg if it is still running (cap reached), then reap it.
    let _ = child.kill();
    let _ = child.wait();
    let mut q = meter.finish(size)?;
    q.decoder = crate::quality::Decoder::Ffmpeg;
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_line() {
        assert_eq!(
            parse_version("ffmpeg version 7.1 Copyright (c) 2000-2024\nbuilt with gcc"),
            Some("7.1".into())
        );
        assert_eq!(
            parse_version("ffmpeg version n6.0-3-gabc Copyright"),
            Some("n6.0-3-gabc".into())
        );
        assert_eq!(parse_version("not ffmpeg at all"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn resolves_file_or_folder() {
        let dir = std::env::temp_dir().join("tl_ffmpeg_resolve");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(exe_name("ffmpeg"));
        std::fs::write(&exe, b"x").unwrap();
        assert_eq!(resolve_entry(&dir, "ffmpeg"), Some(exe.clone()));
        assert_eq!(resolve_entry(&exe, "ffmpeg"), Some(exe.clone()));
        assert_eq!(resolve_entry(&dir.join("nope"), "ffmpeg"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_search_skips_relative_entries() {
        let dir = std::env::temp_dir().join("tl_ffmpeg_path");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(exe_name("ffmpeg"));
        std::fs::write(&exe, b"x").unwrap();
        // "." and "" must be ignored even if they would contain the file.
        let var =
            std::env::join_paths([PathBuf::from("."), PathBuf::from(""), dir.clone()]).unwrap();
        assert_eq!(search_path(&var, "ffmpeg"), Some(exe));
        let only_rel = std::env::join_paths([PathBuf::from("."), PathBuf::from("bin")]).unwrap();
        assert_eq!(search_path(&only_rel, "ffmpeg"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn chosen_path_that_is_missing_is_an_error() {
        let r = locate(Some(Path::new("Z:\\definitely\\not\\here")));
        assert!(matches!(r, Err(FfmpegError::Broken(..))), "{r:?}");
    }

    #[test]
    fn non_ffmpeg_binary_is_rejected() {
        // A file that exists but cannot run (or is not ffmpeg) must not pass.
        let dir = std::env::temp_dir().join("tl_ffmpeg_fake");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(exe_name("ffmpeg"));
        std::fs::write(&exe, b"not an executable").unwrap();
        let r = locate(Some(&dir));
        assert!(matches!(r, Err(FfmpegError::Broken(..))), "{r:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    const BANNER: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'x.m4a':\n  Duration: 00:04:19.04, start: 0.000000, bitrate: 768 kb/s\n  Stream #0:0[0x1](und): Audio: eac3 (ec-3 / 0x332D6365), 48000 Hz, 5.1(side), fltp, 768 kb/s (default)\n";

    #[test]
    fn parses_atmos_banner() {
        let i = parse_banner(BANNER).unwrap();
        assert_eq!(i.sample_rate, 48_000);
        assert_eq!(i.channels, 6);
        assert!((i.duration_secs.unwrap() - 259.04).abs() < 1e-6);
    }

    #[test]
    fn parses_other_layouts() {
        for (layout, n) in [
            ("mono", 1),
            ("stereo", 2),
            ("7.1", 8),
            ("5.1", 6),
            ("7.1.4", 12),
            ("2.1", 3),
            ("quad", 4),
            ("10 channels", 10),
        ] {
            assert_eq!(parse_layout(layout), Some(n), "{layout}");
        }
        assert_eq!(parse_layout("fltp"), None);
        assert_eq!(parse_layout("0 channels"), None);
    }

    #[test]
    fn banner_without_audio_is_none() {
        assert!(parse_banner("Stream #0:0: Video: h264, 1920x1080\n").is_none());
        assert!(parse_banner("").is_none());
    }

    #[test]
    fn feed_keeps_partial_frames() {
        let mut m = Meter::new(48_000, 24, None);
        // Two stereo frames, delivered as 10 bytes then the remaining 6.
        let vals = [0.5f32, -0.5, 0.25, -0.25];
        let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut pending = bytes[..10].to_vec();
        feed(&mut m, &mut pending, 2);
        assert_eq!(m.frames(), 1);
        assert_eq!(pending.len(), 2);
        pending.extend_from_slice(&bytes[10..]);
        feed(&mut m, &mut pending, 2);
        assert_eq!(m.frames(), 2);
        assert!(pending.is_empty());
    }
}
