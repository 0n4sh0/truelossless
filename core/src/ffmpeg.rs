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
}
