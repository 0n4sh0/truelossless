use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Extensions we consider candidates for analysis (lowercase, no dot).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "alac", "mp4", "wav", "aif", "aiff", "mp3", "aac", "ogg", "oga", "opus", "ac3",
    "eac3", "ec3", "dts", "dtshd", "thd", "mlp", "truehd", "wv", "ape", "tta", "dsf", "dff", "mpc",
    "wma", "mka",
];

pub fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Recursively collect audio files under `root` (or `root` itself if it is a file).
/// Results are sorted for stable output.
pub fn find_audio_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && is_audio_path(e.path()))
        .map(|e| e.into_path())
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_audio_extensions_case_insensitively() {
        assert!(is_audio_path(Path::new("a/b/Song.FLAC")));
        assert!(is_audio_path(Path::new("x.m4a")));
        assert!(!is_audio_path(Path::new("cover.jpg")));
        assert!(!is_audio_path(Path::new("noext")));
    }
}
