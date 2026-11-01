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

/// Id-independent identity of a node: path, kind, qualified name and position.
pub fn node_key(view: &View, n: NodeIdx) -> String {
    let node = view.node(n);
    let path = view.file(node.file()).path();
    let dead = if view.is_alive(n) { "" } else { "!dead" };
    format!("{path}:{}:{}#{}@{}:{}{dead}", node.start_line(), node.start_col(), node.kind(), node.qname(), node.end_line())
}

/// Canonical text of everything a snapshot says, independent of internal ids, file order and
/// layer structure. Two views with equal canonical text are the same graph: used to check
/// "incremental == full" (design doc 9.6).
pub fn canonical(view: &View) -> Vec<String> {
    use crate::records::status;
    let mut files: Vec<_> = view.files().collect();
    files.sort_by(|a, b| view.file(*a).path().cmp(view.file(*b).path()));
    let mut out = Vec::new();
    for f in files {
        let fr = view.file(f);
        out.push(format!("F {} lang={} flags={} size={}", fr.path(), fr.lang().as_str(), fr.flags(), fr.rec.size));
        let mut lines: Vec<String> = Vec::new();
        for n in view.nodes_in_file(f) {
            let n = NodeIdx(n);
            let node = view.node(n);
            lines.push(format!(
                "N {} sig={:?} ret={:?} vis={} flags={} deco={:?} name={}",
                node_key(view, n),
                node.signature(),
                node.return_type(),
                node.visibility() as u8,
                view.hot(n).flags,
                node.decorators(),
                node.name()
            ));
            let mut es: Vec<String> = view
                .out_edges(n)
                .map(|e| {
                    format!(
                        "E {} -{}-> {} line={:?} col={:?} class={} prov={} res={} meta={:?}",
                        node_key(view, n),
                        e.kind,
                        node_key(view, e.dst),
                        e.line,
                        e.col,
                        e.class,
                        e.provenance as u8,
                        e.res,
                        view.edge_meta(e.eid)
                    )
                })
                .collect();
            es.sort();
            lines.extend(es);
        }
        for r in view.refs_of_file(f) {
            let target = if r.status == status::RESOLVED { node_key(view, r.target) } else { format!("#{}", r.target.0) };
            lines.push(format!(
                "R {} {}:{} name={} kind={} flags={} status={} by={} target={} cand={:?} tail={}",
                node_key(view, r.from),
                r.line,
                r.col,
                view.text_str(r.name),
                r.kind,
                r.flags,
                status::name(r.status),
                r.by,
                target,
                view.list(r.candidates),
                view.name_str(r.tail)
            ));
        }
        for i in view.imports_of(f) {
            lines.push(format!("I {} {} {} kind={} line={}", view.text_str(i.local), view.text_str(i.imported), view.text_str(i.spec), i.kind, i.line));
        }
        for e in view.exports_of(f) {
            lines.push(format!("X {} {} {} star={}", view.text_str(e.name), view.text_str(e.from_spec), view.text_str(e.from_name), e.star));
        }
        out.extend(lines);
    }
    out
}
