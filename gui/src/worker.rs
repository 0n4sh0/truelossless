//! Background scan: checks files in parallel and streams results to the UI.

use eframe::egui;
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use truelossless_core::{check_file, find_audio_files, CheckOptions, FileReport};

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
                let result = check_file(path, opts).map_err(|e| e.to_string());
                let _ = tx.send(Msg::Done(i, result));
                ctx.request_repaint();
            });

        let _ = tx.send(Msg::Finished);
        ctx.request_repaint();
    });

    ScanHandle { rx, cancel }
}
