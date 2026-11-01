//! Core library: discover audio files and probe their real format.

pub mod probe;
pub mod scan;

pub use probe::{probe_file, probe_many, Container, FileInfo, ProbeError};
pub use scan::find_audio_files;
