//! Edge assembly (design doc 5.3, 5.5 step 5): sort, deduplicate by identity, build the
//! forward CSR (struct of arrays) and the reverse CSR.

use crate::build::EdgeBuild;
use crate::part::Part;
use crate::records::*;
use crate::seg::Seg;
use rayon::prelude::*;
use std::cmp::Ordering;

pub struct EdgeAsm {
    pub out_off: Vec<u32>,
    pub out_dst: Vec<u32>,
    pub out_kind: Vec<u8>,
    pub out_site: Vec<EdgeSite>,
    pub out_res: Vec<u16>,
    pub out_ref: Vec<u32>,
    pub in_off: Vec<u32>,
    pub in_src: Vec<u32>,
    pub in_eid: Vec<u32>,
    pub edge_meta: Vec<EdgeMetaRec>,
}

/// Priority of an edge class when two edges share an identity: extraction beats
/// resolution beats framework beats synthesis (design doc 5.3).
fn rank(class: u8) -> u8 {
    match class {
        outkind::CLASS_EXTRACT => 0,
        outkind::CLASS_RESOLVE => 1,
        outkind::CLASS_FRAMEWORK => 2,
        _ => 3,
    }
}

fn cmp(a: &EdgeBuild, b: &EdgeBuild) -> Ordering {
    (a.src, a.kind & 0x0F, a.dst, a.line, a.col)
        .cmp(&(b.src, b.kind & 0x0F, b.dst, b.line, b.col))
        .then_with(|| rank(outkind::class(a.kind)).cmp(&rank(outkind::class(b.kind))))
        .then_with(|| a.order.cmp(&b.order))
        .then_with(|| a.ref_idx.cmp(&b.ref_idx))
        .then_with(|| a.meta.cmp(&b.meta))
        .then_with(|| a.kind.cmp(&b.kind))
}

/// Sort and deduplicate `edges` in place and build the CSR arrays for `n_nodes` nodes.
pub fn assemble(edges: &mut Vec<EdgeBuild>, n_nodes: usize) -> EdgeAsm {
    edges.par_sort_unstable_by(cmp);
    edges.dedup_by(|b, a| a.src == b.src && a.dst == b.dst && (a.kind & 0x0F) == (b.kind & 0x0F) && a.line == b.line && a.col == b.col);
    let m = edges.len();
    let mut out_off = vec![0u32; n_nodes + 1];
    for e in edges.iter() {
        out_off[e.src as usize + 1] += 1;
    }
    for i in 0..n_nodes {
        out_off[i + 1] += out_off[i];
    }
    let mut out_dst = Vec::with_capacity(m);
    let mut out_kind = Vec::with_capacity(m);
    let mut out_site = Vec::with_capacity(m);
    let mut out_res = Vec::with_capacity(m);
    let mut out_ref = Vec::with_capacity(m);
    let mut edge_meta = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        out_dst.push(e.dst);
        out_kind.push(e.kind);
        out_site.push(EdgeSite { line: e.line, col: e.col });
        out_res.push(e.res);
        out_ref.push(e.ref_idx);
        if e.meta != cg_core::NONE {
            edge_meta.push(EdgeMetaRec { eid: i as u32, text: cg_core::StrId(e.meta) });
        }
    }
    // reverse CSR: counting sort by destination, stable in forward edge order
    let mut in_off = vec![0u32; n_nodes + 1];
    for d in &out_dst {
        in_off[*d as usize + 1] += 1;
    }
    for i in 0..n_nodes {
        in_off[i + 1] += in_off[i];
    }
    let mut fill = in_off.clone();
    let mut in_src = vec![0u32; m];
    let mut in_eid = vec![0u32; m];
    for (src, w) in out_off.windows(2).enumerate() {
        for eid in w[0]..w[1] {
            let d = out_dst[eid as usize] as usize;
            let slot = fill[d] as usize;
            in_src[slot] = src as u32;
            in_eid[slot] = eid;
            fill[d] += 1;
        }
    }
    EdgeAsm { out_off, out_dst, out_kind, out_site, out_res, out_ref, in_off, in_src, in_eid, edge_meta }
}

/// Edge arrays of a delta layer: only nodes that have edges appear (sorted), design doc 5.6.
pub struct SparseAsm {
    pub src_nodes: Vec<u32>,
    pub dst_nodes: Vec<u32>,
    pub asm: EdgeAsm,
}

/// Like [`assemble`], but the CSR is indexed by position in the sorted list of source nodes
/// (`d_src_nodes`) and, for the reverse side, of destination nodes (`d_dst_nodes`).
pub fn assemble_sparse(edges: &mut Vec<EdgeBuild>) -> SparseAsm {
    edges.par_sort_unstable_by(cmp);
    edges.dedup_by(|b, a| a.src == b.src && a.dst == b.dst && (a.kind & 0x0F) == (b.kind & 0x0F) && a.line == b.line && a.col == b.col);
    let m = edges.len();
    let mut src_nodes: Vec<u32> = Vec::new();
    let mut out_off: Vec<u32> = vec![0];
    for (i, e) in edges.iter().enumerate() {
        if src_nodes.last() != Some(&e.src) {
            if !src_nodes.is_empty() {
                out_off.push(i as u32);
            }
            src_nodes.push(e.src);
        }
    }
    if !src_nodes.is_empty() {
        out_off.push(m as u32);
    }
    let mut out_dst = Vec::with_capacity(m);
    let mut out_kind = Vec::with_capacity(m);
    let mut out_site = Vec::with_capacity(m);
    let mut out_res = Vec::with_capacity(m);
    let mut out_ref = Vec::with_capacity(m);
    let mut edge_meta = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        out_dst.push(e.dst);
        out_kind.push(e.kind);
        out_site.push(EdgeSite { line: e.line, col: e.col });
        out_res.push(e.res);
        out_ref.push(e.ref_idx);
        if e.meta != cg_core::NONE {
            edge_meta.push(EdgeMetaRec { eid: i as u32, text: cg_core::StrId(e.meta) });
        }
    }
    let mut dst_nodes: Vec<u32> = out_dst.clone();
    dst_nodes.sort_unstable();
    dst_nodes.dedup();
    let mut in_off = vec![0u32; dst_nodes.len() + 1];
    for d in &out_dst {
        let pos = dst_nodes.binary_search(d).expect("destination listed");
        in_off[pos + 1] += 1;
    }
    for i in 0..dst_nodes.len() {
        in_off[i + 1] += in_off[i];
    }
    let mut fill = in_off.clone();
    let mut in_src = vec![0u32; m];
    let mut in_eid = vec![0u32; m];
    for (si, w) in out_off.windows(2).enumerate() {
        for eid in w[0]..w[1] {
            let pos = dst_nodes.binary_search(&out_dst[eid as usize]).expect("destination listed");
            let slot = fill[pos] as usize;
            in_src[slot] = src_nodes[si];
            in_eid[slot] = eid;
            fill[pos] += 1;
        }
    }
    SparseAsm { src_nodes, dst_nodes, asm: EdgeAsm { out_off, out_dst, out_kind, out_site, out_res, out_ref, in_off, in_src, in_eid, edge_meta } }
}

pub fn install_sparse(seg: &mut Seg, a: SparseAsm) {
    seg.p.d_src_nodes = Part::owned(a.src_nodes);
    seg.p.d_dst_nodes = Part::owned(a.dst_nodes);
    install(seg, a.asm);
}

pub fn install(seg: &mut Seg, a: EdgeAsm) {
    seg.meta.n_edges = a.out_dst.len() as u32;
    seg.p.out_off = Part::owned(a.out_off);
    seg.p.out_dst = Part::owned(a.out_dst);
    seg.p.out_kind = Part::owned(a.out_kind);
    seg.p.out_site = Part::owned(a.out_site);
    seg.p.out_res = Part::owned(a.out_res);
    seg.p.out_ref = Part::owned(a.out_ref);
    seg.p.in_off = Part::owned(a.in_off);
    seg.p.in_src = Part::owned(a.in_src);
    seg.p.in_eid = Part::owned(a.in_eid);
    seg.p.edge_meta = Part::owned(a.edge_meta);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cg_core::{EdgeKind, Provenance, NONE};

    fn e(src: u32, dst: u32, kind: EdgeKind, class: u8, line: u32) -> EdgeBuild {
        EdgeBuild { src, dst, kind: outkind::pack(kind, Provenance::TreeSitter, class), line, col: 0, res: 0, ref_idx: NONE, meta: NONE, order: 0 }
    }

    #[test]
    fn csr_and_dedup_priority() {
        let mut v = vec![
            e(2, 0, EdgeKind::Calls, outkind::CLASS_SYNTH, 5),
            e(0, 1, EdgeKind::Contains, outkind::CLASS_EXTRACT, NONE),
            e(2, 0, EdgeKind::Calls, outkind::CLASS_RESOLVE, 5), // same identity: resolve beats synth
            e(0, 2, EdgeKind::Calls, outkind::CLASS_RESOLVE, 3),
        ];
        let a = assemble(&mut v, 3);
        assert_eq!(a.out_off, vec![0, 2, 2, 3]);
        assert_eq!(a.out_dst, vec![1, 2, 0]);
        assert_eq!(outkind::class(a.out_kind[2]), outkind::CLASS_RESOLVE);
        // reverse: node 0 has one in-edge (from 2), node 1 one (from 0), node 2 one (from 0)
        assert_eq!(a.in_off, vec![0, 1, 2, 3]);
        assert_eq!(a.in_src, vec![2, 0, 0]);
        assert_eq!(a.in_eid, vec![2, 0, 1]);
    }
}
