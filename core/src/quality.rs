//! Audio quality analysis: levels, loudness, clipping and a spectrogram.
//!
//! This decodes the whole file in one streaming pass, so it is slower than the
//! authenticity check in [`crate::analyze`]. Run it on demand for one file.

use crate::decode::open;
use crate::probe::ProbeError;
use realfft::RealFftPlanner;
use std::f64::consts::PI;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Which decoder produced a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoder {
    /// The built-in Symphonia decoders.
    Native,
    /// An external ffmpeg.
    Ffmpeg,
}

impl Decoder {
    pub fn name(self) -> &'static str {
        match self {
            Decoder::Native => "built-in",
            Decoder::Ffmpeg => "ffmpeg",
        }
    }
}

/// Which decoder to use for a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecoderMode {
    /// Built-in first; ffmpeg when that fails or the format is unsupported.
    #[default]
    Auto,
    /// Built-in only.
    Native,
    /// ffmpeg only.
    Ffmpeg,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct QualityOptions {
    pub mode: DecoderMode,
    /// The format is known to be unsupported by the built-in decoders.
    pub header_only: bool,
    /// Duration from our own header parsing, trusted over ffmpeg's banner
    /// (which only counts the first fragment of fragmented MP4).
    pub duration_hint: Option<f64>,
}

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
    pub decoder: Decoder,
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

/// BS.1770 channel weights (applied to mean-square energy) for the usual
/// 5.1 and 7.1 orders: L R C LFE then surrounds. The LFE does not count and
/// surround channels count +1.5 dB. Other layouts weigh every channel equally.
pub(crate) fn loudness_weights(channels: usize) -> Vec<f64> {
    const SURROUND: f64 = 1.41;
    match channels {
        6 => vec![1.0, 1.0, 1.0, 0.0, SURROUND, SURROUND],
        8 => vec![1.0, 1.0, 1.0, 0.0, SURROUND, SURROUND, SURROUND, SURROUND],
        n => vec![1.0; n],
    }
}

/// Streaming meter: feed interleaved samples, then call [`Meter::finish`].
pub(crate) struct Meter {
    sample_rate: u32,
    clip_level: f32,
    hop: u64,
    kernels: [[f32; TP_TAPS]; 3],
    step_len: u64,
    chans: Vec<ChannelState>,
    weights: Vec<f64>,
    spec: Option<SpecBuilder>,
    frame: u64,
    step_energy: f64,
    steps: Vec<f64>,
    values: Vec<f32>,
}

impl Meter {
    /// `declared_bits` sets the clipping level; `n_frames` (if known) sets
    /// the spectrogram spacing.
    pub(crate) fn new(sample_rate: u32, declared_bits: u32, n_frames: Option<u64>) -> Self {
        let bits = declared_bits.clamp(8, 24);
        Meter {
            sample_rate,
            clip_level: (1.0 - 2f64.powi(1 - bits as i32)) as f32,
            hop: n_frames
                .map(|n| (n / SPEC_TARGET_COLS).max(SPEC_WIN as u64))
                .unwrap_or(8192),
            kernels: true_peak_kernels(),
            step_len: (sample_rate / 10).max(1) as u64,
            chans: Vec::new(),
            weights: Vec::new(),
            spec: None,
            frame: 0,
            step_energy: 0.0,
            steps: Vec::new(),
            values: Vec::new(),
        }
    }

    pub(crate) fn frames(&self) -> u64 {
        self.frame
    }

    /// Add whole frames of `ch` interleaved channels. A change of channel
    /// count after the first call is ignored.
    pub(crate) fn push(&mut self, samples: &[f32], ch: usize) {
        if ch == 0 {
            return;
        }
        if self.chans.is_empty() {
            let fs = self.sample_rate as f64;
            self.chans = (0..ch)
                .map(|_| ChannelState {
                    peak: 0.0,
                    true_peak: 0.0,
                    sum_sq: 0.0,
                    clipped: 0,
                    hist: [0.0; TP_TAPS],
                    filter: k_weighting(fs),
                })
                .collect();
            self.weights = loudness_weights(ch);
            let layers = if ch > 1 { 1 + ch.min(MAX_LAYERS) } else { 1 };
            self.spec = Some(SpecBuilder::new(layers, self.hop));
            self.values = vec![0.0; layers];
        }
        if ch != self.chans.len() {
            return;
        }
        for fr in samples.chunks_exact(ch) {
            let mut mix = 0.0f32;
            let mut loud = 0.0f64;
            for (c, &s) in fr.iter().enumerate() {
                let st = &mut self.chans[c];
                let a = s.abs();
                st.peak = st.peak.max(a);
                st.sum_sq += (s as f64) * (s as f64);
                if a >= self.clip_level {
                    st.clipped += 1;
                }
                st.hist.copy_within(1.., 0);
                st.hist[TP_TAPS - 1] = s;
                let mut tp = a;
                for k in &self.kernels {
                    let v: f32 = st.hist.iter().zip(k).map(|(h, w)| h * w).sum();
                    tp = tp.max(v.abs());
                }
                st.true_peak = st.true_peak.max(tp);
                let shelved = st.filter[0].run(s as f64);
                let z = st.filter[1].run(shelved);
                loud += z * z * self.weights[c];
                mix += s;
                if let Some(v) = self.values.get_mut(c + 1) {
                    *v = s;
                }
            }
            self.values[0] = mix / ch as f32;
            if let Some(sb) = self.spec.as_mut() {
                sb.push(self.frame, &self.values);
            }
            self.step_energy += loud;
            self.frame += 1;
            if self.frame.is_multiple_of(self.step_len) {
                self.steps.push(self.step_energy / self.step_len as f64);
                self.step_energy = 0.0;
            }
        }
    }

    pub(crate) fn finish(self, file_size: Option<u64>) -> Result<Quality, ProbeError> {
        let frame = self.frame;
        let spec = self
            .spec
            .ok_or_else(|| ProbeError::Decode("no audio could be decoded".into()))?;
        if frame == 0 {
            return Err(ProbeError::Decode("no audio could be decoded".into()));
        }
        if spec.cols == 0 {
            return Err(ProbeError::Decode("too short to analyse".into()));
        }
        let chans = self.chans;
        let sample_rate = self.sample_rate;
        let fs = sample_rate as f64;

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
            decoder: Decoder::Native,
            sample_rate,
            channels: chans.len(),
            duration_secs,
            bitrate_bps: file_size.map(|s| s as f64 * 8.0 / duration_secs),
            peak_db,
            true_peak_dbtp: to_db(true_peak),
            rms_db,
            dynamic_range_db: (peak_db - rms_db).max(0.0),
            lufs: integrated_lufs(&self.steps),
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
}

/// Measure `path` with the decoder(s) chosen in `opts`.
///
/// In [`DecoderMode::Auto`] a failure of the first decoder falls back to the
/// other, and if both fail the error names both reasons.
pub fn analyze_quality_with(
    path: &Path,
    ffmpeg: Option<&crate::ffmpeg::Toolchain>,
    opts: QualityOptions,
) -> Result<Quality, ProbeError> {
    analyze_quality_cancellable(path, ffmpeg, opts, &AtomicBool::new(false))
}

/// The error a measurement stopped through its cancel flag returns.
pub fn cancelled() -> ProbeError {
    ProbeError::Decode("cancelled".into())
}

/// Like [`analyze_quality_with`], but stops early with [`cancelled`] once
/// `cancel` is set.
pub fn analyze_quality_cancellable(
    path: &Path,
    ffmpeg: Option<&crate::ffmpeg::Toolchain>,
    opts: QualityOptions,
    cancel: &AtomicBool,
) -> Result<Quality, ProbeError> {
    let analyze_quality = |p: &Path| analyze_quality_until(p, cancel);
    let run_ffmpeg = |tc: &crate::ffmpeg::Toolchain| {
        crate::ffmpeg::analyze_quality_ffmpeg(tc, path, opts.duration_hint, cancel)
    };
    match (opts.mode, ffmpeg) {
        (DecoderMode::Native, _) => analyze_quality(path),
        (DecoderMode::Ffmpeg, None) => Err(ProbeError::Decode(
            "ffmpeg was requested but was not found".into(),
        )),
        (DecoderMode::Ffmpeg, Some(tc)) => run_ffmpeg(tc),
        (DecoderMode::Auto, None) => analyze_quality(path),
        (DecoderMode::Auto, Some(tc)) => {
            let (first, second): (Decoder, Decoder) = if opts.header_only {
                (Decoder::Ffmpeg, Decoder::Native)
            } else {
                (Decoder::Native, Decoder::Ffmpeg)
            };
            let run = |d: Decoder| match d {
                Decoder::Native => analyze_quality(path),
                Decoder::Ffmpeg => run_ffmpeg(tc),
            };
            run(first).or_else(|e1| {
                if cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                run(second).map_err(|e2| {
                    ProbeError::Decode(format!("{}: {e1}; {}: {e2}", first.name(), second.name()))
                })
            })
        }
    }
}

/// Decode the whole file and measure its levels, loudness and spectrogram.
///
/// A decoder panic on a damaged file is reported as an error.
pub fn analyze_quality(path: &Path) -> Result<Quality, ProbeError> {
    analyze_quality_until(path, &AtomicBool::new(false))
}

fn analyze_quality_until(path: &Path, cancel: &AtomicBool) -> Result<Quality, ProbeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        analyze_quality_inner(path, cancel)
    }))
    .unwrap_or_else(|_| Err(ProbeError::Decode("decoder crashed on this file".into())))
}

fn analyze_quality_inner(path: &Path, cancel: &AtomicBool) -> Result<Quality, ProbeError> {
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
        .unwrap_or(16);
    let mut meter = Meter::new(sample_rate, declared_bits, params.n_frames);
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    // Stops at the end of the stream (or on an unrecoverable read error).
    while let Ok(packet) = format.next_packet() {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
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
        if sbuf
            .as_ref()
            .is_none_or(|s| s.capacity() < buf.capacity() * ch)
        {
            sbuf = Some(SampleBuffer::new(buf.capacity() as u64, sp));
        }
        let sb = sbuf.as_mut().unwrap();
        sb.copy_interleaved_ref(buf);
        meter.push(sb.samples(), ch);
    }
    meter.finish(size)
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

    #[test]
    fn surround_weights_follow_bs1770() {
        assert_eq!(loudness_weights(2), vec![1.0, 1.0]);
        let w = loudness_weights(6);
        assert_eq!(w[3], 0.0, "LFE is excluded");
        assert!((w[4] - 1.41).abs() < 1e-9 && (w[5] - 1.41).abs() < 1e-9);
        assert_eq!(loudness_weights(8).len(), 8);
        assert_eq!(loudness_weights(3), vec![1.0; 3]);
    }

    #[test]
    fn meter_matches_file_analysis() {
        let sig = sine(44_100, 3.0, 997.0, 0.5);
        let mut m = Meter::new(44_100, 16, None);
        m.push(&sig, 1);
        let q = m.finish(None).unwrap();
        assert!((q.peak_db + 6.02).abs() < 0.1, "{}", q.peak_db);
        assert!((q.rms_db + 9.03).abs() < 0.1, "{}", q.rms_db);
    }

    #[test]
    fn lfe_only_signal_has_no_loudness() {
        // Energy only in the LFE channel of a 5.1 frame must not count.
        let n = 48_000 * 3;
        let mut data = vec![0.0f32; n * 6];
        for i in 0..n {
            data[i * 6 + 3] = 0.5 * (std::f32::consts::TAU * 60.0 * i as f32 / 48_000.0).sin();
        }
        let mut m = Meter::new(48_000, 24, None);
        m.push(&data, 6);
        let q = m.finish(None).unwrap();
        assert!(q.lufs.is_none(), "{:?}", q.lufs);
        assert_eq!(q.channels, 6);
    }

    #[test]
    fn decoder_modes_without_ffmpeg() {
        let p = std::env::temp_dir().join("fc_q_modes.wav");
        write_wav(&p, 44_100, &sine(44_100, 3.0, 997.0, 0.5));
        let opts = |mode| QualityOptions {
            mode,
            ..Default::default()
        };
        let auto = analyze_quality_with(&p, None, opts(DecoderMode::Auto)).unwrap();
        assert_eq!(auto.decoder, Decoder::Native);
        assert!(analyze_quality_with(&p, None, opts(DecoderMode::Native)).is_ok());
        let e = analyze_quality_with(&p, None, opts(DecoderMode::Ffmpeg)).unwrap_err();
        assert!(e.to_string().contains("ffmpeg"), "{e}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_set_cancel_flag_stops_the_measurement() {
        let p = std::env::temp_dir().join("fc_q_cancel.wav");
        write_wav(&p, 44_100, &sine(44_100, 3.0, 997.0, 0.5));
        let stop = AtomicBool::new(true);
        let e =
            analyze_quality_cancellable(&p, None, QualityOptions::default(), &stop).unwrap_err();
        assert_eq!(e.to_string(), cancelled().to_string());
        std::fs::remove_file(&p).ok();
    }
}
