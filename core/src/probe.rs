use crate::special::{self, CodecKind};
use rayon::prelude::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use symphonia::core::codecs::{CODEC_TYPE_ALAC, CODEC_TYPE_FLAC};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or corrupt audio: {0}")]
    Decode(String),
    #[error("no audio track found")]
    NoTrack,
}

/// Container format determined from file content (not the extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Flac,
    Mp4,
    Wav,
    Aiff,
    Mp3,
    Aac,
    Ogg,
    /// Dolby AC-3 / E-AC-3 elementary stream.
    Ac3,
    Dts,
    /// Dolby TrueHD / MLP.
    Mlp,
    WavPack,
    Ape,
    Tta,
    /// DSD (DSF or DFF).
    Dsd,
    Musepack,
    Asf,
    Matroska,
    Unknown,
}

impl Container {
    pub fn name(self) -> &'static str {
        match self {
            Container::Flac => "FLAC",
            Container::Mp4 => "MP4",
            Container::Wav => "WAV",
            Container::Aiff => "AIFF",
            Container::Mp3 => "MP3",
            Container::Aac => "AAC",
            Container::Ogg => "OGG",
            Container::Ac3 => "AC-3",
            Container::Dts => "DTS",
            Container::Mlp => "MLP",
            Container::WavPack => "WavPack",
            Container::Ape => "APE",
            Container::Tta => "TTA",
            Container::Dsd => "DSD",
            Container::Musepack => "Musepack",
            Container::Asf => "ASF",
            Container::Matroska => "Matroska",
            Container::Unknown => "?",
        }
    }

    /// Container a file *should* have given its extension.
    pub fn from_extension(ext: &str) -> Container {
        match ext.to_ascii_lowercase().as_str() {
            "flac" => Container::Flac,
            "m4a" | "alac" | "mp4" => Container::Mp4,
            "wav" => Container::Wav,
            "aif" | "aiff" => Container::Aiff,
            "mp3" => Container::Mp3,
            "aac" => Container::Aac,
            "ogg" | "oga" | "opus" => Container::Ogg,
            "ac3" | "eac3" | "ec3" => Container::Ac3,
            "dts" | "dtshd" => Container::Dts,
            "thd" | "mlp" | "truehd" => Container::Mlp,
            "wv" => Container::WavPack,
            "ape" => Container::Ape,
            "tta" => Container::Tta,
            "dsf" | "dff" => Container::Dsd,
            "mpc" => Container::Musepack,
            "wma" | "asf" => Container::Asf,
            "mka" | "mkv" | "webm" => Container::Matroska,
            _ => Container::Unknown,
        }
    }
}

/// Sniff the container from magic bytes, skipping a leading ID3v2 tag if present.
pub fn sniff_container<R: Read + Seek>(r: &mut R) -> std::io::Result<Container> {
    let offset = id3_end_offset(r)?;
    let mut buf = [0u8; 12];
    r.seek(SeekFrom::Start(offset))?;
    let n = read_up_to(r, &mut buf)?;
    Ok(sniff_bytes(&buf[..n]))
}

/// Offset where audio data starts: 0, or just past a leading ID3v2 tag.
pub(crate) fn id3_end_offset<R: Read + Seek>(r: &mut R) -> std::io::Result<u64> {
    let mut head = [0u8; 10];
    r.seek(SeekFrom::Start(0))?;
    let n = read_up_to(r, &mut head)?;
    if n >= 10 && &head[0..3] == b"ID3" {
        let size = ((head[6] as u64 & 0x7f) << 21)
            | ((head[7] as u64 & 0x7f) << 14)
            | ((head[8] as u64 & 0x7f) << 7)
            | (head[9] as u64 & 0x7f);
        Ok(10 + size)
    } else {
        Ok(0)
    }
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match r.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

fn sniff_bytes(b: &[u8]) -> Container {
    if b.len() >= 4 && &b[0..4] == b"fLaC" {
        return Container::Flac;
    }
    if b.len() >= 4 && &b[0..4] == b"OggS" {
        return Container::Ogg;
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        return Container::Wav;
    }
    if b.len() >= 12 && &b[0..4] == b"FORM" && (&b[8..12] == b"AIFF" || &b[8..12] == b"AIFC") {
        return Container::Aiff;
    }
    if b.len() >= 8 && &b[4..8] == b"ftyp" {
        return Container::Mp4;
    }
    if b.len() >= 2 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0 {
        // Layer bits 00 => ADTS (AAC); otherwise MPEG audio (MP3).
        return if (b[1] & 0x06) == 0 {
            Container::Aac
        } else {
            Container::Mp3
        };
    }
    Container::Unknown
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub extension: String,
    /// Container detected from content.
    pub container: Container,
    /// Short codec name, e.g. "flac", "alac", "mp3".
    pub codec: String,
    pub lossless: bool,
    pub sample_rate: Option<u32>,
    pub bits_per_sample: Option<u32>,
    pub channels: Option<usize>,
    pub duration_secs: Option<f64>,
    /// True when the extension promises a different container than the content.
    pub extension_mismatch: bool,
    /// Set when only the header was identified because the codec cannot be
    /// decoded here. `None` for fully supported files.
    pub header_only: Option<CodecKind>,
    /// Extra detail from the header, e.g. "Dolby Atmos (JOC, complexity 16)".
    pub note: Option<String>,
}

pub fn probe_file(path: &Path) -> Result<FileInfo, ProbeError> {
    let mut file = File::open(path)?;
    let size_bytes = file.metadata()?.len();
    let container = sniff_container(&mut file)?;
    file.seek(SeekFrom::Start(0))?;

    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let expected = Container::from_extension(&extension);
    let extension_mismatch = expected != Container::Unknown && expected != container;

    // Formats we cannot decode are identified from the header alone.
    if let Some(sp) = special::identify(path)? {
        let extension_mismatch = expected != Container::Unknown && expected != sp.container;
        return Ok(FileInfo {
            path: path.to_path_buf(),
            size_bytes,
            extension,
            container: sp.container,
            codec: sp.codec,
            lossless: sp.kind == CodecKind::Lossless,
            sample_rate: sp.sample_rate,
            bits_per_sample: sp.bits,
            channels: sp.channels,
            duration_secs: sp.duration_secs,
            extension_mismatch,
            header_only: Some(sp.kind),
            note: sp.note,
        });
    }

    // No extension hint: we want content-based detection, not trust in the name.
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| ProbeError::Decode(e.to_string()))?;

    let track = probed.format.default_track().ok_or(ProbeError::NoTrack)?;
    let p = &track.codec_params;

    let codec = symphonia::default::get_codecs()
        .get_codec(p.codec)
        .map(|d| d.short_name.to_string())
        .unwrap_or_else(|| "unknown".into());
    let lossless =
        p.codec == CODEC_TYPE_FLAC || p.codec == CODEC_TYPE_ALAC || codec.starts_with("pcm");

    let duration_secs = match (p.n_frames, p.sample_rate) {
        (Some(n), Some(sr)) if sr > 0 => Some(n as f64 / sr as f64),
        _ => None,
    };

    Ok(FileInfo {
        path: path.to_path_buf(),
        size_bytes,
        extension,
        container,
        codec,
        lossless,
        sample_rate: p.sample_rate,
        bits_per_sample: p.bits_per_sample.or(p.bits_per_coded_sample),
        channels: p.channels.map(|c| c.count()),
        duration_secs,
        extension_mismatch,
        header_only: None,
        note: None,
    })
}

/// Probe many files in parallel, preserving input order.
pub fn probe_many(paths: &[PathBuf]) -> Vec<(PathBuf, Result<FileInfo, ProbeError>)> {
    paths
        .par_iter()
        .map(|p| (p.clone(), probe_file(p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sniff(bytes: &[u8]) -> Container {
        sniff_container(&mut Cursor::new(bytes.to_vec())).unwrap()
    }

    #[test]
    fn sniffs_common_magic_bytes() {
        assert_eq!(sniff(b"fLaC\0\0\0\x22rest"), Container::Flac);
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A "), Container::Mp4);
        assert_eq!(sniff(b"RIFF\x24\0\0\0WAVEfmt "), Container::Wav);
        assert_eq!(sniff(b"OggS\0\x02\0\0"), Container::Ogg);
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x00]), Container::Mp3);
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80]), Container::Aac);
        assert_eq!(sniff(b"garbage-data!"), Container::Unknown);
    }

    #[test]
    fn skips_id3_tag_before_flac() {
        // ID3v2 header declaring a 4-byte body, then the FLAC magic.
        let mut data = b"ID3\x03\0\0\0\0\0\x04abcd".to_vec();
        data.extend_from_slice(b"fLaC\0\0\0\x22");
        assert_eq!(sniff(&data), Container::Flac);
    }

    #[test]
    fn extension_maps_to_expected_container() {
        assert_eq!(Container::from_extension("FLAC"), Container::Flac);
        assert_eq!(Container::from_extension("m4a"), Container::Mp4);
        assert_eq!(Container::from_extension("txt"), Container::Unknown);
    }
}
