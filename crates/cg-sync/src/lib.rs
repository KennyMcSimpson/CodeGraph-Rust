//! cg-sync: keeping an index current (design doc 9): change detection, the incremental
//! algorithm, delta layers, and the file watcher.

pub mod detect;
pub mod sync;

pub use detect::{diff_view, Diff};
pub use sync::{sync, sync_locked, SyncOptions, SyncReport};
pub mod watch;
pub use watch::{spawn_watcher, WatchHandle};

use cg_core::Result;
use cg_scan::{scan, ScanOptions};
use cg_store::{Store, View};
use std::path::Path;

/// Files added / modified / removed on disk since the snapshot was written (design doc 9.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
}

impl Changes {
    pub fn total(&self) -> usize {
        self.added + self.modified + self.removed
    }
    pub fn describe(&self) -> String {
        format!("{} added, {} modified, {} removed", self.added, self.modified, self.removed)
    }
}

/// Compare the tree under `root` with `view` (touched-only files do not count as changes).
pub fn measure_changes(root: &Path, view: &View) -> Result<Changes> {
    let scanned = scan(&ScanOptions::new(root));
    let d = diff_view(view, root, &scanned.files);
    Ok(Changes { added: d.added.len(), modified: d.replaced.len(), removed: d.removed.len() })
}

/// Does `root` have an index?
pub fn has_index(root: &Path) -> bool {
    Store::at(root).read_manifest().ok().flatten().is_some()
}
