# flac-checker

Scan folders for audio files and check whether they are what they claim to be
(real FLAC/ALAC/hi-res vs. transcoded or upsampled fakes).

## Layout

- `core/` – analysis library (probing, scanning; spectrum and verdicts to come)
- `cli/`  – `flac-checker <folder-or-file>`
- `gui/`  – egui app (planned)

## Status

- [x] Milestone 1: scan folders, detect real container/codec, flag extension mismatches
- [ ] Milestone 2: decode + spectrum, cutoff detection
- [ ] Milestone 3: verdict engine (fake lossless, fake hi-res, bit depth)
- [ ] Milestone 4: egui GUI

## Usage

```
cargo run -p flac-checker -- path/to/music
```
