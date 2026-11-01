//! An opened project: the snapshot (kept alive together with its shared locks) and the graph
//! built over it.

use crate::graph::Graph;
use crate::tools::StatusInfo;
use cg_core::{brand, CgError, Result};
use cg_store::{Snapshot, Store};
use std::path::Path;

pub struct Project {
    pub store: Store,
    pub snap: Snapshot,
    pub graph: Graph,
}

impl Project {
    /// Find the nearest `.cgr/` at or above `start` and open its current snapshot.
    pub fn locate(start: &Path) -> Result<Project> {
        let store = Store::locate(start).ok_or_else(|| CgError::NoIndex(start.to_path_buf()))?;
        Project::open(store)
    }

    pub fn open(store: Store) -> Result<Project> {
        let snap = store.open_snapshot()?;
        let root = store.root.clone();
        let graph = Graph::new(snap.view.clone(), root);
        Ok(Project { store, snap, graph })
    }

    pub fn root(&self) -> &Path {
        &self.store.root
    }

    pub fn generation(&self) -> u64 {
        self.snap.manifest.generation
    }

    pub fn status_info(&self) -> StatusInfo {
        let m = &self.snap.manifest;
        let bytes = self.snap.view.base.file_len + self.snap.view.delta.as_ref().map_or(0, |d| d.seg.file_len);
        StatusInfo {
            files: self.graph.indexed_files(),
            nodes: m.n_nodes as usize,
            edges: m.n_edges as usize,
            generation: m.generation,
            bytes,
            built_at: format_iso_ms(m.updated_ms.max(m.created_ms)),
            engine_version: m.engine_version,
            engine_outdated: m.engine_version != brand::ENGINE_VERSION,
            changes: None,
            notes: Vec::new(),
        }
    }
}

/// `2026-10-31T12:34:56Z` from milliseconds since the epoch (UTC, no external date crate).
pub fn format_iso_ms(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

pub fn display_path(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_format() {
        assert_eq!(format_iso_ms(0), "");
        assert_eq!(format_iso_ms(1_000_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_iso_ms(1_788_179_696_000), "2026-08-31T12:34:56Z");
    }
}
