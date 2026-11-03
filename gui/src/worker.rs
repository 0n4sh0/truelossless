//! Background scan: checks files in parallel and streams results to the UI.

use eframe::egui;
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use truelossless_core::cache::ScanCache;
use truelossless_core::{check_file, find_audio_files, CheckOptions, FileReport};

/// Results of earlier scans, loaded on first use and shared by all scans.
fn cache() -> &'static Mutex<ScanCache> {
    static CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let loaded = crate::settings::cache_file().map(|f| ScanCache::load(&f));
        Mutex::new(loaded.unwrap_or_default())
    })
}

// Only a few messages are in flight at a time, so the size gap is harmless.
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    /// The files found under the chosen path, in display order.
    Files(Vec<PathBuf>),
    /// Result for the file at this index.
    Done(usize, Result<FileReport, String>),
    Finished,
}

pub struct ScanHandle {
    pub rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

impl ScanHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for ScanHandle {
    fn drop(&mut self) {
        // A replaced scan stops picking up new files.
        self.cancel();
    }
}

/// Forget every cached check result, in memory and on disk.
pub fn clear_cache() {
    if let Ok(mut c) = cache().lock() {
        *c = ScanCache::default();
    }
    if let Some(f) = crate::settings::cache_file() {
        let _ = std::fs::remove_file(f);
    }
}

/// Check one file again, ignoring and replacing its cached result.
pub fn recheck(
    path: PathBuf,
    opts: CheckOptions,
    ctx: egui::Context,
) -> Receiver<(PathBuf, Result<FileReport, String>)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Ok(mut c) = cache().lock() {
            c.remove(&path);
        }
        let r = check_file(&path, opts).map_err(|e| e.to_string());
        if let Ok(mut c) = cache().lock() {
            if let Ok(report) = &r {
                c.insert(&path, opts, Ok(report.clone()));
            }
            if let Some(file) = crate::settings::cache_file() {
                let _ = c.save(&file);
            }
        }
        let _ = tx.send((path, r));
        ctx.request_repaint();
    });
    rx
}

pub fn start_scan(root: PathBuf, opts: CheckOptions, ctx: egui::Context) -> ScanHandle {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();

    std::thread::spawn(move || {
        let files = find_audio_files(&root);
        if tx.send(Msg::Files(files.clone())).is_err() {
            return;
        }
        ctx.request_repaint();

        files
            .par_iter()
            .enumerate()
            .for_each_with(tx.clone(), |tx, (i, path)| {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let cached = cache().lock().ok().and_then(|c| c.get(path, opts));
                let result = cached.unwrap_or_else(|| {
                    let r = check_file(path, opts).map_err(|e| e.to_string());
                    // Read errors may be passing (a locked file), so only
                    // successful checks are kept.
                    if let (Ok(report), Ok(mut c)) = (&r, cache().lock()) {
                        c.insert(path, opts, Ok(report.clone()));
                    }
                    r
                });
                let _ = tx.send(Msg::Done(i, result));
                ctx.request_repaint();
            });

        if let (Some(file), Ok(mut c)) = (crate::settings::cache_file(), cache().lock()) {
            let _ = c.save(&file);
        }
        let _ = tx.send(Msg::Finished);
        ctx.request_repaint();
    });

    ScanHandle { rx, cancel }
}
