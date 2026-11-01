# Contributing to TrueLossless

Thanks for helping. TrueLossless was created by [n4sh](https://github.com/0n4sh0).

## Before you open a pull request

1. Fork the repository and branch from `main`. Direct pushes to `main` are blocked.
2. Run the same checks as CI:

   ```
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

3. Keep each pull request small and about one thing. Add a test for new logic.
4. Do not commit audio files you do not own. Use `gen_samples` for test data.

A pull request needs one approving review and passing checks on Linux, Windows
and macOS before it can be merged.

## Reporting a wrong verdict

Use the "Wrong verdict" issue form. Tell us the codec, sample rate and bit depth,
how you know the file is (not) genuine, and attach a screenshot of the spectrogram
or the CSV row. Please do not attach copyrighted music.

Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Versions and releases

- Versions follow [Semantic Versioning](https://semver.org/): `vMAJOR.MINOR.PATCH`,
  and `-rc.1` style suffixes for pre-releases.
- **The Git tag is the version.** The release workflow stamps the tag into
  `Cargo.toml` at build time, so `--version`, the About window and the archive
  names all agree. The `0.1.0` in the repository is only the development default.
- Only the maintainer releases: merge to `main`, then push a tag such as `v0.2.0`.
  The workflow refuses tags that are not on `main` or not in version form.
- A published tag is never moved. If a release is wrong, ship the next patch
  version.
