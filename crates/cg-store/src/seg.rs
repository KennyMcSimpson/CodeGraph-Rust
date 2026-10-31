//! Segment file: 64-byte header, partition directory, 8-byte aligned partitions (design doc
//! 5.1). A segment is written once, in one sequential pass, and never modified.

use crate::part::Part;
use crate::records::*;
use crate::strpool::StrPool;
use cg_core::{hash, CgError, Result};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

pub const MAGIC: [u8; 4] = *b"CGR\0";
const HEADER_SIZE: usize = 64;
const DIR_ENTRY_SIZE: usize = 40;

#[derive(Clone, Copy, Debug)]
struct DirEntry {
    id: u32,
    elem: u32,
    offset: u64,
    bytes: u64,
    count: u64,
    checksum: u64,
}

macro_rules! seg_parts {
    ($($field:ident : $ty:ty = $id:expr),* $(,)?) => {
        /// Every partition of a segment. Ids are part of the file format: never renumber.
        #[derive(Clone, Default, Debug)]
        pub struct SegParts { $(pub $field: Part<$ty>),* }

        impl SegParts {
            fn entries(&self) -> Vec<(u32, u32, &[u8], u64)> {
                vec![$(($id, std::mem::size_of::<$ty>() as u32, self.$field.as_bytes(), self.$field.len() as u64)),*]
            }

            fn read(map: &Arc<Mmap>, dir: &HashMap<u32, DirEntry>) -> Result<SegParts> {
                Ok(SegParts { $($field: match dir.get(&$id) {
                    Some(e) => {
                        if e.elem as usize != std::mem::size_of::<$ty>() {
                            return Err(CgError::corrupt(format!(
                                "partition {} has element size {}, expected {}", $id, e.elem, std::mem::size_of::<$ty>())));
                        }
                        Part::from_map(map, e.offset as usize, e.bytes as usize)?
                    }
                    None => Part::default(),
                }),*})
            }
        }
    };
}

seg_parts! {
    meta: u8 = 1,
    files: FileRec = 2,
    nodes_hot: NodeHot = 3,
    nodes_cold: NodeCold = 4,
    lists_off: u32 = 5,
    lists: u32 = 6,
    names_off: u32 = 7,
    names_data: u8 = 8,
    paths_off: u32 = 9,
    paths_data: u8 = 10,
    text_off: u32 = 11,
    text_data: u8 = 12,
    out_off: u32 = 13,
    out_dst: u32 = 14,
    out_kind: u8 = 15,
    out_site: EdgeSite = 16,
    out_res: u16 = 17,
    out_ref: u32 = 18,
    in_off: u32 = 19,
    in_src: u32 = 20,
    in_eid: u32 = 21,
    edge_meta: EdgeMetaRec = 22,
    names_fst: u8 = 23,
    name_post_off: u32 = 24,
    name_post: u32 = 25,
    qname_post_off: u32 = 26,
    qname_post: u32 = 27,
    lower_fst: u8 = 28,
    lower_post_off: u32 = 29,
    lower_post: u32 = 30,
    segs_fst: u8 = 31,
    seg_post_off: u32 = 32,
    seg_post: u32 = 33,
    tri_keys: u32 = 34,
    tri_off: u32 = 35,
    tri_post: u32 = 36,
    tokens_fst: u8 = 37,
    token_post_off: u32 = 38,
    token_post: PostEntry = 39,
    node_norm: u16 = 40,
    filestats: FileStat = 41,
    refs: RefRec = 42,
    imports: ImportRec = 43,
    exports: ExportRec = 44,
    scopes: ScopeRec = 45,
    bindings: BindingRec = 46,
    file_facts: FileFacts = 47,
    sites: SiteRec = 48,
    config_digests: u8 = 49,
    fname_idx: FileNameRec = 50,
    tail_idx: TailRec = 51,
    // ---- delta layers (design doc 5.6) ----
    dead_nodes: u64 = 60,
    emeta_off: u32 = 62,
    emeta_data: u8 = 63,
    // Delta only: sorted source nodes whose out-edges live in this layer.
    d_src_nodes: u32 = 64,
    // Delta only: sorted destination nodes with in-edges in this layer.
    d_dst_nodes: u32 = 65,
    // delta_files: per replaced/added/removed file, (FileId in the base space or NONE, state).
    delta_files: u32 = 61,
}

/// Small descriptive block stored as JSON in partition 1. No timestamps: a segment is a pure
/// function of its inputs (design doc 5.5 step 8).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SegMeta {
    /// `base` or `delta`.
    pub kind: String,
    pub format_version: u32,
    pub engine_version: u32,
    pub kernel_abi: u32,
    pub upstream_commit: String,
    pub langs: Vec<String>,
    /// Digest of `codegraph.json` / `cgr.json` and ignore rules at build time.
    pub config_digest: u64,
    pub frameworks: Vec<String>,
    /// Project configuration files that influence resolution -> content hash.
    pub config_digests: BTreeMap<String, u64>,
    pub n_files: u32,
    pub n_nodes: u32,
    pub n_edges: u32,
    pub n_refs: u32,
    /// For a delta: the generation of the base it applies to.
    pub base_gen: u64,
}

pub struct Seg {
    pub meta: SegMeta,
    pub p: SegParts,
    pub names: StrPool,
    pub paths: StrPool,
    pub text: StrPool,
    /// Edge metadata JSON strings, in insertion order (not sorted, not deduplicated by id).
    pub emeta: StrPool,
    pub names_fst: Option<fst::Map<Part<u8>>>,
    pub lower_fst: Option<fst::Map<Part<u8>>>,
    pub segs_fst: Option<fst::Map<Part<u8>>>,
    pub tokens_fst: Option<fst::Map<Part<u8>>>,
    dir: Vec<DirEntry>,
    pub file_len: u64,
}

fn opt_fst(p: &Part<u8>) -> Option<fst::Map<Part<u8>>> {
    if p.is_empty() {
        None
    } else {
        fst::Map::new(p.clone()).ok()
    }
}

impl Seg {
    /// Assemble a segment from parts (pools and FSTs are derived from the parts).
    pub fn from_parts(meta: SegMeta, p: SegParts) -> Seg {
        Seg {
            meta,
            names: StrPool { off: p.names_off.clone(), data: p.names_data.clone() },
            paths: StrPool { off: p.paths_off.clone(), data: p.paths_data.clone() },
            text: StrPool { off: p.text_off.clone(), data: p.text_data.clone() },
            emeta: StrPool { off: p.emeta_off.clone(), data: p.emeta_data.clone() },
            names_fst: opt_fst(&p.names_fst),
            lower_fst: opt_fst(&p.lower_fst),
            segs_fst: opt_fst(&p.segs_fst),
            tokens_fst: opt_fst(&p.tokens_fst),
            p,
            dir: Vec::new(),
            file_len: 0,
        }
    }

    /// Re-derive pools/FSTs after `p` was edited.
    pub fn rebuild_derived(self) -> Seg {
        let Seg { meta, p, .. } = self;
        Seg::from_parts(meta, p)
    }

    pub fn open(path: &Path) -> Result<Seg> {
        let file = std::fs::File::open(path)?;
        // SAFETY: segments are immutable once published (renamed into place); concurrent
        // truncation by another process is excluded by the writer protocol (design doc 5.7).
        let map = Arc::new(unsafe { Mmap::map(&file)? });
        Seg::from_map(map)
    }

    pub fn from_map(map: Arc<Mmap>) -> Result<Seg> {
        let b: &[u8] = &map;
        if b.len() < HEADER_SIZE || b[0..4] != MAGIC {
            return Err(CgError::corrupt("bad segment magic"));
        }
        let version = u32::from_le_bytes(b[4..8].try_into().unwrap());
        if version != cg_core::brand::FORMAT_VERSION {
            return Err(CgError::Version(format!(
                "segment format {version}, this build reads {}",
                cg_core::brand::FORMAT_VERSION
            )));
        }
        let count = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        let file_len = u64::from_le_bytes(b[16..24].try_into().unwrap());
        if file_len as usize != b.len() {
            return Err(CgError::corrupt(format!("segment length {} != recorded {file_len}", b.len())));
        }
        if HEADER_SIZE + count * DIR_ENTRY_SIZE > b.len() {
            return Err(CgError::corrupt("directory past end of file"));
        }
        let mut dir = Vec::with_capacity(count);
        let mut by_id = HashMap::with_capacity(count);
        for i in 0..count {
            let o = HEADER_SIZE + i * DIR_ENTRY_SIZE;
            let rd = |a: usize| u64::from_le_bytes(b[o + a..o + a + 8].try_into().unwrap());
            let e = DirEntry {
                id: u32::from_le_bytes(b[o..o + 4].try_into().unwrap()),
                elem: u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()),
                offset: rd(8),
                bytes: rd(16),
                count: rd(24),
                checksum: rd(32),
            };
            dir.push(e);
            by_id.insert(e.id, e);
        }
        let p = SegParts::read(&map, &by_id)?;
        let meta: SegMeta = if p.meta.is_empty() {
            SegMeta::default()
        } else {
            serde_json::from_slice(&p.meta).map_err(|e| CgError::corrupt(format!("segment meta: {e}")))?
        };
        let mut seg = Seg::from_parts(meta, p);
        seg.dir = dir;
        seg.file_len = file_len;
        Ok(seg)
    }

    /// Recompute every partition checksum (`status --verify`, delta layers at open).
    pub fn verify(&self) -> Result<()> {
        let by_id: HashMap<u32, &(u32, u32, &[u8], u64)> = HashMap::new();
        let _ = by_id;
        let entries = self.p.entries();
        for e in &self.dir {
            let Some((_, _, bytes, _)) = entries.iter().find(|(id, ..)| *id == e.id) else { continue };
            if hash::hash64(bytes) != e.checksum {
                return Err(CgError::corrupt(format!("checksum mismatch in partition {}", e.id)));
            }
        }
        Ok(())
    }

    /// Serialise to bytes (header, directory, partitions).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_into(&mut out).expect("write to Vec");
        out
    }

    pub fn write_into<W: Write>(&self, w: &mut W) -> std::io::Result<u64> {
        let mut parts = self.p.entries();
        // Partition 1 (meta) is always regenerated from `self.meta`.
        let meta_json = serde_json::to_vec(&self.meta).expect("meta json");
        for e in parts.iter_mut() {
            if e.0 == 1 {
                e.2 = &meta_json;
                e.3 = meta_json.len() as u64;
            }
        }
        let align8 = |n: usize| (n + 7) & !7;
        let dir_bytes = parts.len() * DIR_ENTRY_SIZE;
        let mut cur = align8(HEADER_SIZE + dir_bytes);
        let mut dir = Vec::with_capacity(parts.len());
        for (id, elem, bytes, count) in &parts {
            let offset = align8(cur);
            dir.push(DirEntry {
                id: *id,
                elem: *elem,
                offset: offset as u64,
                bytes: bytes.len() as u64,
                count: *count,
                checksum: hash::hash64(bytes),
            });
            cur = offset + bytes.len();
        }
        let total = align8(cur);
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&cg_core::brand::FORMAT_VERSION.to_le_bytes());
        header[8..12].copy_from_slice(&(parts.len() as u32).to_le_bytes());
        header[16..24].copy_from_slice(&(total as u64).to_le_bytes());
        w.write_all(&header)?;
        for e in &dir {
            w.write_all(&e.id.to_le_bytes())?;
            w.write_all(&e.elem.to_le_bytes())?;
            w.write_all(&e.offset.to_le_bytes())?;
            w.write_all(&e.bytes.to_le_bytes())?;
            w.write_all(&e.count.to_le_bytes())?;
            w.write_all(&e.checksum.to_le_bytes())?;
        }
        let mut pos = HEADER_SIZE + dir_bytes;
        const ZEROS: [u8; 8] = [0; 8];
        for (e, (_, _, bytes, _)) in dir.iter().zip(parts.iter()) {
            let pad = e.offset as usize - pos;
            w.write_all(&ZEROS[..pad])?;
            w.write_all(bytes)?;
            pos = e.offset as usize + bytes.len();
        }
        let tail = total - pos;
        w.write_all(&ZEROS[..tail])?;
        Ok(total as u64)
    }

    /// Write to `path` (not atomic by itself: callers write `*.tmp` and rename).
    pub fn write_file(&self, path: &Path, fsync: bool) -> Result<u64> {
        let file = std::fs::File::create(path)?;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, file);
        let n = self.write_into(&mut w)?;
        w.flush()?;
        if fsync {
            w.get_ref().sync_all()?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cg_core::{FileId, NodeIdx, StrId};

    fn sample() -> Seg {
        let mut p = SegParts::default();
        let pool = StrPool::from_sorted_unique(&["a.rs", "b.rs"]);
        p.paths_off = pool.off.clone();
        p.paths_data = pool.data.clone();
        p.files = Part::owned(vec![
            FileRec { path: StrId(0), lang: 8, size: 10, node_start: NodeIdx(0), node_count: 1, ..bytemuck::Zeroable::zeroed() },
            FileRec { path: StrId(1), lang: 8, size: 20, node_start: NodeIdx(1), node_count: 2, ..bytemuck::Zeroable::zeroed() },
        ]);
        p.out_off = Part::owned(vec![0, 1, 1, 1]);
        p.out_dst = Part::owned(vec![2]);
        let meta = SegMeta { kind: "base".into(), format_version: 1, n_files: 2, ..Default::default() };
        let _ = FileId(0);
        Seg::from_parts(meta, p)
    }

    #[test]
    fn roundtrip_and_determinism() {
        let seg = sample();
        let bytes = seg.to_bytes();
        assert_eq!(bytes, sample().to_bytes(), "serialisation must be deterministic");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.seg");
        seg.write_file(&path, false).unwrap();
        let back = Seg::open(&path).unwrap();
        assert_eq!(back.meta, seg.meta);
        assert_eq!(&back.p.files[..], &seg.p.files[..]);
        assert_eq!(&back.p.out_dst[..], &[2]);
        assert_eq!(back.paths.get(StrId(1)), "b.rs");
        back.verify().unwrap();
    }

    #[test]
    fn corruption_is_detected() {
        let seg = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.seg");
        seg.write_file(&path, false).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let n = bytes.len();
        bytes[n - 40] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let back = Seg::open(&path).unwrap();
        assert!(back.verify().is_err());
        std::fs::write(&path, &bytes[..n - 8]).unwrap();
        assert!(Seg::open(&path).is_err());
        std::fs::write(&path, b"nope").unwrap();
        assert!(Seg::open(&path).is_err());
    }
}
