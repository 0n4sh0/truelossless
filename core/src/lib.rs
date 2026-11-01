//! Core library: discover audio files, probe their real format, and analyse
//! their spectrum.

pub mod analyze;
pub mod check;
pub mod csv;
pub mod cutoff;
pub mod decode;
pub mod ffmpeg;
pub mod flac;
pub mod probe;
pub mod quality;
pub mod scan;
pub mod special;
pub mod spectrum;
pub mod verdict;

pub use analyze::{analyze_file, analyze_many, Analysis};
pub use check::{check_file, check_many, CheckOptions, FileReport};
pub use csv::{csv_line, CSV_HEADER};
pub use cutoff::{detect_cutoff, Cutoff};
pub use ffmpeg::{locate as locate_ffmpeg, Toolchain};
pub use flac::{read_streaminfo, verify_md5, Md5Report, Md5Status, StreamInfo};
pub use probe::{probe_file, probe_many, Container, FileInfo, ProbeError};
pub use quality::{analyze_quality, analyze_quality_with, ChannelStats, Quality, Spectrogram};
pub use scan::find_audio_files;
pub use special::CodecKind;
pub use spectrum::{average_spectrum, Spectrum};
pub use verdict::{evaluate, Evidence, Status, Verdict};
