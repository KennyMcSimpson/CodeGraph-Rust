//! Change detection (design doc 9.2): compare the files found on disk with the files of a view
//! by (mtime, size) and, when those differ, by content hash. A file whose content hash is
//! unchanged only had its timestamp touched.

use cg_core::{hash, FileId};
use cg_scan::FileEntry;
use cg_store::View;
use rayon::prelude::*;
use std::path::Path;

#[derive(Default, Debug)]
pub struct Diff {
    /// Existing files with new content.
    pub replaced: Vec<(FileId, FileEntry)>,
    /// Files not in the view.
    pub added: Vec<FileEntry>,
    /// Files in the view that are gone (or no longer indexable).
    pub removed: Vec<FileId>,
    /// Same content, different mtime / size: only the record is refreshed.
    pub touched: Vec<(FileId, FileEntry)>,
    pub unchanged: usize,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.replaced.is_empty() && self.added.is_empty() && self.removed.is_empty() && self.touched.is_empty()
    }
    pub fn changed(&self) -> usize {
        self.replaced.len() + self.added.len() + self.removed.len() + self.touched.len()
    }
}

enum Class {
    Same,
    Touched,
    Replaced,
}

/// Compare `scanned` (sorted by path) with the files of `view`.
pub fn diff_view(view: &View, root: &Path, scanned: &[FileEntry]) -> Diff {
    let mut table: Vec<(&str, FileId)> = view.files().map(|f| (view.file(f).path(), f)).collect();
    table.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut d = Diff::default();
    let mut both: Vec<(FileId, &FileEntry)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < table.len() || j < scanned.len() {
        let ord = match (table.get(i), scanned.get(j)) {
            (Some(a), Some(b)) => a.0.as_bytes().cmp(b.rel.as_bytes()),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => break,
        };
        match ord {
            std::cmp::Ordering::Less => {
                d.removed.push(table[i].1);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                d.added.push(scanned[j].clone());
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                both.push((table[i].1, &scanned[j]));
                i += 1;
                j += 1;
            }
        }
    }
    let classes: Vec<Class> = both
        .par_iter()
        .map(|(f, e)| {
            let rec = view.file(*f).rec;
            if rec.mtime_ns == e.mtime_ns && rec.size as u64 == e.size {
                return Class::Same;
            }
            match std::fs::read(root.join(&e.rel)) {
                Ok(bytes) if hash::hash128(&bytes) == rec.hash => Class::Touched,
                _ => Class::Replaced,
            }
        })
        .collect();
    for ((f, e), c) in both.into_iter().zip(classes) {
        match c {
            Class::Same => d.unchanged += 1,
            Class::Touched => d.touched.push((f, e.clone())),
            Class::Replaced => d.replaced.push((f, e.clone())),
        }
    }
    d
}
