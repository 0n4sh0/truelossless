use realfft::RealFftPlanner;

pub const FFT_SIZE: usize = 8192;
const HOP: usize = FFT_SIZE / 2;

/// Long-term average power spectrum of a signal.
#[derive(Debug, Clone)]
pub struct Spectrum {
    pub sample_rate: u32,
    /// Width of one bin in Hz.
    pub bin_hz: f32,
    /// Average power per bin in dB, relative to the strongest bin (peak = 0 dB).
    pub db: Vec<f32>,
}

impl Spectrum {
    pub fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }

    pub fn bin_for_hz(&self, hz: f32) -> usize {
        ((hz / self.bin_hz).round() as usize).min(self.db.len().saturating_sub(1))
    }
}

/// 4-term Blackman-Harris window: ~-92 dB sidelobes, so loud low-frequency
/// content does not leak into the high band we need to inspect.
fn blackman_harris(n: usize) -> Vec<f32> {
    let (a0, a1, a2, a3) = (0.35875f64, 0.48829, 0.14128, 0.01168);
    let denom = (n - 1) as f64;
    (0..n)
        .map(|i| {
            let x = std::f64::consts::TAU * i as f64 / denom;
            (a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos()) as f32
        })
        .collect()
}

/// Average the power spectrum over every full FFT frame in `windows`
/// (each window is analysed separately so frames never span a gap).
/// Returns `None` when there is too little audio or it is digital silence.
pub fn average_spectrum(windows: &[Vec<f32>], sample_rate: u32) -> Option<Spectrum> {
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    let window = blackman_harris(FFT_SIZE);

    let mut acc = vec![0f64; FFT_SIZE / 2 + 1];
    let mut frames = 0usize;

    for w in windows {
        let mut start = 0;
        while start + FFT_SIZE <= w.len() {
            for (i, v) in input.iter_mut().enumerate() {
                *v = w[start + i] * window[i];
            }
            fft.process(&mut input, &mut output).ok()?;
            for (a, c) in acc.iter_mut().zip(output.iter()) {
                *a += (c.re as f64).powi(2) + (c.im as f64).powi(2);
            }
            frames += 1;
            start += HOP;
        }
    }

    if frames == 0 {
        return None;
    }
    let peak = acc.iter().cloned().fold(0.0, f64::max);
    if peak <= 0.0 {
        return None;
    }
    let db = acc
        .iter()
        .map(|&p| (10.0 * ((p / peak).max(1e-20)).log10()) as f32)
        .collect();

    Some(Spectrum {
        sample_rate,
        bin_hz: sample_rate as f32 / FFT_SIZE as f32,
        db,
    })
}

#[cfg(test)]
pub(crate) mod testutil {
    /// Deterministic white noise in [-1, 1] (xorshift) so tests are repeatable.
    pub fn white_noise(len: usize, seed: u64) -> Vec<f32> {
        let mut s = seed | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f32 / (1u64 << 52) as f32 - 1.0
            })
            .collect()
    }

    /// Windowed-sinc low-pass FIR (Blackman), `taps` must be odd.
    pub fn lowpass(signal: &[f32], cutoff_hz: f32, sample_rate: f32, taps: usize) -> Vec<f32> {
        let fc = cutoff_hz / sample_rate;
        let m = (taps - 1) as f32 / 2.0;
        let h: Vec<f32> = (0..taps)
            .map(|i| {
                let x = i as f32 - m;
                let sinc = if x == 0.0 {
                    2.0 * fc
                } else {
                    (std::f32::consts::TAU * fc * x).sin() / (std::f32::consts::PI * x)
                };
                let w = 0.42 - 0.5 * (std::f32::consts::TAU * i as f32 / (taps - 1) as f32).cos()
                    + 0.08 * (2.0 * std::f32::consts::TAU * i as f32 / (taps - 1) as f32).cos();
                sinc * w
            })
            .collect();
        (0..signal.len().saturating_sub(taps))
            .map(|n| h.iter().enumerate().map(|(k, c)| c * signal[n + k]).sum())
            .collect()
    }

    /// Write mono 16-bit PCM WAV from float samples in [-1, 1].
    pub fn write_wav(path: &std::path::Path, sample_rate: u32, samples: &[f32]) {
        use std::io::Write;
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
            .collect();
        let mut f = std::fs::File::create(path).unwrap();
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

    /// Band-limited test signal plus a very low noise floor (like dither).
    pub fn bandlimited_noise(sample_rate: u32, cutoff_hz: f32, secs: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * secs) as usize;
        let noise = white_noise(n + 512, 0x1234_5678);
        let mut out = lowpass(&noise, cutoff_hz, sample_rate as f32, 511);
        let floor = white_noise(out.len(), 0x9e37_79b9);
        for (o, f) in out.iter_mut().zip(floor) {
            *o += f * 1e-5;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_has_no_spectrum() {
        assert!(average_spectrum(&[vec![0.0; 20_000]], 44_100).is_none());
    }

    #[test]
    fn too_short_input_has_no_spectrum() {
        assert!(average_spectrum(&[vec![0.5; 100]], 44_100).is_none());
    }

    #[test]
    fn sine_peaks_at_its_frequency() {
        let sr = 44_100u32;
        let sig: Vec<f32> = (0..sr as usize)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr as f32).sin())
            .collect();
        let s = average_spectrum(&[sig], sr).unwrap();
        let peak = s
            .db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!((peak as f32 * s.bin_hz - 1000.0).abs() < 2.0 * s.bin_hz);
    }
}
