//! Audio quality analysis: levels, loudness, clipping and a spectrogram.
//!
//! This decodes the whole file in one streaming pass, so it is slower than the
//! authenticity check in [`crate::analyze`]. Run it on demand for one file.

use crate::decode::open;
use crate::probe::ProbeError;
use realfft::RealFftPlanner;
use std::f64::consts::PI;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::errors::Error as SymError;

/// FFT size of one spectrogram column.
pub const SPEC_WIN: usize = 2048;
/// Frequency rows per spectrogram column.
pub const SPEC_ROWS: usize = SPEC_WIN / 2;
/// Spectrogram columns aimed for (the real count depends on file length).
const SPEC_TARGET_COLS: u64 = 900;
/// Level that maps to byte 0 in a spectrogram.
pub const SPEC_DB_MIN: f32 = -120.0;
/// Most channels that get their own spectrogram layer.
const MAX_LAYERS: usize = 8;

#[derive(Debug, Clone)]
pub struct ChannelStats {
    pub peak_db: f64,
    pub rms_db: f64,
    /// Peak minus RMS, in dB (the crest factor).
    pub dynamic_range_db: f64,
    /// Samples at full scale.
    pub clipped: u64,
}

/// Time/frequency picture of the audio. Values 0..=255 map to
/// `SPEC_DB_MIN`..0 dBFS.
#[derive(Debug, Clone)]
pub struct Spectrogram {
    pub sample_rate: u32,
    pub cols: usize,
    pub rows: usize,
    pub duration_secs: f64,
    /// Layer 0 is the channel mix. With more than one channel, layers 1.. are
    /// the single channels. Each layer is column-major: `col * rows + row`,
    /// where row 0 is 0 Hz.
    pub layers: Vec<Vec<u8>>,
}

impl Spectrogram {
    pub fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }
}

#[derive(Debug, Clone)]
pub struct Quality {
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_secs: f64,
    pub bitrate_bps: Option<f64>,
    pub peak_db: f64,
    pub true_peak_dbtp: f64,
    pub rms_db: f64,
    pub dynamic_range_db: f64,
    /// Integrated loudness (ITU-R BS.1770 / EBU R128). `None` for clips under ~0.4 s.
    pub lufs: Option<f64>,
    pub clipped: u64,
    pub per_channel: Vec<ChannelStats>,
    pub spectrogram: Spectrogram,
}

fn to_db(x: f64) -> f64 {
    if x <= 1e-12 {
        -120.0
    } else {
        20.0 * x.log10()
    }
}

/// Second-order filter section, transposed direct form II.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages of BS.1770 for any sample rate.
fn k_weighting(fs: f64) -> [Biquad; 2] {
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let high = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    [shelf, high]
}

/// Integrated loudness from per-100 ms energies (4 steps = one 400 ms block).
fn integrated_lufs(steps: &[f64]) -> Option<f64> {
    if steps.len() < 4 {
        return None;
    }
    let lufs = |e: f64| -0.691 + 10.0 * e.log10();
    let blocks: Vec<f64> = steps
        .windows(4)
        .map(|w| w.iter().sum::<f64>() / 4.0)
        .collect();
    let loud: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|&e| e > 0.0 && lufs(e) > -70.0)
        .collect();
    if loud.is_empty() {
        return None;
    }
    let mean = loud.iter().sum::<f64>() / loud.len() as f64;
    let gate = lufs(mean) - 10.0;
    let kept: Vec<f64> = loud.into_iter().filter(|&e| lufs(e) > gate).collect();
    if kept.is_empty() {
        return None;
    }
    Some(lufs(kept.iter().sum::<f64>() / kept.len() as f64))
}

const TP_TAPS: usize = 12;

/// 4x oversampling kernels (windowed sinc) for the three in-between points.
fn true_peak_kernels() -> [[f32; TP_TAPS]; 3] {
    let mut out = [[0f32; TP_TAPS]; 3];
    for (phase, row) in out.iter_mut().enumerate() {
        let pos = 5.0 + (phase + 1) as f64 / 4.0;
        for (k, w) in row.iter_mut().enumerate() {
            let x = pos - k as f64;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            let win = if x.abs() < 6.0 {
                0.5 * (1.0 + (PI * x / 6.0).cos())
            } else {
                0.0
            };
            *w = (sinc * win) as f32;
        }
    }
    out
}

struct ChannelState {
    peak: f32,
    true_peak: f32,
    sum_sq: f64,
    clipped: u64,
    hist: [f32; TP_TAPS],
    filter: [Biquad; 2],
}

/// Collects spectrogram columns from sparse windows of the signal.
struct SpecBuilder {
    hop: u64,
    window: Vec<f32>,
    pending: Vec<Vec<f32>>,
    layers: Vec<Vec<u8>>,
    cols: usize,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
}

impl SpecBuilder {
    fn new(n_layers: usize, hop: u64) -> Self {
        let window = (0..SPEC_WIN)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / SPEC_WIN as f32).cos())
            .collect();
        SpecBuilder {
            hop,
            window,
            pending: vec![Vec::with_capacity(SPEC_WIN); n_layers],
            layers: vec![Vec::new(); n_layers],
            cols: 0,
            fft: RealFftPlanner::<f32>::new().plan_fft_forward(SPEC_WIN),
        }
    }

    /// Feed one frame (`values[0]` is the mix, then each channel).
    fn push(&mut self, index: u64, values: &[f32]) {
        let pos = index % self.hop;
        if pos >= SPEC_WIN as u64 {
            return;
        }
        for (p, v) in self.pending.iter_mut().zip(values) {
            p.push(*v);
        }
        if pos == SPEC_WIN as u64 - 1 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let norm = SPEC_WIN as f32 / 4.0;
        for (pend, layer) in self.pending.iter_mut().zip(self.layers.iter_mut()) {
            if pend.len() == SPEC_WIN {
                let mut input: Vec<f32> =
                    pend.iter().zip(&self.window).map(|(s, w)| s * w).collect();
                let mut out = self.fft.make_output_vec();
                if self.fft.process(&mut input, &mut out).is_ok() {
                    for c in out.iter().take(SPEC_ROWS) {
                        let db = 20.0 * (c.norm() / norm + 1e-12).log10();
                        let v = ((db - SPEC_DB_MIN) / -SPEC_DB_MIN * 255.0).clamp(0.0, 255.0);
                        layer.push(v as u8);
                    }
                } else {
                    layer.extend(std::iter::repeat_n(0, SPEC_ROWS));
                }
            } else {
                layer.extend(std::iter::repeat_n(0, SPEC_ROWS));
            }
            pend.clear();
        }
        self.cols += 1;
    }
}

/// Decode the whole file and measure its levels, loudness and spectrogram.
///
/// A decoder panic on a damaged file is reported as an error.
pub fn analyze_quality(path: &Path) -> Result<Quality, ProbeError> {
    std::panic::catch_unwind(|| analyze_quality_inner(path))
        .unwrap_or_else(|_| Err(ProbeError::Decode("decoder crashed on this file".into())))
}

fn analyze_quality_inner(path: &Path) -> Result<Quality, ProbeError> {
    let size = std::fs::metadata(path).map(|m| m.len()).ok();
    let crate::decode::Opened {
        mut format,
        mut decoder,
        track_id,
        params,
    } = open(path)?;
    let sample_rate = params
        .sample_rate
        .ok_or_else(|| ProbeError::Decode("unknown sample rate".into()))?;
    let declared_bits = params
        .bits_per_sample
        .or(params.bits_per_coded_sample)
        .unwrap_or(16)
        .clamp(8, 24);
    let clip_level = (1.0 - 2f64.powi(1 - declared_bits as i32)) as f32;
    let hop = params
        .n_frames
        .map(|n| (n / SPEC_TARGET_COLS).max(SPEC_WIN as u64))
        .unwrap_or(8192);

    let fs = sample_rate as f64;
    let step_len = (sample_rate / 10).max(1) as u64;
    let kernels = true_peak_kernels();

    let mut chans: Vec<ChannelState> = Vec::new();
    let mut spec: Option<SpecBuilder> = None;
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    let mut frame: u64 = 0;
    let mut step_energy = 0.0f64;
    let mut steps: Vec<f64> = Vec::new();
    let mut values: Vec<f32> = Vec::new();

    // Stops at the end of the stream (or on an unrecoverable read error).
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let sp = *buf.spec();
        let ch = sp.channels.count().max(1);
        if chans.is_empty() {
            chans = (0..ch)
                .map(|_| ChannelState {
                    peak: 0.0,
                    true_peak: 0.0,
                    sum_sq: 0.0,
                    clipped: 0,
                    hist: [0.0; TP_TAPS],
                    filter: k_weighting(fs),
                })
                .collect();
            let layers = if ch > 1 { 1 + ch.min(MAX_LAYERS) } else { 1 };
            spec = Some(SpecBuilder::new(layers, hop));
            values = vec![0.0; layers];
        }
        if ch != chans.len() {
            continue; // channel layout changed mid-stream: ignore the odd packet
        }
        if sbuf
            .as_ref()
            .is_none_or(|s| s.capacity() < buf.capacity() * ch)
        {
            sbuf = Some(SampleBuffer::new(buf.capacity() as u64, sp));
        }
        let sb = sbuf.as_mut().unwrap();
        sb.copy_interleaved_ref(buf);

        for fr in sb.samples().chunks_exact(ch) {
            let mut mix = 0.0f32;
            let mut loud = 0.0f64;
            for (c, &s) in fr.iter().enumerate() {
                let st = &mut chans[c];
                let a = s.abs();
                st.peak = st.peak.max(a);
                st.sum_sq += (s as f64) * (s as f64);
                if a >= clip_level {
                    st.clipped += 1;
                }
                st.hist.copy_within(1.., 0);
                st.hist[TP_TAPS - 1] = s;
                let mut tp = a;
                for k in &kernels {
                    let v: f32 = st.hist.iter().zip(k).map(|(h, w)| h * w).sum();
                    tp = tp.max(v.abs());
                }
                st.true_peak = st.true_peak.max(tp);
                let shelved = st.filter[0].run(s as f64);
                let z = st.filter[1].run(shelved);
                loud += z * z;
                mix += s;
                if let Some(v) = values.get_mut(c + 1) {
                    *v = s;
                }
            }
            values[0] = mix / ch as f32;
            if let Some(sb) = spec.as_mut() {
                sb.push(frame, &values);
            }
            step_energy += loud;
            frame += 1;
            if frame.is_multiple_of(step_len) {
                steps.push(step_energy / step_len as f64);
                step_energy = 0.0;
            }
        }
    }

    let spec = spec.ok_or_else(|| ProbeError::Decode("no audio could be decoded".into()))?;
    if frame == 0 {
        return Err(ProbeError::Decode("no audio could be decoded".into()));
    }
    if spec.cols == 0 {
        return Err(ProbeError::Decode("too short to analyse".into()));
    }

    let duration_secs = frame as f64 / fs;
    let per_channel: Vec<ChannelStats> = chans
        .iter()
        .map(|c| {
            let peak_db = to_db(c.peak as f64);
            let rms_db = to_db((c.sum_sq / frame as f64).sqrt());
            ChannelStats {
                peak_db,
                rms_db,
                dynamic_range_db: (peak_db - rms_db).max(0.0),
                clipped: c.clipped,
            }
        })
        .collect();
    let peak = chans.iter().map(|c| c.peak).fold(0.0, f32::max) as f64;
    let true_peak = chans.iter().map(|c| c.true_peak).fold(0.0, f32::max) as f64;
    let total_sq: f64 = chans.iter().map(|c| c.sum_sq).sum();
    let rms_db = to_db((total_sq / (frame as f64 * chans.len() as f64)).sqrt());
    let peak_db = to_db(peak);

    Ok(Quality {
        sample_rate,
        channels: chans.len(),
        duration_secs,
        bitrate_bps: size.map(|s| s as f64 * 8.0 / duration_secs),
        peak_db,
        true_peak_dbtp: to_db(true_peak),
        rms_db,
        dynamic_range_db: (peak_db - rms_db).max(0.0),
        lufs: integrated_lufs(&steps),
        clipped: chans.iter().map(|c| c.clipped).sum(),
        per_channel,
        spectrogram: Spectrogram {
            sample_rate,
            cols: spec.cols,
            rows: SPEC_ROWS,
            duration_secs,
            layers: spec.layers,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::testutil::write_wav;

    fn sine(sr: u32, secs: f32, hz: f32, amp: f32) -> Vec<f32> {
        (0..(sr as f32 * secs) as usize)
            .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / sr as f32).sin())
            .collect()
    }

    fn run(name: &str, sr: u32, sig: &[f32]) -> Quality {
        let p = std::env::temp_dir().join(name);
        write_wav(&p, sr, sig);
        let q = analyze_quality(&p).unwrap();
        std::fs::remove_file(&p).ok();
        q
    }

    #[test]
    fn sine_levels_are_correct() {
        let q = run("fc_q_sine.wav", 44_100, &sine(44_100, 3.0, 997.0, 0.5));
        assert!((q.peak_db + 6.02).abs() < 0.1, "{}", q.peak_db);
        assert!((q.rms_db + 9.03).abs() < 0.1, "{}", q.rms_db);
        assert!(
            (q.dynamic_range_db - 3.01).abs() < 0.15,
            "{}",
            q.dynamic_range_db
        );
        assert_eq!(q.clipped, 0);
        assert_eq!(q.channels, 1);
    }

    #[test]
    fn louder_signal_reads_louder_in_lufs() {
        let a = run("fc_q_a.wav", 44_100, &sine(44_100, 3.0, 997.0, 0.1))
            .lufs
            .unwrap();
        let b = run(
            "fc_q_b.wav",
            44_100,
            &sine(44_100, 3.0, 997.0, 0.316_227_77),
        )
        .lufs
        .unwrap();
        assert!((b - a - 10.0).abs() < 0.2, "{a} {b}");
        // Mono sine at -20 dBFS peak: mean square 0.005 => about -23.7 LUFS.
        assert!((a + 23.7).abs() < 1.5, "{a}");
    }

    #[test]
    fn clipping_is_counted() {
        let sig: Vec<f32> = sine(44_100, 2.0, 200.0, 2.0)
            .iter()
            .map(|s| s.clamp(-1.0, 1.0))
            .collect();
        let q = run("fc_q_clip.wav", 44_100, &sig);
        assert!(q.clipped > 100, "{}", q.clipped);
        assert!(q.peak_db > -0.1);
    }

    #[test]
    fn true_peak_is_at_least_sample_peak() {
        let q = run(
            "fc_q_tp.wav",
            44_100,
            &sine(44_100, 2.0, 11_025.0 + 300.0, 0.9),
        );
        assert!(q.true_peak_dbtp >= q.peak_db - 0.01);
    }

    #[test]
    fn spectrogram_shows_the_tone_row() {
        let q = run("fc_q_spec.wav", 44_100, &sine(44_100, 4.0, 5_000.0, 0.5));
        let s = &q.spectrogram;
        assert!(s.cols > 10);
        let col = &s.layers[0][(s.cols / 2) * s.rows..(s.cols / 2 + 1) * s.rows];
        let (row, _) = col.iter().enumerate().max_by_key(|(_, v)| **v).unwrap();
        let hz = row as f32 * q.sample_rate as f32 / SPEC_WIN as f32;
        assert!((hz - 5_000.0).abs() < 60.0, "{hz}");
    }

    #[test]
    fn silence_has_no_loudness() {
        let q = run("fc_q_silence.wav", 44_100, &vec![0.0; 44_100 * 2]);
        assert!(q.lufs.is_none());
        assert!(q.peak_db <= -100.0);
    }
}
