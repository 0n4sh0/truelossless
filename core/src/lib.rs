//! Core library: discover audio files, probe their real format, and analyse
//! their spectrum.

pub mod cutoff;
pub mod probe;
pub mod scan;
pub mod spectrum;

pub use cutoff::{detect_cutoff, Cutoff};
pub use probe::{probe_file, probe_many, Container, FileInfo, ProbeError};
pub use scan::find_audio_files;
pub use spectrum::{average_spectrum, Spectrum};
