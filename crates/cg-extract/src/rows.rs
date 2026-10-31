//! Zero-copy views over the kernel's flat tables (ABI v2, see `cg-kernel/src/buffers.rs`).
//! Decoding rules are those of upstream `decode.ts`: `0xFFFF_FFFF` is "absent", lists are
//! NUL-joined, booleans are (present, value) bit pairs, columns are UTF-16.

use cg_core::{EdgeKind, NodeKind, Provenance, RefKind, Visibility, NONE};
use cg_kernel::{EmitOut, EDGE_ROW_SIZE, META_SIZE, NODE_ROW_SIZE, REF_FLAG_FILE_PATH, REF_ROW_SIZE};

#[inline]
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
#[inline]
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// One file's tables.
#[derive(Clone, Copy)]
pub struct FileRows<'a> {
    pub emit: &'a EmitOut,
}

#[inline]
fn opt_u32(v: u32) -> Option<u32> {
    (v != NONE).then_some(v)
}

impl<'a> FileRows<'a> {
    pub fn new(emit: &'a EmitOut) -> Self {
        debug_assert!(emit.meta.len() >= META_SIZE);
        FileRows { emit }
    }
    pub fn node_count(&self) -> usize {
        u32_at(&self.emit.meta, 4) as usize
    }
    pub fn edge_count(&self) -> usize {
        u32_at(&self.emit.meta, 8) as usize
    }
    pub fn ref_count(&self) -> usize {
        u32_at(&self.emit.meta, 12) as usize
    }
    /// JSON array of extraction errors, if any were recorded.
    pub fn errors_json(&self) -> Option<&'a str> {
        let off = u32_at(&self.emit.meta, 20);
        if off == NONE {
            return None;
        }
        let len = u32_at(&self.emit.meta, 24) as usize;
        self.s(off, len as u32)
    }

    #[inline]
    fn s(&self, off: u32, len: u32) -> Option<&'a str> {
        if off == NONE {
            return None;
        }
        let (o, l) = (off as usize, len as usize);
        std::str::from_utf8(self.emit.arena.get(o..o + l)?).ok()
    }

    pub fn node(&self, i: usize) -> NodeView<'a> {
        let b = &self.emit.nodes[i * NODE_ROW_SIZE..(i + 1) * NODE_ROW_SIZE];
        NodeView { rows: *self, b }
    }
    pub fn edge(&self, i: usize) -> EdgeView<'a> {
        let b = &self.emit.edges[i * EDGE_ROW_SIZE..(i + 1) * EDGE_ROW_SIZE];
        EdgeView { rows: *self, b }
    }
    pub fn reference(&self, i: usize) -> RefView<'a> {
        let b = &self.emit.refs[i * REF_ROW_SIZE..(i + 1) * REF_ROW_SIZE];
        RefView { rows: *self, b }
    }
    pub fn nodes(&self) -> impl Iterator<Item = NodeView<'a>> + '_ {
        (0..self.node_count()).map(move |i| self.node(i))
    }
    pub fn edges(&self) -> impl Iterator<Item = EdgeView<'a>> + '_ {
        (0..self.edge_count()).map(move |i| self.edge(i))
    }
    pub fn refs(&self) -> impl Iterator<Item = RefView<'a>> + '_ {
        (0..self.ref_count()).map(move |i| self.reference(i))
    }
}

#[derive(Clone, Copy)]
pub struct NodeView<'a> {
    rows: FileRows<'a>,
    b: &'a [u8],
}

impl<'a> NodeView<'a> {
    fn st(&self, at: usize) -> Option<&'a str> {
        self.rows.s(u32_at(self.b, at), u32_at(self.b, at + 4))
    }
    pub fn kind(&self) -> NodeKind {
        NodeKind::from_u8(self.b[0]).unwrap_or(NodeKind::Variable)
    }
    pub fn visibility(&self) -> Visibility {
        Visibility::from_u8(self.b[1]).unwrap_or(Visibility::None)
    }
    pub fn raw_flags(&self) -> u16 {
        u16_at(self.b, 2)
    }
    fn flag(&self, pair: u16) -> Option<bool> {
        let f = self.raw_flags();
        if f & (1 << (pair * 2)) == 0 {
            None
        } else {
            Some(f & (1 << (pair * 2 + 1)) != 0)
        }
    }
    pub fn is_exported(&self) -> Option<bool> {
        self.flag(0)
    }
    pub fn is_async(&self) -> Option<bool> {
        self.flag(1)
    }
    pub fn is_static(&self) -> Option<bool> {
        self.flag(2)
    }
    pub fn is_abstract(&self) -> Option<bool> {
        self.flag(3)
    }
    pub fn start_line(&self) -> u32 {
        u32_at(self.b, 4)
    }
    pub fn end_line(&self) -> u32 {
        u32_at(self.b, 8)
    }
    pub fn start_col(&self) -> u32 {
        u32_at(self.b, 12)
    }
    pub fn end_col(&self) -> u32 {
        u32_at(self.b, 16)
    }
    pub fn name(&self) -> &'a str {
        self.st(20).unwrap_or("")
    }
    pub fn qname(&self) -> &'a str {
        self.st(28).unwrap_or("")
    }
    /// Kernel-computed stable id: `kind:hash32[:col]` or `file:<path>`.
    pub fn id(&self) -> &'a str {
        self.st(36).unwrap_or("")
    }
    pub fn docstring(&self) -> Option<&'a str> {
        self.st(44)
    }
    pub fn signature(&self) -> Option<&'a str> {
        self.st(52)
    }
    /// NUL-joined list.
    pub fn decorators(&self) -> Option<&'a str> {
        self.st(60)
    }
    pub fn type_params(&self) -> Option<&'a str> {
        self.st(68)
    }
    pub fn return_type(&self) -> Option<&'a str> {
        self.st(76)
    }
    pub fn extra_json(&self) -> Option<&'a str> {
        self.st(84)
    }
}

#[derive(Clone, Copy)]
pub struct EdgeView<'a> {
    rows: FileRows<'a>,
    b: &'a [u8],
}

impl<'a> EdgeView<'a> {
    fn st(&self, at: usize) -> Option<&'a str> {
        self.rows.s(u32_at(self.b, at), u32_at(self.b, at + 4))
    }
    /// Source node row, or `None` when the edge names its source by id string.
    pub fn source_row(&self) -> Option<u32> {
        opt_u32(u32_at(self.b, 0))
    }
    pub fn target_row(&self) -> Option<u32> {
        opt_u32(u32_at(self.b, 4))
    }
    pub fn kind(&self) -> EdgeKind {
        EdgeKind::from_u8(self.b[8]).unwrap_or(EdgeKind::References)
    }
    pub fn provenance(&self) -> Provenance {
        Provenance::from_u8(self.b[9]).unwrap_or(Provenance::None)
    }
    pub fn line(&self) -> Option<u32> {
        opt_u32(u32_at(self.b, 12))
    }
    pub fn col(&self) -> Option<u32> {
        opt_u32(u32_at(self.b, 16))
    }
    pub fn metadata_json(&self) -> Option<&'a str> {
        self.st(20)
    }
    pub fn source_id(&self) -> Option<&'a str> {
        self.st(28)
    }
    pub fn target_id(&self) -> Option<&'a str> {
        self.st(36)
    }
}

#[derive(Clone, Copy)]
pub struct RefView<'a> {
    rows: FileRows<'a>,
    b: &'a [u8],
}

impl<'a> RefView<'a> {
    fn st(&self, at: usize) -> Option<&'a str> {
        self.rows.s(u32_at(self.b, at), u32_at(self.b, at + 4))
    }
    pub fn from_row(&self) -> Option<u32> {
        opt_u32(u32_at(self.b, 0))
    }
    pub fn kind(&self) -> RefKind {
        RefKind::from_wire(self.b[4]).unwrap_or(RefKind::Edge(EdgeKind::References))
    }
    pub fn carries_file_path(&self) -> bool {
        self.b[5] & REF_FLAG_FILE_PATH != 0
    }
    pub fn line(&self) -> u32 {
        u32_at(self.b, 8)
    }
    pub fn col(&self) -> u32 {
        u32_at(self.b, 12)
    }
    pub fn name(&self) -> &'a str {
        self.st(16).unwrap_or("")
    }
    /// NUL-joined candidate qualified names.
    pub fn candidates(&self) -> Option<&'a str> {
        self.st(24)
    }
    pub fn from_id(&self) -> Option<&'a str> {
        self.st(32)
    }
}
