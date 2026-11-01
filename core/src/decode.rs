use crate::probe::ProbeError;
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Mono PCM excerpts taken from a file.
#[derive(Debug, Clone)]
pub struct DecodedWindows {
    pub sample_rate: u32,
    /// Each excerpt is contiguous audio (channels averaged to mono, range ~[-1, 1]).
    pub windows: Vec<Vec<f32>>,
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
    let mut format = probed.format;

    let track = format.default_track().ok_or(ProbeError::NoTrack)?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let sample_rate = params
        .sample_rate
        .ok_or_else(|| ProbeError::Decode("unknown sample rate".into()))?;
    let total_secs = params.n_frames.map(|n| n as f64 / sample_rate as f64);

    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| ProbeError::Decode(e.to_string()))?;

    let want = (window_secs * sample_rate as f64) as usize;
    let mut windows = Vec::new();

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
                let w = collect(format.as_mut(), decoder.as_mut(), track_id, want);
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
    })
}

/// Decode packets until `want` mono frames are gathered or the stream ends.
fn collect(
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track_id: u32,
    want: usize,
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
                let needed = buf.capacity() * ch;
                if sbuf.as_ref().map_or(true, |s| s.capacity() < needed) {
                    sbuf = Some(SampleBuffer::new(buf.capacity() as u64, spec));
                }
                let sb = sbuf.as_mut().unwrap();
                sb.copy_interleaved_ref(buf);
                for frame in sb.samples().chunks(ch) {
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
    use std::io::Write;

    /// Write a 16-bit mono PCM WAV with a 1 kHz tone.
    fn write_wav(path: &Path, sample_rate: u32, secs: u32) {
        let n = (sample_rate * secs) as usize;
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = (std::f32::consts::TAU * 1000.0 * i as f32 / sample_rate as f32).sin();
            data.extend_from_slice(&((v * 16000.0) as i16).to_le_bytes());
        }
        let mut f = File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data.len() as u32).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        f.write_all(&data).unwrap();
    }

    #[test]
    fn short_file_decodes_as_one_window() {
        let p = std::env::temp_dir().join("fc_short.wav");
        write_wav(&p, 44_100, 2);
        let d = decode_windows(&p, 4, 5.0).unwrap();
        assert_eq!(d.sample_rate, 44_100);
        assert_eq!(d.windows.len(), 1);
        assert_eq!(d.windows[0].len(), 2 * 44_100);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn long_file_is_sampled_in_windows() {
        let p = std::env::temp_dir().join("fc_long.wav");
        write_wav(&p, 8_000, 60);
        let d = decode_windows(&p, 3, 2.0).unwrap();
        assert_eq!(d.windows.len(), 3);
        assert!(d.windows.iter().all(|w| w.len() == 2 * 8_000));
        std::fs::remove_file(&p).ok();
    }
}
