# Security policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Use GitHub's private reporting: **Security → Report a vulnerability** on this
repository. Include the affected version, the steps to reproduce, and a sample
file if the problem is triggered by one.

You can expect an answer within 7 days. Confirmed problems are fixed in a new
release and credited in the release notes, unless you prefer to stay anonymous.

## Scope

TrueLossless reads untrusted audio files with third-party decoders. These are
in scope: crashes, hangs or memory problems caused by a crafted file, and
spreadsheet formula injection through the CSV export. The verdicts themselves
are heuristics and a wrong verdict is a normal bug, not a vulnerability.

## Release integrity

Release archives are built by GitHub Actions from a `v*` tag, and each release
includes `SHA256SUMS.txt`. The binaries are not code-signed.
