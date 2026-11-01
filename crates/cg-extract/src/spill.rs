//! Release accumulated kernel buffers to immutable temporary mappings when their resident
//! share reaches the extraction budget. One file backs a whole chunk, avoiding a descriptor
//! per source file. Facts remain resident; this is a degradation path, not an RSS hard limit.

use crate::rows::{FileRows, TableSlices};
use crate::FileBatch;
use memmap2::Mmap;
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use tempfile::NamedTempFile;

struct Backing {
    // Drop the mapping before deleting the file (also required on Windows).
    map: Mmap,
    _file: NamedTempFile,
}

pub(crate) struct SpilledTables {
    backing: Arc<Backing>,
    ranges: [Range<usize>; 5],
}

impl SpilledTables {
    pub(crate) fn rows(&self) -> FileRows<'_> {
        let bytes = &self.backing.map;
        let [meta, nodes, edges, refs, arena] = self.ranges.each_ref().map(|r| &bytes[r.clone()]);
        FileRows::from_slices(TableSlices { meta, nodes, edges, refs, arena })
    }
}

/// Heap capacity of the kernel buffers; includes spare capacity that will be released.
pub fn resident_bytes(batch: &FileBatch) -> u64 {
    let e = &batch.emit;
    [&e.meta, &e.nodes, &e.edges, &e.refs, &e.arena].iter().map(|v| v.capacity() as u64).sum()
}

/// Spill all resident tables in `batches` as one immutable chunk. No buffers are released
/// until the write and map have both succeeded, so an I/O failure leaves the input intact.
pub fn spill<'a>(batches: impl IntoIterator<Item = &'a mut FileBatch>, dir: &Path) -> io::Result<u64> {
    let batches: Vec<_> = batches.into_iter().filter(|b| b.spilled.is_none()).collect();
    if batches.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::Builder::new().prefix("extract-").suffix(".tmp").tempfile_in(dir)?;
    let mut ranges = Vec::with_capacity(batches.len());
    let mut offset = 0usize;
    let mut released = 0u64;
    for b in &batches {
        released += resident_bytes(b);
        let e = &b.emit;
        let mut parts: [Range<usize>; 5] = std::array::from_fn(|_| 0..0);
        for (slot, bytes) in parts.iter_mut().zip([&e.meta, &e.nodes, &e.edges, &e.refs, &e.arena]) {
            file.write_all(bytes)?;
            *slot = offset..offset + bytes.len();
            offset += bytes.len();
        }
        ranges.push(parts);
    }
    file.flush()?;
    // SAFETY: the private tempfile is never modified or truncated after this mapping and
    // its owner lives as long as every slice into it. No source file is mapped here.
    let map = unsafe { Mmap::map(file.as_file())? };
    let backing = Arc::new(Backing { map, _file: file });
    for (b, ranges) in batches.into_iter().zip(ranges) {
        b.spilled = Some(SpilledTables { backing: backing.clone(), ranges });
        b.emit.meta = Vec::new();
        b.emit.nodes = Vec::new();
        b.emit.edges = Vec::new();
        b.emit.refs = Vec::new();
        b.emit.arena = Vec::new();
    }
    Ok(released)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{extract_bytes, ExtractConfig};
    use cg_core::Lang;

    #[test]
    fn spill_preserves_every_abi_buffer_and_shares_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ExtractConfig::new(dir.path());
        let mut batches: Vec<_> =
            ["a.ts", "b.ts"].iter().map(|rel| extract_bytes(rel, b"export function foo() { return foo(); }", 0, Lang::TypeScript, &cfg)).collect();
        let before: Vec<_> = batches
            .iter()
            .map(|b| {
                let e = b.rows().emit;
                [e.meta, e.nodes, e.edges, e.refs, e.arena].map(<[u8]>::to_vec)
            })
            .collect();
        assert!(spill(&mut batches, dir.path()).unwrap() > 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        for (b, expected) in batches.iter().zip(before) {
            assert_eq!(resident_bytes(b), 0);
            let e = b.rows().emit;
            assert_eq!([e.meta, e.nodes, e.edges, e.refs, e.arena].map(<[u8]>::to_vec), expected);
            assert!(b.rows().node_count() > 1);
        }
        assert_eq!(spill(&mut batches, dir.path()).unwrap(), 0);
        drop(batches);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
