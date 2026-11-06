//! Notices file changes by polling mtimes: for watch mode (on a thread) and for keeping
//! the file list current (one directory, cheap enough to do inline).

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

/// Build output, dependencies and caches: they change on every build and nobody edits them.
const SKIP: &[&str] = &[
    "target", "node_modules", "build", "build-release", "dist", "out", "zig-out", "__pycache__", "venv",
];
/// Stop descending after this many entries so a huge tree can't stall the thread.
const MAX_ENTRIES: usize = 50_000;
const INTERVAL: Duration = Duration::from_millis(700);

/// Watches a tree on a background thread. Dropping it stops the thread.
pub struct Watcher {
    rx: Receiver<()>,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        thread::spawn(move || {
            let mut last = tree_fingerprint(&root);
            while !flag.load(Ordering::Relaxed) {
                thread::sleep(INTERVAL);
                let now = tree_fingerprint(&root);
                if now == last {
                    continue;
                }
                // Let a burst of saves (format on save, git checkout) settle first.
                thread::sleep(Duration::from_millis(250));
                last = tree_fingerprint(&root);
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
        Self { rx, stop }
    }

    /// True if something changed since the last call.
    pub fn changed(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// Entry hashes are summed, so the order read_dir returns them in doesn't matter.

fn tree_fingerprint(root: &Path) -> u64 {
    let mut sum = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return sum;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !name.starts_with('.') && !SKIP.contains(&name.as_ref()) {
                    stack.push(e.path());
                }
                continue;
            }
            if name.starts_with(".#") || name.ends_with('~') || name.ends_with(".swp") {
                continue; // editor lock and backup files
            }
            sum = sum.wrapping_add(entry_hash(&e));
        }
    }
    sum
}

/// Names, sizes and mtimes of one directory's entries, plus `extra` files (e.g. the git index).
pub fn dir_fingerprint(dir: &Path, extra: &[PathBuf]) -> u64 {
    let mut sum = fs::read_dir(dir)
        .map(|rd| rd.flatten().fold(0u64, |acc, e| acc.wrapping_add(entry_hash(&e))))
        .unwrap_or(0);
    for f in extra {
        let mut h = DefaultHasher::new();
        f.hash(&mut h);
        fs::metadata(f).ok().and_then(|m| m.modified().ok()).hash(&mut h);
        sum = sum.wrapping_add(h.finish());
    }
    sum
}

fn entry_hash(e: &fs::DirEntry) -> u64 {
    let mut h = DefaultHasher::new();
    e.path().hash(&mut h);
    if let Ok(m) = e.metadata() {
        m.len().hash(&mut h);
        m.modified().ok().hash(&mut h);
    }
    h.finish()
}
