use crate::probe::ProbeError;
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{CodecParameters, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Which bits of the integer samples are ever used.
///
/// A 24-bit file padded up from 16-bit audio has its 8 low bits always zero.
#[derive(Debug, Clone, Default)]
pub struct BitStats {
    /// Width of the decoder's integer samples (0 = unknown / not integer).
    pub container_bits: u32,
    /// OR of the magnitudes of every sample seen (scaled to integers).
    pub or_mask: u64,
    pub samples: u64,
}

impl BitStats {
    fn add(&mut self, scaled: i64) {
        self.or_mask |= scaled.unsigned_abs();
        self.samples += 1;
    }

    /// Number of bits that actually carry information, or `None` when it
    /// cannot be told reliably (silence, too few samples, 32-bit/float data
    /// that does not fit exactly in `f32`).
    pub fn effective_bits(&self) -> Option<u32> {
        if self.samples < 10_000 || self.or_mask == 0 {
            return None;
        }
        if !matches!(self.container_bits, 16 | 24) {
            return None;
        }
        let unused = self.or_mask.trailing_zeros().min(self.container_bits);
        Some(self.container_bits - unused)
    }
}

/// Mono PCM excerpts taken from a file.
#[derive(Debug, Clone)]
pub struct DecodedWindows {
    pub sample_rate: u32,
    /// Each excerpt is contiguous audio (channels averaged to mono, range ~[-1, 1]).
    pub windows: Vec<Vec<f32>>,
    pub bit_stats: BitStats,
}

/// An opened file ready to decode its default audio track.
pub(crate) struct Opened {
    pub format: Box<dyn FormatReader>,
    pub decoder: Box<dyn Decoder>,
    pub track_id: u32,
    pub params: CodecParameters,
}

pub(crate) fn open(path: &Path) -> Result<Opened, ProbeError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| ProbeError::Decode(e.to_string()))?;
    let format = probed.format;

    let track = format.default_track().ok_or(ProbeError::NoTrack)?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| ProbeError::Decode(e.to_string()))?;
    Ok(Opened {
        format,
        decoder,
        track_id,
        params,
    })
}

/// Cap for files whose length is unknown or too short to sample in pieces.
const MAX_CONTIGUOUS_SECS: f64 = 30.0;

/// Decode `n_windows` excerpts of `window_secs` each, spread evenly through the
/// file. Short files (or ones with unknown length) are decoded from the start
/// as a single excerpt, so we never pay for a full decode of a long file.
pub fn decode_windows(
    path: &Path,
    n_windows: usize,
    window_secs: f64,
) -> Result<DecodedWindows, ProbeError> {
    let Opened {
        mut format,
        mut decoder,
        track_id,
        params,
    } = open(path)?;
    let sample_rate = params
        .sample_rate
        .ok_or_else(|| ProbeError::Decode("unknown sample rate".into()))?;
    let total_secs = params.n_frames.map(|n| n as f64 / sample_rate as f64);

    let declared = params.bits_per_sample.or(params.bits_per_coded_sample);
    let want = (window_secs * sample_rate as f64) as usize;
    let mut windows = Vec::new();
    let mut stats = BitStats::default();

    match total_secs {
        Some(total) if total > n_windows as f64 * window_secs * 1.2 && n_windows > 0 => {
            for i in 0..n_windows {
                let center = total * (i + 1) as f64 / (n_windows + 1) as f64;
                let start = (center - window_secs / 2.0).max(0.0);
                let seeked = format.seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time: Time::from(start),
                        track_id: Some(track_id),
                    },
                );
                if seeked.is_err() {
                    break;
                }
                decoder.reset();
                let w = collect(
                    format.as_mut(),
                    decoder.as_mut(),
                    track_id,
                    want,
                    declared,
                    &mut stats,
                );
                if !w.is_empty() {
                    windows.push(w);
                }
            }
        }
        _ => {
            let cap = total_secs
                .unwrap_or(MAX_CONTIGUOUS_SECS)
                .min((n_windows as f64 * window_secs).max(window_secs))
                .min(MAX_CONTIGUOUS_SECS.max(window_secs));
            let w = collect(
                format.as_mut(),
                decoder.as_mut(),
                track_id,
                (cap * sample_rate as f64) as usize,
                declared,
                &mut stats,
            );
            if !w.is_empty() {
                windows.push(w);
            }
        }
    }

    if windows.is_empty() {
        return Err(ProbeError::Decode("no audio could be decoded".into()));
    }
    Ok(DecodedWindows {
        sample_rate,
        windows,
        bit_stats: stats,
    })
}

/// Sample width in bits when the buffer holds exact 16/24-bit integers.
///
/// Some decoders (FLAC) always return 32-bit buffers with the real samples
/// left-aligned, so for those the file's declared width is used. Converting
/// to `f32` and scaling by the declared width still yields exact integers.
fn integer_bits(buf: &AudioBufferRef, declared: Option<u32>) -> u32 {
    match buf {
        AudioBufferRef::S16(_) => 16,
        AudioBufferRef::S24(_) => 24,
        AudioBufferRef::S32(_) => match declared {
            Some(b @ (16 | 24)) => b,
            _ => 0,
        },
        _ => 0,
    }
}

/// Decode packets until `want` mono frames are gathered or the stream ends.
fn collect(
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track_id: u32,
    want: usize,
    declared_bits: Option<u32>,
    stats: &mut BitStats,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(want);
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    while out.len() < want {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                let ch = spec.channels.count().max(1);
                let bits = integer_bits(&buf, declared_bits);
                if stats.container_bits == 0 {
                    stats.container_bits = bits;
                }
                // Exact for 16/24-bit integers: they fit in an f32 mantissa.
                let scale = if bits > 0 {
                    (1u64 << (bits - 1)) as f32
                } else {
                    0.0
                };

                let needed = buf.capacity() * ch;
                if sbuf.as_ref().map_or(true, |s| s.capacity() < needed) {
                    sbuf = Some(SampleBuffer::new(buf.capacity() as u64, spec));
                }
                let sb = sbuf.as_mut().unwrap();
                sb.copy_interleaved_ref(buf);
                for frame in sb.samples().chunks(ch) {
                    if bits == stats.container_bits && bits > 0 {
                        for s in frame {
                            stats.add((s * scale).round() as i64);
                        }
                    }
                    out.push(frame.iter().sum::<f32>() / ch as f32);
                    if out.len() >= want {
                        break;
                    }
                }
            }
            // A corrupt packet should not abort the whole analysis.
            Err(SymError::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::testutil::{white_noise, write_wav};

    fn tone(sample_rate: u32, secs: u32) -> Vec<f32> {
        (0..(sample_rate * secs) as usize)
            .map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn short_file_decodes_as_one_window() {
        let p = std::env::temp_dir().join("fc_short.wav");
        write_wav(&p, 44_100, &tone(44_100, 2));
        let d = decode_windows(&p, 4, 5.0).unwrap();
        assert_eq!(d.sample_rate, 44_100);
        assert_eq!(d.windows.len(), 1);
        assert_eq!(d.windows[0].len(), 2 * 44_100);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn long_file_is_sampled_in_windows() {
        let p = std::env::temp_dir().join("fc_long.wav");
        write_wav(&p, 8_000, &tone(8_000, 60));
        let d = decode_windows(&p, 3, 2.0).unwrap();
        assert_eq!(d.windows.len(), 3);
        assert!(d.windows.iter().all(|w| w.len() == 2 * 8_000));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn full_16_bit_data_uses_all_bits() {
        let p = std::env::temp_dir().join("fc_bits_full.wav");
        let sig: Vec<f32> = white_noise(44_100, 7).iter().map(|s| s * 0.5).collect();
        write_wav(&p, 44_100, &sig);
        let d = decode_windows(&p, 2, 1.0).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(d.bit_stats.effective_bits(), Some(16));
    }

    #[test]
    fn coarsely_quantised_16_bit_data_is_detected() {
        let p = std::env::temp_dir().join("fc_bits_coarse.wav");
        // Round every sample to a multiple of 256 => only 8 real bits.
        let sig: Vec<f32> = white_noise(44_100, 9)
            .iter()
            .map(|s| ((s * 0.5 * 128.0).round() * 256.0) / 32767.0)
            .collect();
        write_wav(&p, 44_100, &sig);
        let d = decode_windows(&p, 2, 1.0).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(d.bit_stats.effective_bits(), Some(8));
    }
}
