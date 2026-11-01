//! Generates a folder of synthetic audio files that exercise every verdict.
//!
//! cargo run -p truelossless-core --features testgen --example gen_samples -- <out-dir>
//!
//! The audio is band-limited noise, so it is only useful for testing the
//! checker and its UI, not for listening.

use std::fs;
use std::path::PathBuf;
use truelossless_core::flac::testflac::write_flac;
use truelossless_core::spectrum::testutil::{bandlimited_noise, write_wav};

/// Noise low-passed at `lowpass_hz`, as integers using `bits - shift` real bits.
fn pcm(sample_rate: u32, lowpass_hz: f32, bits: u32, shift: u32, secs: f32) -> Vec<i32> {
    let amp = (1i64 << (bits - shift - 1)) as f32 * 0.25;
    bandlimited_noise(sample_rate, lowpass_hz, secs)
        .iter()
        .map(|s| ((s * amp).round() as i32) << shift)
        .collect()
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "samples".into()));
    let a = out.join("Album A - Genuine");
    let b = out.join("Album B - Mixed");
    let c = out.join("Hi-Res");
    for d in [&a, &b, &c] {
        fs::create_dir_all(d).unwrap();
    }
    let secs = 8.0;

    // Genuine CD-quality tracks.
    for n in 1..=3 {
        write_flac(
            &a.join(format!("{n:02} - Real track.flac")),
            44_100,
            1,
            16,
            &pcm(44_100, 21_800.0, 16, 0, secs),
            false,
            false,
        );
    }

    // Lossy transcodes of different quality.
    write_flac(
        &b.join("01 - From MP3 128k.flac"),
        44_100,
        1,
        16,
        &pcm(44_100, 16_000.0, 16, 0, secs),
        false,
        false,
    );
    write_flac(
        &b.join("02 - From MP3 192k.flac"),
        44_100,
        1,
        16,
        &pcm(44_100, 18_800.0, 16, 0, secs),
        false,
        false,
    );
    write_flac(
        &b.join("03 - Real track.flac"),
        44_100,
        1,
        16,
        &pcm(44_100, 21_800.0, 16, 0, secs),
        false,
        false,
    );
    write_flac(
        &b.join("04 - Damaged.flac"),
        44_100,
        1,
        16,
        &pcm(44_100, 21_800.0, 16, 0, secs),
        true,
        false,
    );

    // Hi-res: one real, three fakes.
    write_flac(
        &c.join("01 - Real 96k 24bit.flac"),
        96_000,
        1,
        24,
        &pcm(96_000, 44_000.0, 24, 0, secs),
        false,
        false,
    );
    write_flac(
        &c.join("02 - Upsampled from 44k.flac"),
        96_000,
        1,
        24,
        &pcm(96_000, 21_800.0, 24, 0, secs),
        false,
        false,
    );
    write_flac(
        &c.join("03 - 16bit padded to 24.flac"),
        96_000,
        1,
        24,
        &pcm(96_000, 44_000.0, 24, 8, secs),
        false,
        false,
    );
    write_flac(
        &c.join("04 - Upsampled 192k from 96k.flac"),
        192_000,
        1,
        24,
        &pcm(192_000, 45_500.0, 24, 0, secs),
        false,
        false,
    );

    // A WAV pretending to be FLAC.
    write_wav(
        &b.join("05 - Actually a WAV.flac"),
        44_100,
        &bandlimited_noise(44_100, 21_800.0, 3.0),
    );

    println!("Wrote samples to {}", out.display());
}
