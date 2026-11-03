//! On-disk cache of check results, so unchanged files are not decoded again.
//!
//! An entry is reused only while the file keeps the same size and
//! modification time, and only if the cached run did at least the checks
//! now asked for. The whole cache is dropped when the program version
//! changes, since a new analyser may reach different verdicts.
//!
//! The format is a small hand-written binary encoding. Spectra are stored
//! in half-dB steps, which is finer than the plot can show.

use crate::analyze::Analysis;
use crate::check::{CheckOptions, FileReport};
use crate::cutoff::Cutoff;
use crate::flac::{Md5Report, Md5Status};
use crate::probe::{Container, FileInfo};
use crate::special::CodecKind;
use crate::spectrum::Spectrum;
use crate::verdict::{Status, Verdict};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const MAGIC: &[u8; 4] = b"TLC1";

/// A check result: the report, or why the file could not be read.
pub type CheckResult = Result<FileReport, String>;

struct Entry {
    size: u64,
    mtime_ns: u64,
    analyzed: bool,
    md5: bool,
    result: CheckResult,
}

#[derive(Default)]
pub struct ScanCache {
    entries: HashMap<PathBuf, Entry>,
    dirty: bool,
}

/// Size and modification time, or `None` if the file cannot be read.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((meta.len(), mtime.as_nanos() as u64))
}

impl ScanCache {
    /// Read the cache at `file`. A missing, damaged or outdated cache gives an
    /// empty one.
    pub fn load(file: &Path) -> ScanCache {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| decode(&bytes))
            .unwrap_or_default()
    }

    /// Write the cache to `file` if anything changed. Entries for files that
    /// no longer exist are dropped.
    pub fn save(&mut self, file: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.entries.retain(|p, _| p.exists());
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write a temporary file first so a crash never leaves half a cache.
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, encode(&self.entries))?;
        std::fs::rename(&tmp, file)?;
        self.dirty = false;
        Ok(())
    }

    /// The cached result for `path`, if the file is unchanged and the cached
    /// run covered `opts`.
    pub fn get(&self, path: &Path, opts: CheckOptions) -> Option<CheckResult> {
        let e = self.entries.get(path)?;
        let (size, mtime_ns) = stamp(path)?;
        let fresh = e.size == size && e.mtime_ns == mtime_ns;
        let covers = (e.analyzed || !opts.analyze) && (e.md5 || !opts.verify_md5);
        (fresh && covers).then(|| e.result.clone())
    }

    pub fn insert(&mut self, path: &Path, opts: CheckOptions, result: CheckResult) {
        let Some((size, mtime_ns)) = stamp(path) else {
            return;
        };
        self.entries.insert(
            path.to_path_buf(),
            Entry {
                size,
                mtime_ns,
                analyzed: opts.analyze,
                md5: opts.verify_md5,
                result,
            },
        );
        self.dirty = true;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Encoding

const CONTAINERS: [Container; 18] = [
    Container::Flac,
    Container::Mp4,
    Container::Wav,
    Container::Aiff,
    Container::Mp3,
    Container::Aac,
    Container::Ogg,
    Container::Ac3,
    Container::Dts,
    Container::Mlp,
    Container::WavPack,
    Container::Ape,
    Container::Tta,
    Container::Dsd,
    Container::Musepack,
    Container::Asf,
    Container::Matroska,
    Container::Unknown,
];
const KINDS: [CodecKind; 3] = [CodecKind::Lossy, CodecKind::Lossless, CodecKind::Unknown];
const STATUSES: [Status; 6] = [
    Status::Unchecked,
    Status::Genuine,
    Status::Lossy,
    Status::Suspect,
    Status::Corrupt,
    Status::Fake,
];
const MD5S: [Md5Status; 3] = [
    Md5Status::Match,
    Md5Status::Mismatch,
    Md5Status::Unavailable,
];

fn index_of<T: PartialEq>(list: &[T], v: &T) -> u8 {
    list.iter().position(|x| x == v).unwrap_or(0) as u8
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v.as_bytes());
    }
    fn opt<T>(&mut self, v: &Option<T>, f: impl FnOnce(&mut Self, &T)) {
        self.bool(v.is_some());
        if let Some(x) = v {
            f(self, x);
        }
    }
}

struct R<'a> {
    b: &'a [u8],
    i: usize,
}

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn opt<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<Option<T>> {
        Some(if self.bool()? { Some(f(self)?) } else { None })
    }
    fn pick<T: Copy>(&mut self, list: &[T]) -> Option<T> {
        list.get(self.u8()? as usize).copied()
    }
}

fn encode(entries: &HashMap<PathBuf, Entry>) -> Vec<u8> {
    let mut w = W::default();
    w.0.extend_from_slice(MAGIC);
    w.str(env!("CARGO_PKG_VERSION"));
    w.u32(entries.len() as u32);
    for (path, e) in entries {
        w.str(&path.to_string_lossy());
        w.u64(e.size);
        w.u64(e.mtime_ns);
        w.bool(e.analyzed);
        w.bool(e.md5);
        match &e.result {
            Ok(r) => {
                w.u8(0);
                write_report(&mut w, r);
            }
            Err(msg) => {
                w.u8(1);
                w.str(msg);
            }
        }
    }
    w.0
}

fn decode(bytes: &[u8]) -> Option<ScanCache> {
    let mut r = R { b: bytes, i: 0 };
    if r.take(4)? != MAGIC || r.str()? != env!("CARGO_PKG_VERSION") {
        return None;
    }
    let n = r.u32()?;
    let mut entries = HashMap::new();
    for _ in 0..n {
        let path = PathBuf::from(r.str()?);
        let size = r.u64()?;
        let mtime_ns = r.u64()?;
        let analyzed = r.bool()?;
        let md5 = r.bool()?;
        let result = match r.u8()? {
            0 => Ok(read_report(&mut r)?),
            _ => Err(r.str()?),
        };
        entries.insert(
            path,
            Entry {
                size,
                mtime_ns,
                analyzed,
                md5,
                result,
            },
        );
    }
    Some(ScanCache {
        entries,
        dirty: false,
    })
}

fn write_report(w: &mut W, r: &FileReport) {
    let i = &r.info;
    w.str(&i.path.to_string_lossy());
    w.u64(i.size_bytes);
    w.str(&i.extension);
    w.u8(index_of(&CONTAINERS, &i.container));
    w.str(&i.codec);
    w.bool(i.lossless);
    w.opt(&i.sample_rate, |w, v| w.u32(*v));
    w.opt(&i.bits_per_sample, |w, v| w.u32(*v));
    w.opt(&i.channels, |w, v| w.u64(*v as u64));
    w.opt(&i.duration_secs, |w, v| w.f64(*v));
    w.bool(i.extension_mismatch);
    w.opt(&i.header_only, |w, v| w.u8(index_of(&KINDS, v)));
    w.opt(&i.note, |w, v| w.str(v));

    w.opt(&r.analysis, |w, a| {
        let s = &a.spectrum;
        w.u32(s.sample_rate);
        w.f32(s.bin_hz);
        w.u32(s.db.len() as u32);
        for &d in &s.db {
            w.u8((-d * 2.0).round().clamp(0.0, 255.0) as u8);
        }
        let c = &a.cutoff;
        for v in [c.bandwidth_hz, c.cliff_hz, c.cliff_db, c.floor_db] {
            w.f32(v);
        }
        w.opt(&a.stored_bits, |w, v| w.u32(*v));
        w.opt(&a.effective_bits, |w, v| w.u32(*v));
    });
    w.opt(&r.md5, |w, m| {
        w.u8(index_of(&MD5S, &m.status));
        w.u64(m.decode_errors as u64);
    });
    w.u8(index_of(&STATUSES, &r.verdict.status));
    w.u32(r.verdict.reasons.len() as u32);
    for reason in &r.verdict.reasons {
        w.str(reason);
    }
    w.opt(&r.analysis_error, |w, v| w.str(v));
}

fn read_report(r: &mut R) -> Option<FileReport> {
    let info = FileInfo {
        path: PathBuf::from(r.str()?),
        size_bytes: r.u64()?,
        extension: r.str()?,
        container: r.pick(&CONTAINERS)?,
        codec: r.str()?,
        lossless: r.bool()?,
        sample_rate: r.opt(|r| r.u32())?,
        bits_per_sample: r.opt(|r| r.u32())?,
        channels: r.opt(|r| r.u64().map(|v| v as usize))?,
        duration_secs: r.opt(|r| r.f64())?,
        extension_mismatch: r.bool()?,
        header_only: r.opt(|r| r.pick(&KINDS))?,
        note: r.opt(|r| r.str())?,
    };
    let analysis = r.opt(|r| {
        let sample_rate = r.u32()?;
        let bin_hz = r.f32()?;
        let n = r.u32()? as usize;
        let db = r.take(n)?.iter().map(|&q| -(q as f32) / 2.0).collect();
        let cutoff = Cutoff {
            bandwidth_hz: r.f32()?,
            cliff_hz: r.f32()?,
            cliff_db: r.f32()?,
            floor_db: r.f32()?,
        };
        Some(Analysis {
            spectrum: Spectrum {
                sample_rate,
                bin_hz,
                db,
            },
            cutoff,
            stored_bits: r.opt(|r| r.u32())?,
            effective_bits: r.opt(|r| r.u32())?,
        })
    })?;
    let md5 = r.opt(|r| {
        Some(Md5Report {
            status: r.pick(&MD5S)?,
            decode_errors: r.u64()? as usize,
        })
    })?;
    let status = r.pick(&STATUSES)?;
    let reasons = (0..r.u32()?).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
    let analysis_error = r.opt(|r| r.str())?;
    Some(FileReport {
        info,
        analysis,
        md5,
        verdict: Verdict { status, reasons },
        analysis_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(path: &Path) -> FileReport {
        FileReport {
            info: FileInfo {
                path: path.to_path_buf(),
                size_bytes: 4,
                extension: "flac".into(),
                container: Container::Flac,
                codec: "flac".into(),
                lossless: true,
                sample_rate: Some(44_100),
                bits_per_sample: Some(16),
                channels: Some(2),
                duration_secs: Some(12.5),
                extension_mismatch: false,
                header_only: None,
                note: Some("note".into()),
            },
            analysis: Some(Analysis {
                spectrum: Spectrum {
                    sample_rate: 44_100,
                    bin_hz: 5.38,
                    db: vec![0.0, -10.5, -60.0, -200.0],
                },
                cutoff: Cutoff {
                    bandwidth_hz: 16_000.0,
                    cliff_hz: 16_100.0,
                    cliff_db: 40.0,
                    floor_db: -90.0,
                },
                stored_bits: Some(16),
                effective_bits: Some(16),
            }),
            md5: Some(Md5Report {
                status: Md5Status::Match,
                decode_errors: 0,
            }),
            verdict: Verdict {
                status: Status::Suspect,
                reasons: vec!["band-limited".into(), "second".into()],
            },
            analysis_error: None,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tl-cache-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp("roundtrip");
        let audio = dir.join("a.flac");
        std::fs::write(&audio, b"fLaC").unwrap();
        let store = dir.join("cache.bin");
        let opts = CheckOptions::standard();

        let mut c = ScanCache::default();
        c.insert(&audio, opts, Ok(report(&audio)));
        c.insert(&dir.join("gone.flac"), opts, Err("x".into()));
        c.save(&store).unwrap();

        let c = ScanCache::load(&store);
        assert_eq!(c.len(), 1, "entries for missing files are not stored");
        let r = c.get(&audio, opts).unwrap().unwrap();
        assert_eq!(r.verdict, report(&audio).verdict);
        assert_eq!(r.info.note.as_deref(), Some("note"));
        let a = r.analysis.unwrap();
        assert_eq!(a.cutoff, report(&audio).analysis.unwrap().cutoff);
        assert_eq!(a.spectrum.db, vec![0.0, -10.5, -60.0, -127.5]);
        assert_eq!(r.md5.unwrap().status, Md5Status::Match);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn misses_when_file_changes_or_more_is_asked() {
        let dir = temp("stale");
        let audio = dir.join("a.flac");
        std::fs::write(&audio, b"fLaC").unwrap();
        let opts = CheckOptions::standard();
        let mut c = ScanCache::default();
        c.insert(&audio, opts, Ok(report(&audio)));
        assert!(c.get(&audio, opts).is_some());

        let with_md5 = CheckOptions {
            verify_md5: true,
            ..opts
        };
        assert!(c.get(&audio, with_md5).is_none(), "MD5 was not verified");

        std::fs::write(&audio, b"fLaC-changed").unwrap();
        assert!(c.get(&audio, opts).is_none(), "size changed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_damaged_files() {
        assert!(decode(b"TLC1\x05").is_none());
        assert!(decode(b"nope").is_none());
    }
}
