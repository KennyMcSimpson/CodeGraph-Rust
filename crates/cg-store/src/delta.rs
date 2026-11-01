//! Delta layer assembly (design doc 5.6). A delta is a function of the base segment and the
//! current source tree: files whose content changed (`Replaced`) or appeared (`New`) carry
//! their nodes, references and facts; files whose references or edges changed but not their
//! content (`EdgesOnly`) carry fresh copies of both; `Removed` files are only marked. The base
//! nodes of replaced and removed files are listed in the `dead_nodes` bitset.
//!
//! Ids inside a delta are global (base length + local id); the delta's own string pools and
//! index partitions are local, as documented in `view.rs`.

use crate::build::{BuildBase, Draft, EdgeBuild};
use crate::edges;
use crate::finish::RefUpdate;
use crate::index::{self, TokenInputs};
use crate::part::Part;
use crate::records::*;
use crate::seg::Seg;
use crate::view::DeltaState;
use cg_core::{FileId, NodeIdx, NodeKind, NONE};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

/// Offsets of the base segment's id spaces, for building a delta on top of it.
pub fn base_offsets(base: &Seg) -> BuildBase {
    BuildBase {
        nodes: base.p.nodes_hot.len() as u32,
        names: base.names.len() as u32,
        paths: base.paths.len() as u32,
        text: base.text.len() as u32,
        lists: base.p.lists_off.len().saturating_sub(1) as u32,
    }
}

/// A base file whose references and out-edges are rewritten while its nodes stay.
pub struct EdgesOnlyFile {
    pub file: FileId,
    /// The file record to use (mtime / size may have been refreshed).
    pub rec: FileRec,
    /// All references of the file with their (re-)resolved results, global ids.
    pub refs: Vec<RefRec>,
}

pub struct DeltaParts {
    /// Replaced and new files, built with [`crate::build::build_draft_at`].
    pub draft: Draft,
    /// Parallel to the draft's files: `Replaced` or `New`.
    pub states: Vec<DeltaState>,
    /// Parallel to the draft's files: their global file ids.
    pub ids: Vec<FileId>,
    /// Removed base files with their base records.
    pub removed: Vec<(FileId, FileRec)>,
    pub edges_only: Vec<EdgesOnlyFile>,
    /// Outcomes of the draft's own references (parallel); empty = leave pending.
    pub ref_updates: Vec<RefUpdate>,
    /// Edges besides the draft's extraction edges: resolution, synthesis, copied base edges.
    pub edges: Vec<EdgeBuild>,
    /// Metadata strings referenced by `edges[..].meta` (index into this list).
    pub emeta: Vec<String>,
    /// Per `ImportRec` / `ExportRec` of the draft (empty = unchanged).
    pub import_targets: Vec<u32>,
    pub export_targets: Vec<u32>,
    /// Per-file synthesis sites of the draft (empty = unchanged) and their per-file ranges.
    pub sites: Vec<SiteRec>,
    pub site_ranges: Vec<(u32, u32)>,
    /// Base nodes made dead (nodes of replaced and removed files).
    pub dead_base_nodes: Vec<u32>,
    /// Totals for the manifest.
    pub total_files: u32,
    pub total_nodes: u32,
    pub total_edges: u32,
    pub base_gen: u64,
    /// Build the search indexes (the draft used for resolution skips them).
    pub indexes: bool,
}

/// Assemble the delta segment.
pub fn assemble_delta(base: &Seg, parts: DeltaParts) -> Seg {
    let DeltaParts {
        draft,
        states,
        ids,
        removed,
        edges_only,
        ref_updates,
        edges: extra_edges,
        emeta: extra_emeta,
        import_targets,
        export_targets,
        sites,
        site_ranges,
        dead_base_nodes,
        total_files,
        total_nodes,
        total_edges,
        base_gen,
        indexes,
    } = parts;
    let Draft { mut seg, edges: mut all_edges, emeta: mut emeta } = draft;
    let bb = base_offsets(base);
    let n_draft_files = seg.p.files.len();
    debug_assert_eq!(n_draft_files, states.len());
    debug_assert_eq!(n_draft_files, ids.len());

    // ---- draft references ----
    let mut refs: Vec<RefRec> = seg.p.refs.to_vec();
    if !ref_updates.is_empty() {
        assert_eq!(refs.len(), ref_updates.len(), "one update per reference of the draft");
        for (r, u) in refs.iter_mut().zip(&ref_updates) {
            r.status = u.status;
            r.by = u.by;
            r.target = NodeIdx(u.target);
        }
    }
    if !import_targets.is_empty() {
        let mut v = seg.p.imports.to_vec();
        for (r, t) in v.iter_mut().zip(&import_targets) {
            r.target = FileId(*t);
        }
        seg.p.imports = Part::owned(v);
    }
    if !export_targets.is_empty() {
        let mut v = seg.p.exports.to_vec();
        for (r, t) in v.iter_mut().zip(&export_targets) {
            r.target = FileId(*t);
        }
        seg.p.exports = Part::owned(v);
    }
    if !sites.is_empty() {
        seg.p.sites = Part::owned(sites);
        let mut ff = seg.p.file_facts.to_vec();
        for (f, (s, c)) in ff.iter_mut().zip(&site_ranges) {
            f.site_start = *s;
            f.site_count = *c;
        }
        seg.p.file_facts = Part::owned(ff);
    }

    // ---- file entries ----
    let mut files: Vec<FileRec> = seg.p.files.to_vec();
    let mut facts: Vec<FileFacts> = seg.p.file_facts.to_vec();
    let mut triples: Vec<u32> = Vec::with_capacity((n_draft_files + edges_only.len() + removed.len()) * 3);
    for i in 0..n_draft_files {
        triples.extend_from_slice(&[ids[i].0, states[i] as u32, 0]);
    }
    for eo in edges_only {
        let mut rec = eo.rec;
        rec.ref_start = refs.len() as u32;
        rec.ref_count = eo.refs.len() as u32;
        refs.extend(eo.refs);
        files.push(rec);
        facts.push(bytemuck::Zeroable::zeroed());
        triples.extend_from_slice(&[eo.file.0, DeltaState::EdgesOnly as u32, 0]);
    }
    for (id, mut rec) in removed {
        rec.ref_start = 0;
        rec.ref_count = 0;
        files.push(rec);
        facts.push(bytemuck::Zeroable::zeroed());
        triples.extend_from_slice(&[id.0, DeltaState::Removed as u32, 0]);
    }
    seg.p.files = Part::owned(files);
    seg.p.file_facts = Part::owned(facts);
    seg.p.delta_files = Part::owned(triples);
    seg.p.refs = Part::owned(refs);

    // ---- dead nodes ----
    let words = (bb.nodes as usize + 63) / 64;
    let mut dead = vec![0u64; words];
    for n in dead_base_nodes {
        dead[n as usize / 64] |= 1u64 << (n % 64);
    }
    seg.p.dead_nodes = Part::owned(dead);

    // ---- edges ----
    let meta_base = emeta.len() as u32;
    for mut e in extra_edges {
        if e.meta != NONE {
            e.meta += meta_base;
        }
        all_edges.push(e);
    }
    emeta.extend(extra_emeta);
    let mut em_off = Vec::with_capacity(emeta.len() + 1);
    let mut em_data = Vec::new();
    em_off.push(0u32);
    for m in &emeta {
        em_data.extend_from_slice(m.as_bytes());
        em_off.push(em_data.len() as u32);
    }
    seg.p.emeta_off = Part::owned(em_off);
    seg.p.emeta_data = Part::owned(em_data);
    let asm = edges::assemble_sparse(&mut all_edges);
    edges::install_sparse(&mut seg, asm);

    // ---- search indexes over the delta's own nodes ----
    if indexes {
        let name_post_off = seg.p.name_post_off.clone();
        let qname_post_off = seg.p.qname_post_off.clone();
        let ni = index::build_name_indexes(&seg.names, |i| name_post_off[i + 1] > name_post_off[i] || qname_post_off[i + 1] > qname_post_off[i]);
        seg.p.names_fst = Part::owned(ni.names_fst);
        seg.p.lower_fst = Part::owned(ni.lower_fst);
        seg.p.lower_post_off = Part::owned(ni.lower_post_off);
        seg.p.lower_post = Part::owned(ni.lower_post);
        seg.p.segs_fst = Part::owned(ni.segs_fst);
        seg.p.seg_post_off = Part::owned(ni.seg_post_off);
        seg.p.seg_post = Part::owned(ni.seg_post);
        seg.p.tri_keys = Part::owned(ni.tri_keys);
        seg.p.tri_off = Part::owned(ni.tri_off);
        seg.p.tri_post = Part::owned(ni.tri_post);
        let slot: FxHashMap<u32, usize> = seg.p.delta_files.chunks_exact(3).enumerate().map(|(i, t)| (t[0], i)).collect();
        let ti = index::build_token_index(&TokenInputs {
            hot: &seg.p.nodes_hot,
            cold: &seg.p.nodes_cold,
            files: &seg.p.files,
            names: &seg.names,
            paths: &seg.paths,
            text: &seg.text,
            names_base: bb.names,
            paths_base: bb.paths,
            text_base: bb.text,
            node_base: bb.nodes,
            file_slot: Some(&slot),
        });
        seg.p.tokens_fst = Part::owned(ti.fst);
        seg.p.token_post_off = Part::owned(ti.post_off);
        seg.p.token_post = Part::owned(ti.post);
        seg.p.node_norm = Part::owned(ti.node_norm);
    }

    seg.meta.kind = "delta".into();
    seg.meta.base_gen = base_gen;
    seg.meta.n_files = total_files;
    seg.meta.n_nodes = total_nodes;
    seg.meta.n_edges = total_edges;
    seg.meta.n_refs = seg.p.refs.len() as u32;
    seg.rebuild_derived()
}

/// Number of live (non-file-node-only) definitions etc. is not needed here; helper for tests:
/// the nodes of a base file.
pub fn base_nodes_of(base: &Seg, f: FileId) -> std::ops::Range<u32> {
    let r = &base.p.files[f.idx()];
    r.node_start.0..r.node_start.0 + r.node_count
}

#[allow(dead_code)]
fn _unused(_: NodeKind) {
    let _ = (&|a: &[NodeHot]| a.par_iter().count(),);
}
