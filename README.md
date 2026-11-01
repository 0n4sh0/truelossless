# flac-checker

Scan folders for audio files and check whether they are what they claim to be
(real FLAC/ALAC/hi-res vs. transcoded or upsampled fakes).

## Layout

- `core/` – analysis library (probe, decode, spectrum, verdicts)
- `cli/`  – `flac-checker <folder-or-file>`
- `gui/`  – `flac-checker-gui`, minimal egui app

## What it checks

| Check | Catches |
|---|---|
| Content-based container sniffing | MP3/AAC renamed to `.flac`, WAV named `.flac` |
| Spectral bandwidth + edge sharpness | Lossy-to-FLAC transcodes (e.g. 16 kHz cutoff ≈ MP3 128k) |
| Bandwidth vs. sample rate | 44.1/48 kHz material upsampled to 96/192 kHz |
| Effective bit depth | 16-bit data padded into a 24-bit file |
| FLAC MD5 (`--verify`) | Damaged or edited audio |

Verdicts: `GENUINE`, `LOSSY` (honest lossy file), `SUSPECT` (weak evidence),
`CORRUPT`, `FAKE` (strong evidence). Heuristics can be wrong, which is why
weak evidence only yields `SUSPECT`.

## Usage

```
cargo run --release -p flac-checker -- path/to/music
flac-checker path/to/music --verify      # also verify FLAC MD5 (slow)
flac-checker path/to/music --probe-only  # headers only (fast)
flac-checker path/to/music --all         # show reasons for genuine files too
```

GUI: `cargo run --release -p flac-checker-gui`, or drop a folder onto the window.

Exit code is `2` if any file is fake, corrupt or unreadable.

## Status

- [x] Milestone 1: scan folders, detect real container/codec, flag extension mismatches
- [x] Milestone 2: decode + spectrum, cutoff detection
- [x] Milestone 3: verdict engine (fake lossless, fake hi-res, bit depth, MD5)
- [x] Milestone 4: egui GUI (folder picker, drag-drop, list + spectrum + details)
- [ ] Milestone 5: CSV export, drag-and-drop, release build

## Known limits

- Thresholds are tuned on synthetic signals only; real-world tuning is pending.
- ALAC decoding is unverified against real files (no encoder available in tests).
- Mono-as-stereo, clipping and dynamic range checks are not implemented.
