## TrueLossless {{VERSION}}

Check whether your "lossless" and "hi-res" audio files are the real thing.
TrueLossless finds lossy transcodes saved as FLAC, audio upsampled to
96/192 kHz, 16-bit audio padded into 24-bit files, mislabelled containers and
damaged FLAC streams. It has a desktop app (`truelossless-gui`) and a command
line tool (`truelossless`).

### Which file do I download?

| Platform | Download | Notes |
|---|---|---|
| Windows (x64) | `truelossless-{{VERSION}}-windows-x64.zip` | Normal build |
| Windows (x64) | `truelossless-{{VERSION}}-windows-x64-portable.zip` | UPX-compressed, smaller |
| Linux (x64) | `truelossless-{{VERSION}}-linux-x64.tar.gz` | Normal build |
| Linux (x64) | `truelossless-{{VERSION}}-linux-x64-portable.tar.gz` | UPX-compressed, smaller |
| macOS (Apple silicon) | `truelossless-{{VERSION}}-macos-arm64.tar.gz` | M1 and newer |
| macOS (Intel) | `truelossless-{{VERSION}}-macos-x64.tar.gz` | |

Each archive holds both programs. If you are unsure, pick the normal build.
The portable builds start a little slower, and some antivirus programs flag
UPX-packed files.

### Highlights

- Spectral analysis finds where the real content ends and compares it with the
  claimed sample rate and bit depth.
- Audio quality card: peak, true peak, RMS, dynamic range, loudness (LUFS),
  clipping and per-channel stats.
- Spectrogram with Mix and per-channel views.
- Optional FLAC MD5 verification, and CSV export from the app and the CLI.
- Formats: FLAC, ALAC, WAV, AIFF, MP3, AAC, Ogg Vorbis.

### Verify your download

Compare the SHA-256 of your file with `SHA256SUMS.txt`:

```
sha256sum -c SHA256SUMS.txt --ignore-missing     # Linux, macOS (use shasum -a 256 -c)
Get-FileHash .\truelossless-{{VERSION}}-windows-x64.zip   # Windows PowerShell
```

### First run

The binaries are not code-signed.

- **Windows:** SmartScreen may warn. Choose More info, then Run anyway.
- **macOS:** run `xattr -d com.apple.quarantine truelossless*` once.
- **Linux:** run `chmod +x truelossless*` if the executable bit is missing.

### Good to know

Verdicts are heuristics tuned on synthetic signals so far. Treat `SUSPECT` as a
reason to look closer, not as proof. Please report wrong verdicts as issues,
and include the file's codec, sample rate and spectrogram if you can.

Created by [n4sh](https://github.com/0n4sh0). Source and issues: https://github.com/0n4sh0/truelossless

Report security problems privately, see
[SECURITY.md](https://github.com/0n4sh0/truelossless/blob/main/SECURITY.md).

---

