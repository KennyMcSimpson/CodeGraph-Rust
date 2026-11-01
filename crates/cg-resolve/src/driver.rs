//! Resolution rounds (design doc 6.4). Round 1 resolves imports and type references, which yields
//! the type hierarchy; round 2 resolves calls and everything else against it. Inside a round
//! references are independent, so results never depend on thread scheduling.

use crate::engine::Engine;
use crate::resolve::{by, FileCx, Outcome, WorkerCx};
use cg_core::{EdgeKind, FileId, Lang, NodeIdx, NodeKind, Provenance, RefKind, NONE};
use cg_store::{outkind, status, EdgeBuild, RefUpdate};
use rayon::prelude::*;

#[derive(Default)]
pub struct ResolveOutput {
    /// One entry per reference of the view (global ref ids).
    pub updates: Vec<RefUpdate>,
    pub edges: Vec<EdgeBuild>,
    pub emeta: Vec<String>,
    /// Per `ImportRec` in segment order.
    pub import_targets: Vec<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Round {
    One,
    Two,
}

fn in_round(kind: u8, round: Round) -> bool {
    let first = matches!(
        RefKind::from_wire(kind),
        Some(RefKind::Edge(
            EdgeKind::Imports | EdgeKind::Extends | EdgeKind::Implements | EdgeKind::TypeOf | EdgeKind::Returns | EdgeKind::References | EdgeKind::Instantiates
        ))
    );
    (round == Round::One) == first
}

/// Resolve the references `ids` (global ids) of the given files. Used by full builds (all files
/// and refs) and by incremental sync (a subset).
pub fn resolve_refs<'e, 'v>(e: &'e Engine<'v>, files: &[FileId], round: Round_, only: Option<&rustc_hash::FxHashSet<u32>>) -> Vec<(u32, Outcome)> {
    let round = match round {
        Round_::One => Round::One,
        Round_::Two => Round::Two,
    };
    let parts: Vec<Vec<(u32, Outcome)>> = files
        .par_iter()
        .map_init(WorkerCx::new, |wcx, &f| {
            let view = e.view;
            let range = view.ref_range(f);
            let refs = view.refs_of_file(f);
            let wanted = refs.iter().enumerate().any(|(i, r)| in_round(r.kind, round) && only.map_or(true, |s| s.contains(&(range.start + i as u32))));
            if !wanted {
                return Vec::new();
            }
            let fcx = FileCx::new(e, f);
            let mut out = Vec::with_capacity(refs.len());
            for (i, r) in refs.iter().enumerate() {
                let id = range.start + i as u32;
                if !in_round(r.kind, round) || only.map_or(false, |s| !s.contains(&id)) {
                    continue;
                }
                out.push((id, fcx.resolve_ref(wcx, r)));
            }
            out
        })
        .collect();
    parts.into_iter().flatten().collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Round_ {
    One,
    Two,
}

pub fn hierarchy_pairs(e: &Engine<'_>, outcomes: &[(u32, Outcome)]) -> Vec<(NodeIdx, NodeIdx)> {
    outcomes
        .iter()
        .filter(|(_, o)| o.status == status::RESOLVED)
        .filter_map(|(id, o)| {
            let r = e.view.reference(*id);
            matches!(RefKind::from_wire(r.kind), Some(RefKind::Edge(EdgeKind::Extends | EdgeKind::Implements)))
                .then_some((r.from, NodeIdx(o.target)))
        })
        .collect()
}

/// Kind of the edge a resolved reference becomes (design doc 6.3, last paragraph).
pub fn final_kind(rk: RefKind, src: NodeKind, dst: NodeKind, lang: Lang) -> EdgeKind {
    match rk {
        RefKind::FunctionRef => EdgeKind::References,
        RefKind::Edge(EdgeKind::Extends) if matches!(dst, NodeKind::Interface | NodeKind::Protocol | NodeKind::Trait) && !matches!(src, NodeKind::Interface | NodeKind::Protocol | NodeKind::Trait) => {
            EdgeKind::Implements
        }
        RefKind::Edge(EdgeKind::Calls) => {
            if lang == Lang::Go && dst.is_type() {
                EdgeKind::References
            } else if matches!(dst, NodeKind::Class | NodeKind::Struct | NodeKind::Union) {
                EdgeKind::Instantiates
            } else {
                EdgeKind::Calls
            }
        }
        RefKind::Edge(k) => k,
    }
}

pub fn make_edge(e: &Engine<'_>, id: u32, o: &Outcome, fnref_meta: u32) -> Option<EdgeBuild> {
    if o.status != status::RESOLVED {
        return None;
    }
    let view = e.view;
    let r = view.reference(id);
    let dst = NodeIdx(o.target);
    let rk = RefKind::from_wire(r.kind)?;
    let from_file = view.hot(r.from).file;
    let lang = view.file(from_file).lang();
    let src_kind = NodeKind::from_u8(view.hot(r.from).kind).unwrap_or(NodeKind::File);
    let dst_kind = NodeKind::from_u8(view.hot(dst).kind).unwrap_or(NodeKind::Variable);
    let kind = final_kind(rk, src_kind, dst_kind, lang);
    if r.from == dst && kind == EdgeKind::Imports {
        return None;
    }
    let prov = if matches!(o.by, by::PACKAGE | by::GLOBAL) { Provenance::Heuristic } else { Provenance::TreeSitter };
    Some(EdgeBuild {
        src: r.from.0,
        dst: dst.0,
        kind: outkind::pack(kind, prov, outkind::CLASS_RESOLVE),
        line: r.line,
        col: r.col,
        res: o.by as u16 | (o.conf as u16) << 8,
        ref_idx: id,
        meta: if matches!(rk, RefKind::FunctionRef) { fnref_meta } else { NONE },
        order: 0,
    })
}

pub const FNREF_META: &str = r#"{"fnRef":true}"#;

pub fn update_of(o: &Outcome) -> RefUpdate {
    RefUpdate { status: o.status, by: o.by, target: o.target }
}

/// Full resolution of the view held by `e` (design doc 6.4). Consumes the hierarchy slot of the
/// engine, so call it on a fresh engine.
pub fn resolve_all(e: &mut Engine<'_>) -> ResolveOutput {
    let view = e.view;
    let n_refs = view.n_refs();
    let files: Vec<FileId> = view.files().collect();
    let mut outcomes = vec![Outcome::PENDING; n_refs];

    let r1 = resolve_refs(e, &files, Round_::One, None);
    let pairs = hierarchy_pairs(e, &r1);
    e.set_hierarchy(pairs.into_iter());
    for (id, o) in &r1 {
        outcomes[*id as usize] = *o;
    }
    let r2 = resolve_refs(e, &files, Round_::Two, None);
    for (id, o) in &r2 {
        outcomes[*id as usize] = *o;
    }

    let mut edges = Vec::new();
    for (id, o) in outcomes.iter().enumerate() {
        if let Some(edge) = make_edge(e, id as u32, o, 0) {
            edges.push(edge);
        }
    }
    let mut import_targets = Vec::new();
    for f in view.files() {
        for t in &e.imp_targets[f.idx()] {
            import_targets.push(match t {
                crate::modules::ImportTarget::File(tf) => tf.0,
                _ => NONE,
            });
        }
    }
    ResolveOutput {
        updates: outcomes.iter().map(update_of).collect(),
        edges,
        emeta: vec![FNREF_META.to_owned()],
        import_targets,
    }
}
