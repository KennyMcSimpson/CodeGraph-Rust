//! cg-scan: which files are in the index, and what we know about them from their paths
//! (design doc 4.5). Content-level work (hashing, extraction) lives in `cg-extract`.

pub mod generated;
pub mod lang;
pub mod rules;
pub mod testpath;
mod walk;

pub use rules::IgnoreRules;
pub use walk::{mtime_ns, rel_string, scan, stat_entry, FileEntry, ScanOptions, ScanResult, MAX_SOURCE_FILE_SIZE_BYTES};
