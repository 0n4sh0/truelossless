//! FLAC-specific checks: STREAMINFO parsing and MD5 integrity verification.
//!
//! A FLAC encoder stores the MD5 of the original PCM (little-endian,
//! interleaved, rounded up to whole bytes per sample) in STREAMINFO. Decoding
//! the whole file and hashing the result proves the audio is intact.

use crate::decode::open;
use crate::probe::{id3_end_offset, ProbeError};
use md5::{Digest, Md5};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::errors::Error as SymError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    /// Total samples per channel (0 = unknown).
    pub total_samples: u64,
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// Parse the 34-byte STREAMINFO block body.
    pub fn parse(b: &[u8; 34]) -> StreamInfo {
        let packed = u64::from_be_bytes(b[10..18].try_into().unwrap());
        StreamInfo {
            sample_rate: (packed >> 44) as u32,
            channels: (((packed >> 41) & 0x7) + 1) as u8,
            bits_per_sample: (((packed >> 36) & 0x1f) + 1) as u8,
            total_samples: packed & 0xf_ffff_ffff,
            md5: b[18..34].try_into().unwrap(),
        }
    }

    /// Encoders write all zeros when they did not compute an MD5.
    pub fn has_md5(&self) -> bool {
        self.md5 != [0u8; 16]
    }
}

/// Read STREAMINFO from the start of a FLAC file. `Ok(None)` if it is not FLAC.
pub fn read_streaminfo(path: &Path) -> Result<Option<StreamInfo>, ProbeError> {
    let mut f = File::open(path)?;
    let offset = id3_end_offset(&mut f)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut head = [0u8; 8];
    if f.read_exact(&mut head).is_err() || &head[0..4] != b"fLaC" {
        return Ok(None);
    }
    // First metadata block must be STREAMINFO (type 0) of length 34.
    let len = u32::from_be_bytes([0, head[5], head[6], head[7]]);
    if head[4] & 0x7f != 0 || len != 34 {
        return Ok(None);
    }
    let mut body = [0u8; 34];
    f.read_exact(&mut body)?;
    Ok(Some(StreamInfo::parse(&body)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Md5Status {
    /// Decoded audio hashes to the stored MD5.
    Match,
    /// Hash differs or decoding hit errors: the file is damaged or edited.
    Mismatch,
    /// The encoder stored no MD5, so integrity cannot be proven.
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
pub struct Md5Report {
    pub status: Md5Status,
    pub decode_errors: usize,
}

/// Append the PCM bytes of `buf` in the order FLAC hashes them.
/// Returns false for sample formats FLAC never produces.
///
/// `bits` is the stream's real sample width. symphonia's FLAC decoder returns
/// 32-bit buffers with the samples left-aligned, so those are shifted back down
/// and written with `ceil(bits / 8)` bytes, as the FLAC format specifies.
fn push_pcm(out: &mut Vec<u8>, buf: &AudioBufferRef, bits: u32) -> bool {
    macro_rules! interleave {
        ($b:expr, $bytes:expr) => {{
            // The buffers arrive as Cow<AudioBuffer<_>>; borrow the inner buffer.
            let b = &**$b;
            let ch = b.spec().channels.count();
            for i in 0..b.frames() {
                for c in 0..ch {
                    $bytes(out, b.chan(c)[i]);
                }
            }
            true
        }};
    }
    match buf {
        AudioBufferRef::S8(b) => interleave!(b, |o: &mut Vec<u8>, s: i8| o.push(s as u8)),
        AudioBufferRef::S16(b) => {
            interleave!(b, |o: &mut Vec<u8>, s: i16| o.extend_from_slice(&s.to_le_bytes()))
        }
        AudioBufferRef::S24(b) => interleave!(b, |o: &mut Vec<u8>, s: symphonia::core::sample::i24| {
            o.extend_from_slice(&s.inner().to_le_bytes()[..3])
        }),
        AudioBufferRef::S32(b) => {
            let bytes = bits.div_ceil(8).clamp(1, 4) as usize;
            let shift = 32 - bits.clamp(1, 32);
            interleave!(b, |o: &mut Vec<u8>, s: i32| {
                o.extend_from_slice(&(s >> shift).to_le_bytes()[..bytes])
            })
        }
        _ => false,
    }
}

/// Fully decode a FLAC file and compare the result with the stored MD5.
/// This reads the whole file, so it is much slower than the spectrum check.
pub fn verify_md5(path: &Path) -> Result<Md5Report, ProbeError> {
    let info = read_streaminfo(path)?
        .ok_or_else(|| ProbeError::Decode("not a FLAC file".into()))?;
    if !info.has_md5() {
        return Ok(Md5Report {
            status: Md5Status::Unavailable,
            decode_errors: 0,
        });
    }

    let mut o = open(path)?;
    let mut hasher = Md5::new();
    let mut scratch = Vec::new();
    let mut decode_errors = 0usize;
    let mut supported = true;

    loop {
        let packet = match o.format.next_packet() {
            Ok(p) => p,
            Err(_) => break, // end of stream (or unrecoverable read error)
        };
        if packet.track_id() != o.track_id {
            continue;
        }
        match o.decoder.decode(&packet) {
            Ok(buf) => {
                scratch.clear();
                supported &= push_pcm(&mut scratch, &buf, info.bits_per_sample as u32);
                hasher.update(&scratch);
            }
            Err(SymError::DecodeError(_)) => decode_errors += 1,
            Err(_) => break,
        }
    }

    let digest: [u8; 16] = hasher.finalize().into();
    let status = if !supported {
        Md5Status::Unavailable
    } else if decode_errors == 0 && digest == info.md5 {
        Md5Status::Match
    } else {
        Md5Status::Mismatch
    };
    Ok(Md5Report {
        status,
        decode_errors,
    })
}

/// Test helper: a minimal FLAC encoder using verbatim subframes. It produces
/// valid files (correct CRCs and MD5) without needing an external encoder.
#[cfg(test)]
pub(crate) mod testflac {
    use super::*;
    use std::io::Write;

    struct Bits {
        bytes: Vec<u8>,
        used: u32,
    }
    impl Bits {
        fn new() -> Self {
            Bits {
                bytes: Vec::new(),
                used: 0,
            }
        }
        fn put(&mut self, value: u32, n: u32) {
            for i in (0..n).rev() {
                if self.used % 8 == 0 {
                    self.bytes.push(0);
                }
                let bit = ((value >> i) & 1) as u8;
                *self.bytes.last_mut().unwrap() |= bit << (7 - self.used % 8);
                self.used += 1;
            }
        }
    }

    fn crc8(data: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &b in data {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
            }
        }
        crc
    }

    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0u16;
        for &b in data {
            crc ^= (b as u16) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
            }
        }
        crc
    }

    fn utf8_number(n: u32) -> Vec<u8> {
        match n {
            0..=0x7f => vec![n as u8],
            0x80..=0x7ff => vec![0xC0 | (n >> 6) as u8, 0x80 | (n & 0x3f) as u8],
            0x800..=0xffff => vec![
                0xE0 | (n >> 12) as u8,
                0x80 | ((n >> 6) & 0x3f) as u8,
                0x80 | (n & 0x3f) as u8,
            ],
            _ => vec![
                0xF0 | (n >> 18) as u8,
                0x80 | ((n >> 12) & 0x3f) as u8,
                0x80 | ((n >> 6) & 0x3f) as u8,
                0x80 | (n & 0x3f) as u8,
            ],
        }
    }

    /// Write interleaved `pcm` (values within `bits` range) as a FLAC file.
    /// With `corrupt_md5` the stored MD5 is deliberately wrong; with
    /// `no_md5` it is all zeros.
    pub fn write_flac(
        path: &Path,
        sample_rate: u32,
        channels: usize,
        bits: u32,
        pcm: &[i32],
        corrupt_md5: bool,
        no_md5: bool,
    ) {
        const BLOCK: usize = 4096;
        let frames = pcm.len() / channels;

        let mut raw = Vec::new();
        for s in pcm {
            raw.extend_from_slice(&s.to_le_bytes()[..(bits / 8) as usize]);
        }
        let mut md5: [u8; 16] = Md5::digest(&raw).into();
        if corrupt_md5 {
            md5[0] ^= 0xff;
        }
        if no_md5 {
            md5 = [0; 16];
        }

        let mut out = b"fLaC".to_vec();
        out.extend_from_slice(&[0x80, 0, 0, 34]);
        out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
        out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let packed: u64 = ((sample_rate as u64) << 44)
            | (((channels - 1) as u64) << 41)
            | (((bits - 1) as u64) << 36)
            | frames as u64;
        out.extend_from_slice(&packed.to_be_bytes());
        out.extend_from_slice(&md5);

        for (n, chunk) in pcm.chunks(BLOCK * channels).enumerate() {
            let bs = chunk.len() / channels;
            let mut frame = vec![0xFF, 0xF8, 0x70, ((channels - 1) as u8) << 4];
            frame.extend(utf8_number(n as u32));
            frame.extend_from_slice(&((bs - 1) as u16).to_be_bytes());
            frame.push(crc8(&frame));

            let mut w = Bits::new();
            for c in 0..channels {
                w.put(0x02, 8); // verbatim subframe, no wasted bits
                for i in 0..bs {
                    let v = chunk[i * channels + c];
                    w.put((v as u32) & ((1u64 << bits) - 1) as u32, bits);
                }
            }
            frame.extend_from_slice(&w.bytes);
            let crc = crc16(&frame);
            frame.extend_from_slice(&crc.to_be_bytes());
            out.extend_from_slice(&frame);
        }
        File::create(path).unwrap().write_all(&out).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testflac::write_flac;
    use super::*;
    use crate::spectrum::testutil::white_noise;

    fn noise_pcm(n: usize, bits: u32, shift: u32) -> Vec<i32> {
        let amp = ((1i64 << (bits - shift - 1)) - 1) as f32 * 0.5;
        white_noise(n, 42)
            .iter()
            .map(|s| ((s * amp).round() as i32) << shift)
            .collect()
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn parses_streaminfo_fields() {
        let p = tmp("fc_info.flac");
        write_flac(&p, 96_000, 2, 24, &noise_pcm(2 * 5000, 24, 0), false, false);
        let si = read_streaminfo(&p).unwrap().unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(si.sample_rate, 96_000);
        assert_eq!(si.channels, 2);
        assert_eq!(si.bits_per_sample, 24);
        assert_eq!(si.total_samples, 5000);
        assert!(si.has_md5());
    }

    #[test]
    fn non_flac_has_no_streaminfo() {
        let p = tmp("fc_notflac.bin");
        std::fs::write(&p, b"RIFF....WAVEfmt ").unwrap();
        assert!(read_streaminfo(&p).unwrap().is_none());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn intact_16_bit_stereo_file_matches() {
        let p = tmp("fc_ok16.flac");
        write_flac(&p, 44_100, 2, 16, &noise_pcm(2 * 10_000, 16, 0), false, false);
        let r = verify_md5(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(r.status, Md5Status::Match);
        assert_eq!(r.decode_errors, 0);
    }

    #[test]
    fn intact_24_bit_file_matches() {
        let p = tmp("fc_ok24.flac");
        write_flac(&p, 96_000, 1, 24, &noise_pcm(9_000, 24, 0), false, false);
        let r = verify_md5(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(r.status, Md5Status::Match);
    }

    #[test]
    fn wrong_stored_md5_is_a_mismatch() {
        let p = tmp("fc_bad.flac");
        write_flac(&p, 44_100, 1, 16, &noise_pcm(9_000, 16, 0), true, false);
        let r = verify_md5(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(r.status, Md5Status::Mismatch);
    }

    #[test]
    fn missing_md5_is_unavailable() {
        let p = tmp("fc_nomd5.flac");
        write_flac(&p, 44_100, 1, 16, &noise_pcm(9_000, 16, 0), false, true);
        let r = verify_md5(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(r.status, Md5Status::Unavailable);
    }

    #[test]
    fn flipped_audio_byte_is_detected() {
        let p = tmp("fc_flip.flac");
        write_flac(&p, 44_100, 1, 16, &noise_pcm(9_000, 16, 0), false, false);
        let mut bytes = std::fs::read(&p).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x55;
        std::fs::write(&p, &bytes).unwrap();
        let r = verify_md5(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(r.status, Md5Status::Mismatch);
    }

    #[test]
    fn padded_24_bit_flac_reports_16_effective_bits() {
        use crate::decode::decode_windows;
        let p = tmp("fc_pad24.flac");
        // 16-bit audio shifted left by 8 => 24-bit container, 16 real bits.
        write_flac(&p, 96_000, 1, 24, &noise_pcm(40_000, 24, 8), false, false);
        let d = decode_windows(&p, 2, 0.2).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(d.bit_stats.container_bits, 24);
        assert_eq!(d.bit_stats.effective_bits(), Some(16));
    }

    #[test]
    fn real_24_bit_flac_reports_24_effective_bits() {
        use crate::decode::decode_windows;
        let p = tmp("fc_real24.flac");
        write_flac(&p, 96_000, 1, 24, &noise_pcm(40_000, 24, 0), false, false);
        let d = decode_windows(&p, 2, 0.2).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(d.bit_stats.effective_bits(), Some(24));
    }
}
