//! Human-readable and canonical dumps of a snapshot, for tests, `cgr debug dump` and the
//! oracle comparison (design doc 13.2).

use crate::records::status;
use crate::view::View;
use cg_core::{EdgeKind, NodeIdx, NodeKind};

/// `path::qname` of a node (`path` alone for file nodes).
pub fn node_label(view: &View, n: NodeIdx) -> String {
    let node = view.node(n);
    let path = view.file(node.file()).path();
    if node.kind() == NodeKind::File {
        path.to_owned()
    } else {
        format!("{}::{}", path, node.qname())
    }
}

/// All edges except `contains` as sorted `src -kind-> dst` lines.
pub fn dump_edges(view: &View) -> Vec<String> {
    let mut out = Vec::new();
    for f in view.files() {
        for n in view.nodes_in_file(f) {
            let n = NodeIdx(n);
            for e in view.out_edges(n) {
                if e.kind == EdgeKind::Contains {
                    continue;
                }
                out.push(format!("{} -{}-> {}", node_label(view, n), e.kind, node_label(view, e.dst)));
            }
        }
    }
    out.sort();
    out
}

/// Statistics line per reference status, for quick quality checks.
pub fn ref_stats(view: &View) -> Vec<(&'static str, usize)> {
    let mut counts = [0usize; 8];
    for f in view.files() {
        for r in view.refs_of_file(f) {
            counts[(r.status as usize).min(7)] += 1;
        }
    }
    (0..7).map(|i| (status::name(i as u8), counts[i])).collect()
}

/// Unresolved references as `path:line name (status)` for debugging.
pub fn dump_unresolved(view: &View) -> Vec<String> {
    let mut out = Vec::new();
    for f in view.files() {
        let path = view.file(f).path();
        for r in view.refs_of_file(f) {
            if r.status != status::RESOLVED {
                out.push(format!("{path}:{} {} [{}]", r.line, view.text_str(r.name), status::name(r.status)));
            }
        }
    }
    out
}
