//! User settings (a small `key=value` file) and the ffmpeg lookup result.

use std::path::{Path, PathBuf};
use truelossless_core::ffmpeg::FfmpegError;
use truelossless_core::{locate_ffmpeg, DecoderMode, Toolchain};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// ffmpeg file or folder picked by the user. `None` means "search".
    pub ffmpeg_path: Option<PathBuf>,
    pub mode: DecoderMode,
}

/// Outcome of looking for ffmpeg.
#[derive(Default)]
pub enum FfmpegState {
    Found(Toolchain),
    #[default]
    Missing,
    /// A chosen or discovered ffmpeg exists but does not work.
    Broken(String),
}

impl FfmpegState {
    pub fn toolchain(&self) -> Option<Toolchain> {
        match self {
            FfmpegState::Found(t) => Some(t.clone()),
            _ => None,
        }
    }
}

fn mode_key(m: DecoderMode) -> &'static str {
    match m {
        DecoderMode::Auto => "auto",
        DecoderMode::Native => "native",
        DecoderMode::Ffmpeg => "ffmpeg",
    }
}

fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "ffmpeg" if !v.trim().is_empty() => s.ffmpeg_path = Some(PathBuf::from(v.trim())),
            "decoder" => {
                s.mode = match v.trim() {
                    "native" => DecoderMode::Native,
                    "ffmpeg" => DecoderMode::Ffmpeg,
                    _ => DecoderMode::Auto,
                }
            }
            _ => {}
        }
    }
    s
}

fn render(s: &Settings) -> String {
    let mut out = format!("decoder={}\n", mode_key(s.mode));
    if let Some(p) = &s.ffmpeg_path {
        out.push_str(&format!("ffmpeg={}\n", p.display()));
    }
    out
}

/// Where finished checks are cached between runs.
pub fn cache_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("scan-cache.bin"))
}

fn config_dir() -> Option<PathBuf> {
    let var = |n: &str| std::env::var_os(n).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("APPDATA")?
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library").join("Application Support")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))?
    };
    Some(base.join("truelossless"))
}

fn config_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.txt"))
}

impl Settings {
    /// Missing or unreadable settings give the defaults.
    pub fn load() -> Self {
        config_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| parse(&t))
            .unwrap_or_default()
    }

    /// Best effort: a failure to save must never disturb the app.
    pub fn save(&self) {
        let Some(file) = config_file() else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, render(self));
    }

    /// Look for ffmpeg using the chosen path, then the standard places.
    pub fn resolve(&self) -> FfmpegState {
        resolve_path(self.ffmpeg_path.as_deref())
    }
}

fn resolve_path(chosen: Option<&Path>) -> FfmpegState {
    match locate_ffmpeg(chosen) {
        Ok(t) => FfmpegState::Found(t),
        Err(FfmpegError::NotFound) => FfmpegState::Missing,
        Err(e) => FfmpegState::Broken(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = Settings {
            ffmpeg_path: Some(PathBuf::from("D:/Program Files/ffmpeg/bin")),
            mode: DecoderMode::Ffmpeg,
        };
        assert_eq!(parse(&render(&s)), s);
        let d = Settings::default();
        assert_eq!(parse(&render(&d)), d);
    }

    #[test]
    fn ignores_junk_and_unknown_modes() {
        let s = parse("garbage\nfoo=bar\ndecoder=warp\nffmpeg=\n");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn broken_chosen_path_is_reported() {
        let st = resolve_path(Some(Path::new("Z:/nowhere/at/all")));
        assert!(matches!(st, FfmpegState::Broken(_)));
    }
}
