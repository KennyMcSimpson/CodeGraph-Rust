//! Fixed-size on-disk records (design doc 5.3 and 16.3). All `#[repr(C)]`, no padding,
//! little-endian hosts only; sizes are asserted at compile time.

use bytemuck::{Pod, Zeroable};
use cg_core::{FileId, ListId, NodeIdx, ScopeId, StrId};

macro_rules! rec {
    ($(#[$m:meta])* $name:ident, $size:expr, { $($(#[$fm:meta])* $f:ident : $t:ty),* $(,)? }) => {
        $(#[$m])*
        #[repr(C)]
        #[derive(Pod, Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name { $($(#[$fm])* pub $f: $t),* }
        const _: () = assert!(std::mem::size_of::<$name>() == $size);
    };
}

rec!(
    /// One indexed file. Files are sorted by path bytes; `FileId` is the index.
    FileRec, 64, {
    path: StrId,
    lang: u8,
    flags: u8,
    _p: u16,
    size: u32,
    node_start: NodeIdx,
    node_count: u32,
    ref_start: u32,
    ref_count: u32,
    /// Text-pool id of a JSON array of extraction errors, or NONE.
    errors: StrId,
    mtime_ns: i64,
    hash: [u8; 16],
    _r: [u8; 8],
});

rec!(
    /// The part of a node that traversal and search touch.
    NodeHot, 32, {
    file: FileId,
    kind: u8,
    vis: u8,
    /// Kernel bool pairs: (exported, async, static, abstract) x (present, value).
    flags: u16,
    name: StrId,
    qname: StrId,
    start_line: u32,
    end_line: u32,
    /// Source of the `contains` edge; the file node for top-level symbols; NONE for file nodes.
    parent: NodeIdx,
    _p: u32,
});

rec!(
    NodeCold, 56, {
    start_col: u32,
    end_col: u32,
    signature: StrId,
    docstring: StrId,
    return_type: StrId,
    decorators: ListId,
    type_params: ListId,
    extra: StrId,
    /// Stable external id: first 16 bytes of sha256, as the upstream id's hex part.
    digest: [u8; 16],
    /// UTF-16 column suffix of a colliding id, or NONE.
    col_suffix: u32,
    _p: u32,
});

rec!(
    /// Where in source an edge was written.
    EdgeSite, 8, { line: u32, col: u32 }
);

rec!(
    /// An extraction-time reference and, after resolution, its outcome.
    RefRec, 32, {
    from: NodeIdx,
    name: StrId,
    kind: u8,
    flags: u8,
    status: u8,
    by: u8,
    line: u32,
    col: u32,
    candidates: ListId,
    /// Resolved target; for `Ambiguous` the candidate count.
    target: NodeIdx,
    /// Last name segment / `module:<spec>` key used by incremental sync.
    tail: StrId,
});

rec!(
    ImportRec, 32, {
    file: FileId,
    local: StrId,
    imported: StrId,
    spec: StrId,
    kind: u8,
    type_only: u8,
    _p: u16,
    scope: ScopeId,
    line: u32,
    /// Resolved module file (filled in resolution round 0); NONE = outside the project.
    target: FileId,
});

rec!(
    ExportRec, 28, {
    file: FileId,
    name: StrId,
    /// NONE means "re-export".
    node: NodeIdx,
    from_spec: StrId,
    from_name: StrId,
    star: u8,
    _p: [u8; 3],
    /// File the re-export points to once resolved (round 0), else NONE.
    target: FileId,
});

rec!(
    ScopeRec, 32, {
    file: FileId,
    parent: ScopeId,
    kind: u8,
    _p: [u8; 3],
    _p2: u32,
    start: u64,
    end: u64,
});

rec!(
    BindingRec, 28, {
    scope: ScopeId,
    name: StrId,
    kind: u8,
    _p: [u8; 3],
    decl_type: StrId,
    init_ctor: StrId,
    line: u32,
    node: NodeIdx,
});

rec!(
    /// Per-file ranges into the facts partitions.
    FileFacts, 40, {
    imp_start: u32,
    imp_count: u32,
    exp_start: u32,
    exp_count: u32,
    scope_start: u32,
    scope_count: u32,
    bind_start: u32,
    bind_count: u32,
    site_start: u32,
    site_count: u32,
});

rec!(
    /// A synthesis "site" (design doc 7.1), grouped by file.
    SiteRec, 32, {
    key_kind: u8,
    role: u8,
    pass: u16,
    key_a: u32,
    key_b: u32,
    node: NodeIdx,
    line: u32,
    payload: StrId,
    _p: [u8; 8],
});

rec!(
    /// Sparse edge metadata: (edge index, text-pool id of a JSON object).
    EdgeMetaRec, 8, { eid: u32, text: StrId }
);

rec!(
    /// Token posting for BM25: which node, in which fields, how often.
    PostEntry, 8, { node: u32, mask: u8, tf: u8, _p: u16 }
);

rec!(
    FileStat, 24, {
    intra_edges: u32,
    out_edges: u32,
    in_edges: u32,
    symbols: u32,
    flags: u32,
    _p: u32,
});

rec!(
    /// (basename or stem in the names pool, file)
    FileNameRec, 8, { name: StrId, file: FileId }
);

rec!(
    /// (tail key, ref index), sorted by tail then index.
    TailRec, 8, { tail: StrId, ref_idx: u32 }
);

/// `RefRec::status` values.
pub mod status {
    pub const PENDING: u8 = 0;
    pub const RESOLVED: u8 = 1;
    pub const NO_CANDIDATE: u8 = 2;
    pub const AMBIGUOUS: u8 = 3;
    pub const EXTERNAL: u8 = 4;
    pub const LOCAL_BINDING: u8 = 5;
    pub const BUILTIN: u8 = 6;

    pub fn name(s: u8) -> &'static str {
        match s {
            PENDING => "Pending",
            RESOLVED => "Resolved",
            NO_CANDIDATE => "NoCandidate",
            AMBIGUOUS => "Ambiguous",
            EXTERNAL => "External",
            LOCAL_BINDING => "LocalBinding",
            BUILTIN => "Builtin",
            _ => "?",
        }
    }
}

/// `out_kind` byte layout (design doc 5.3): low 4 bits edge kind, bits 4-5 provenance,
/// bits 6-7 edge class.
pub mod outkind {
    use cg_core::{EdgeKind, Provenance};
    pub const CLASS_EXTRACT: u8 = 0;
    pub const CLASS_RESOLVE: u8 = 1;
    pub const CLASS_SYNTH: u8 = 2;
    pub const CLASS_FRAMEWORK: u8 = 3;

    #[inline]
    pub fn pack(kind: EdgeKind, prov: Provenance, class: u8) -> u8 {
        (kind as u8) | ((prov as u8) << 4) | (class << 6)
    }
    #[inline]
    pub fn kind(b: u8) -> EdgeKind {
        EdgeKind::from_u8(b & 0x0F).unwrap_or(EdgeKind::References)
    }
    #[inline]
    pub fn provenance(b: u8) -> Provenance {
        Provenance::from_u8((b >> 4) & 3).unwrap_or(Provenance::None)
    }
    #[inline]
    pub fn class(b: u8) -> u8 {
        b >> 6
    }
}
