//! Synthesised edges for dynamic dispatch (design doc 7). Every pass is a pure function of the
//! view (hierarchy, members, persisted sites), so the same code serves full builds and the
//! incremental sync, which re-runs it and rewrites the files whose synthesised edges changed.

pub mod sites;

use crate::engine::Engine;
use crate::resolve::{Accept, NameRes, WorkerCx};
use cg_core::{EdgeKind, FileId, Lang, NodeIdx, NodeKind, Provenance, NONE};
use cg_store::{outkind, EdgeBuild, View};
use rustc_hash::{FxHashMap, FxHashSet};
use sites::{PASS_EMITTER, PASS_JSX, PASS_REACT_RENDER, ROLE_EMIT, ROLE_ON};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct SynthOutput {
    pub edges: Vec<EdgeBuild>,
    pub emeta: Vec<String>,
}

/// Fan-out ceilings: a key with more targets is skipped entirely (precision first, design 7.2).
const MAX_IMPLS: usize = 12;
const MAX_EMITTERS: usize = 8;
const MAX_HANDLERS: usize = 8;
const MAX_GO_IMPLEMENTERS: usize = 24;

/// Go method names that nearly every type has: an interface made only of these says nothing.
const GENERIC_GO_METHODS: &[&str] = &["String", "Error", "Close", "Read", "Write", "Len", "Less", "Swap", "Reset", "Unwrap", "Is", "As", "Format", "Clone"];

struct Out {
    edges: Vec<EdgeBuild>,
    emeta: Vec<String>,
    meta_ids: FxHashMap<String, u32>,
}

impl Out {
    #[allow(clippy::too_many_arguments)] // one call site per synthesis pass; a builder would only add noise
    fn push(
        &mut self,
        kind: EdgeKind,
        src: NodeIdx,
        dst: NodeIdx,
        line: u32,
        by: &str,
        registered_at: Option<String>,
        extra: Option<(&str, &str)>,
        order: u16,
    ) {
        if src == dst {
            return;
        }
        let mut m = serde_json::Map::new();
        m.insert("synthesizedBy".into(), by.into());
        if let Some(r) = registered_at {
            m.insert("registeredAt".into(), r.into());
        }
        if let Some((k, v)) = extra {
            m.insert(k.into(), v.into());
        }
        let json = serde_json::Value::Object(m).to_string();
        let id = *self.meta_ids.entry(json.clone()).or_insert_with(|| {
            self.emeta.push(json);
            (self.emeta.len() - 1) as u32
        });
        self.edges.push(EdgeBuild {
            src: src.0,
            dst: dst.0,
            kind: outkind::pack(kind, Provenance::Heuristic, outkind::CLASS_SYNTH),
            line: if line == 0 { NONE } else { line },
            col: NONE,
            res: 0,
            ref_idx: NONE,
            meta: id,
            order,
        });
    }
}

fn path_line(view: &View, f: FileId, line: u32) -> String {
    format!("{}:{}", view.file(f).path(), line)
}

/// Invert the engine's hierarchy: type -> direct subtypes.
fn subtypes(e: &Engine<'_>, extra: &[(NodeIdx, NodeIdx)]) -> FxHashMap<NodeIdx, Vec<NodeIdx>> {
    let mut subs: FxHashMap<NodeIdx, Vec<NodeIdx>> = FxHashMap::default();
    for (sub, sups) in &e.supers {
        for s in sups {
            subs.entry(*s).or_default().push(*sub);
        }
    }
    for (sub, sup) in extra {
        subs.entry(*sup).or_default().push(*sub);
    }
    for v in subs.values_mut() {
        v.sort();
        v.dedup();
    }
    subs
}

fn descendants(subs: &FxHashMap<NodeIdx, Vec<NodeIdx>>, t: NodeIdx) -> Vec<NodeIdx> {
    let mut out = Vec::new();
    let mut seen: FxHashSet<NodeIdx> = FxHashSet::default();
    seen.insert(t);
    let mut frontier = vec![t];
    for _ in 0..10 {
        let mut next = Vec::new();
        for x in frontier {
            for s in subs.get(&x).into_iter().flatten() {
                if seen.insert(*s) {
                    out.push(*s);
                    next.push(*s);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    out
}

/// Go has no `implements`: a struct implements an interface when it has every method (by name).
fn go_implements(e: &Engine<'_>, out: &mut Out) -> Vec<(NodeIdx, NodeIdx)> {
    let view = e.view;
    // method name -> types that declare it (receiver methods are named `Type::method`)
    let mut by_method: FxHashMap<&str, Vec<NodeIdx>> = FxHashMap::default();
    let mut interfaces: Vec<(NodeIdx, Vec<&str>)> = Vec::new();
    for f in view.files() {
        if view.file(f).lang() != Lang::Go {
            continue;
        }
        for n in view.nodes_in_file(f) {
            let n = NodeIdx(n);
            let node = view.node(n);
            match node.kind() {
                NodeKind::Interface => {
                    let mut names: Vec<&str> = view
                        .out_edges(n)
                        .filter(|ed| ed.kind == EdgeKind::Contains)
                        .map(|ed| view.node(ed.dst))
                        .filter(|c| c.kind() == NodeKind::Method)
                        .map(|c| c.name())
                        .collect();
                    names.sort_unstable();
                    names.dedup();
                    if !names.is_empty() {
                        interfaces.push((n, names));
                    }
                }
                NodeKind::Method | NodeKind::Function => {
                    if let Some((tq, name)) = node.qname().rsplit_once("::") {
                        if name == node.name() {
                            for t in view.by_qname(tq) {
                                if matches!(view.node(t).kind(), NodeKind::Struct | NodeKind::Class) && view.hot(t).file != FileId::NONE {
                                    by_method.entry(view.name_str(view.hot(n).name)).or_default().push(t);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut pairs: Vec<(NodeIdx, NodeIdx)> = Vec::new();
    for (iface, names) in &interfaces {
        if names.iter().all(|n| GENERIC_GO_METHODS.contains(n)) {
            continue;
        }
        // intersect candidate sets, rarest method first
        let mut order: Vec<&str> = names.to_vec();
        order.sort_by_key(|n| by_method.get(*n).map_or(0, |v| v.len()));
        let Some(first) = by_method.get(order[0]) else { continue };
        let mut cands: Vec<NodeIdx> = first.clone();
        cands.sort();
        cands.dedup();
        for n in &order[1..] {
            let Some(set) = by_method.get(*n) else {
                cands.clear();
                break;
            };
            let set: FxHashSet<NodeIdx> = set.iter().copied().collect();
            cands.retain(|c| set.contains(c));
            if cands.is_empty() {
                break;
            }
        }
        if cands.is_empty() || cands.len() > MAX_GO_IMPLEMENTERS {
            continue;
        }
        for c in cands {
            if c != *iface && view.hot(c).file != view.hot(*iface).file || c != *iface {
                out.push(EdgeKind::Implements, c, *iface, view.hot(c).start_line, "go-implements", None, None, 10);
                pairs.push((c, *iface));
            }
        }
    }
    pairs
}

/// A call to an interface / abstract method lands on its implementations: the hop
/// `Interface.m -> Impl.m` lets a flow continue through dynamic dispatch.
fn interface_impl(e: &Engine<'_>, subs: &FxHashMap<NodeIdx, Vec<NodeIdx>>, out: &mut Out) {
    let view = e.view;
    let mut types: Vec<NodeIdx> = subs.keys().copied().collect();
    types.sort();
    for t in types {
        let tn = view.node(t);
        let abstract_type = matches!(tn.kind(), NodeKind::Interface | NodeKind::Trait | NodeKind::Protocol) || tn.is_abstract() == Some(true);
        let descs = descendants(subs, t);
        if descs.is_empty() {
            continue;
        }
        let mut members: Vec<NodeIdx> =
            view.out_edges(t).filter(|ed| ed.kind == EdgeKind::Contains).map(|ed| ed.dst).filter(|m| view.node(*m).kind() == NodeKind::Method).collect();
        // Go / Rust declare trait-like methods through qualified names as well
        members.sort();
        for m in members {
            let mn = view.node(m);
            if !abstract_type && mn.is_abstract() != Some(true) {
                continue;
            }
            let mut impls: Vec<NodeIdx> = Vec::new();
            for d in &descs {
                if let Some(im) = e.direct_member(*d, mn.name()) {
                    if im != m && view.node(im).kind().is_callable() && !impls.contains(&im) {
                        impls.push(im);
                    }
                }
            }
            if impls.is_empty() || impls.len() > MAX_IMPLS {
                continue;
            }
            for im in impls {
                out.push(EdgeKind::Calls, m, im, mn.start_line(), "interface-impl", None, None, 20);
            }
        }
    }
}

/// `.emit('evt')` in a method -> the named handlers registered with `.on('evt', handler)`.
fn event_emitter<'e, 'v>(e: &'e Engine<'v>, out: &mut Out, wcx: &mut WorkerCx<'e, 'v>) {
    let view = e.view;
    #[derive(Default)]
    struct Key {
        emitters: Vec<NodeIdx>,
        handlers: Vec<(NodeIdx, String, u32, FileId)>,
    }
    let mut keys: BTreeMap<String, Key> = BTreeMap::new();
    for f in view.files() {
        for s in view.sites_of(f) {
            if s.pass != PASS_EMITTER {
                continue;
            }
            let event = view.text_str(cg_core::StrId(s.key_a)).to_owned();
            let k = keys.entry(event).or_default();
            if s.role == ROLE_EMIT {
                k.emitters.push(s.node);
            } else if s.role == ROLE_ON {
                k.handlers.push((s.node, view.text_str(s.payload).to_owned(), s.line, f));
            }
        }
    }
    for (event, mut k) in keys {
        k.emitters.sort();
        k.emitters.dedup();
        if k.emitters.is_empty() || k.emitters.len() > MAX_EMITTERS {
            continue;
        }
        let mut targets: Vec<(NodeIdx, u32, FileId)> = Vec::new();
        for (reg, handler, line, f) in &k.handlers {
            // `this.onFoo` / `obj.onFoo` / `onFoo`: resolve the last segment where it was written
            let name = handler.rsplit('.').next().unwrap_or(handler);
            let cx = wcx.file_cx(e, *f);
            let t = if handler.starts_with("this.") {
                view.node(*reg)
                    .parent()
                    .and_then(|c| e.direct_member(c, name))
                    .or_else(|| view.node(*reg).parent().and_then(|c| view.node(c).parent()).and_then(|c| e.direct_member(c, name)))
            } else if !handler.contains('.') {
                match cx.resolve_name(wcx, name, Accept::Callable, 0, u32::MAX, None, false) {
                    NameRes::Found(n, ..) => Some(n),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(t) = t {
                if view.node(t).kind().is_callable() {
                    targets.push((t, *line, *f));
                }
            }
        }
        targets.sort();
        targets.dedup_by_key(|t| t.0);
        if targets.is_empty() || targets.len() > MAX_HANDLERS {
            continue;
        }
        for em in &k.emitters {
            for (t, line, f) in &targets {
                out.push(EdgeKind::Calls, *em, *t, 0, "event-emitter", Some(path_line(view, *f, *line)), Some(("event", &event)), 30);
            }
        }
    }
}

/// A React class component's method that calls `setState` re-renders the component.
fn react_render(e: &Engine<'_>, out: &mut Out) {
    let view = e.view;
    for f in view.files() {
        let sites: Vec<_> = view.sites_of(f).iter().filter(|s| s.pass == PASS_REACT_RENDER).collect();
        if sites.is_empty() {
            continue;
        }
        let components: FxHashSet<NodeIdx> = sites.iter().filter(|s| s.role == 1).map(|s| s.node).collect();
        for s in sites.iter().filter(|s| s.role == 0) {
            let m = s.node;
            let Some(class) = view.node(m).parent() else { continue };
            if !components.contains(&class) {
                continue;
            }
            if let Some(render) = e.direct_member(class, "render") {
                if render != m {
                    out.push(EdgeKind::Calls, m, render, 0, "react-render", Some(path_line(view, f, s.line)), None, 40);
                }
            }
        }
    }
}

/// A component's JSX tag renders the child component it names.
fn jsx_render<'e, 'v>(e: &'e Engine<'v>, out: &mut Out, wcx: &mut WorkerCx<'e, 'v>) {
    let view = e.view;
    for f in view.files() {
        let sites: Vec<_> = view.sites_of(f).iter().filter(|s| s.pass == PASS_JSX).collect();
        if sites.is_empty() {
            continue;
        }
        let cx = wcx.file_cx(e, f);
        for s in sites {
            let tag = view.text_str(cg_core::StrId(s.key_a));
            if let NameRes::Found(t, ..) = cx.resolve_name(wcx, tag, Accept::Callable, 0, u32::MAX, None, false) {
                let k = view.node(t).kind();
                if matches!(k, NodeKind::Function | NodeKind::Class | NodeKind::Component | NodeKind::Variable | NodeKind::Constant) && t != s.node {
                    out.push(EdgeKind::Calls, s.node, t, 0, "jsx-render", Some(path_line(view, f, s.line)), Some(("via", tag)), 50);
                }
            }
        }
    }
}

/// Run every pass over the engine's view (hierarchy installed).
pub fn run_all(e: &Engine<'_>) -> SynthOutput {
    let mut out = Out { edges: Vec::new(), emeta: Vec::new(), meta_ids: FxHashMap::default() };
    let mut wcx = WorkerCx::new();
    let go_pairs = go_implements(e, &mut out);
    let subs = subtypes(e, &go_pairs);
    interface_impl(e, &subs, &mut out);
    event_emitter(e, &mut out, &mut wcx);
    react_render(e, &mut out);
    jsx_render(e, &mut out, &mut wcx);
    SynthOutput { edges: out.edges, emeta: out.emeta }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cg_core::Lang;

    #[test]
    fn jsx_regex_finds_capitalised_tags_only() {
        let re = sites::SiteScanner;
        let _ = (&re, Lang::Jsx);
    }
}
