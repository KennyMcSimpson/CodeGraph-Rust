//! File watching (design doc 9.1). One process per project holds `watch.lock` and runs the
//! watcher; the others poll the lock and take over within a few seconds if the holder dies.
//! Events are filtered by the indexing rules, debounced adaptively (300 ms of quiet for up to two
//! files, 2 s for more), and answered with one sync. A change burst of more than 500 files is just
//! a sync as well: sync always compares the whole tree against the index.

use crate::sync::{sync, SyncOptions};
use cg_core::config::ProjectConfig;
use cg_core::brand;
use cg_scan::IgnoreRules;
use cg_store::Store;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const QUIET_SMALL: Duration = Duration::from_millis(300);
const QUIET_LARGE: Duration = Duration::from_millis(2000);
const ELECTION_POLL: Duration = Duration::from_secs(2);
const FALLBACK_SCAN: Duration = Duration::from_secs(60);

pub struct WatchHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatchHandle {
    /// Ask the watcher to stop and wait (up to ~1 s) for it.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_millis(1500);
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn is_config_name(name: &str) -> bool {
    matches!(
        name,
        ".gitignore" | "cgr.json" | "codegraph.json" | "package.json" | "go.mod" | "Cargo.toml" | "pyproject.toml" | "setup.py" | "setup.cfg" | "pom.xml" | "build.gradle" | "build.gradle.kts" | "jsconfig.json"
    ) || name.starts_with("tsconfig")
        || (name.starts_with("requirements") && name.ends_with(".txt"))
}

struct Filter {
    root: PathBuf,
    rules: IgnoreRules,
}

impl Filter {
    fn new(root: &Path) -> Filter {
        let config = ProjectConfig::load(root);
        Filter { root: root.to_path_buf(), rules: IgnoreRules::new(root, &config) }
    }

    /// Does an event at `p` possibly change what the index says?
    fn relevant(&self, p: &Path) -> bool {
        let Ok(rel) = p.strip_prefix(&self.root) else { return false };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() || rel == brand::DATA_DIR || rel.starts_with(&format!("{}/", brand::DATA_DIR)) || rel == ".git" || rel.starts_with(".git/") {
            return false;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        if is_config_name(name) {
            return true;
        }
        let is_dir = p.is_dir();
        !self.rules.is_ignored(&rel, is_dir)
    }
}

/// Start a watcher thread for the project at `root`. Returns immediately.
pub fn spawn_watcher(root: PathBuf, opts: SyncOptions) -> WatchHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("cgr-watch".into())
        .spawn(move || watch_loop(root, opts, s2))
        .ok();
    WatchHandle { stop, thread }
}

fn sleep_checking(stop: &AtomicBool, total: Duration) {
    let end = Instant::now() + total;
    while !stop.load(Ordering::SeqCst) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn watch_loop(root: PathBuf, opts: SyncOptions, stop: Arc<AtomicBool>) {
    let store = Store::at(&root);
    while !stop.load(Ordering::SeqCst) {
        let guard = match store.try_watch_lock() {
            Ok(Some(g)) => g,
            _ => {
                // another process watches; take over when it goes away
                sleep_checking(&stop, ELECTION_POLL);
                continue;
            }
        };
        tracing::debug!("watching {}", root.display());
        // catch up with whatever changed while nobody watched
        run_sync(&root, &opts);
        run_watch(&root, &opts, &stop);
        drop(guard);
    }
}

fn run_sync(root: &Path, opts: &SyncOptions) {
    match sync(root, opts) {
        Ok(r) if !r.up_to_date => tracing::info!("{}", r.summary()),
        Ok(_) => {}
        Err(e) => tracing::warn!("sync failed: {e}"),
    }
}

fn run_watch(root: &Path, opts: &SyncOptions, stop: &AtomicBool) {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    });
    let mut watcher = match watcher {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::warn!("file watching unavailable ({e}); falling back to periodic scans");
            None
        }
    };
    let mut degraded = watcher.is_none();
    if let Some(w) = watcher.as_mut() {
        if let Err(e) = w.watch(root, RecursiveMode::Recursive) {
            tracing::warn!("cannot watch {} ({e}); falling back to periodic scans", root.display());
            degraded = true;
        }
    }
    let mut filter = Filter::new(root);
    let mut dirty: HashSet<PathBuf> = HashSet::new();
    let mut last_event = Instant::now();
    let mut last_scan = Instant::now();
    let store = Store::at(root);
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(ev)) => {
                if matches!(ev.kind, EventKind::Access(_)) {
                    continue;
                }
                for p in ev.paths {
                    if p.file_name().and_then(|n| n.to_str()).map_or(false, is_config_name) {
                        filter = Filter::new(root);
                    }
                    if filter.relevant(&p) {
                        dirty.insert(p);
                        last_event = Instant::now();
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("watcher error: {e}");
                degraded = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !dirty.is_empty() {
            let quiet = if dirty.len() <= 2 { QUIET_SMALL } else { QUIET_LARGE };
            if last_event.elapsed() >= quiet {
                dirty.clear();
                run_sync(root, opts);
                last_scan = Instant::now();
            }
        } else if degraded && last_scan.elapsed() >= FALLBACK_SCAN.min(Duration::from_secs(5)) {
            run_sync(root, opts);
            last_scan = Instant::now();
        } else if last_scan.elapsed() >= FALLBACK_SCAN * 5 {
            // slow safety net: events can be dropped (inotify queue overflow, network drives)
            run_sync(root, opts);
            last_scan = Instant::now();
        }
        // the index may have been removed (`cgr uninit`): stop watching
        if store.read_manifest().ok().flatten().is_none() {
            break;
        }
    }
}
