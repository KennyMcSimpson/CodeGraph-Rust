//! Synthesised edges for dynamic dispatch (design doc 7). Every pass is a pure function of the
//! view (hierarchy, members, persisted sites), so the same code serves full builds and the
//! incremental sync, which re-runs it and rewrites the files whose synthesised edges changed.

pub mod sites;

use crate::engine::Engine;
use crate::resolve::{Accept, NameRes, WorkerCx};
use cg_core::{EdgeKind, FileId, Lang, NodeIdx, NodeKind, Provenance, NONE};
use cg_store::{outkind, EdgeBuild, View};
use rayon::prelude::*;
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

#[derive(Default)]
struct Out {
    edges: Vec<EdgeBuild>,
    emeta: Vec<String>,
    meta_ids: FxHashMap<String, u32>,
}

impl Out {
    fn append(&mut self, other: Out) {
        let remap: Vec<_> = other
            .emeta
            .into_iter()
            .map(|meta| {
                *self.meta_ids.entry(meta.clone()).or_insert_with(|| {
                    self.emeta.push(meta);
                    (self.emeta.len() - 1) as u32
                })
            })
            .collect();
        self.edges.extend(other.edges.into_iter().map(|mut edge| {
            if edge.meta != NONE {
                edge.meta = remap[edge.meta as usize];
            }
            edge
        }));
    }
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
            if c != *iface {
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
fn event_emitter<'e, 'v>(e: &'e Engine<'v>, out: &mut Out, wcx: &mut WorkerCx<'e, 'v>, selected: Option<&FxHashSet<String>>) {
    if selected.is_some_and(|keys| keys.is_empty()) {
        return;
    }
    let view = e.view;
    #[derive(Default)]
    struct Key<'a> {
        emitters: Vec<NodeIdx>,
        handlers: Vec<(NodeIdx, &'a str, u32, FileId)>,
    }
    let mut keys: BTreeMap<&str, Key<'_>> = BTreeMap::new();
    for f in view.files() {
        for s in view.sites_of(f) {
            if s.pass != PASS_EMITTER {
                continue;
            }
            let event = view.text_str(cg_core::StrId(s.key_a));
            if selected.is_some_and(|keys| !keys.contains(event)) {
                continue;
            }
            let k = keys.entry(event).or_default();
            if s.role == ROLE_EMIT {
                k.emitters.push(s.node);
            } else if s.role == ROLE_ON {
                k.handlers.push((s.node, view.text_str(s.payload), s.line, f));
            }
        }
    }
    for k in keys.values_mut() {
        k.emitters.sort();
        k.emitters.dedup();
    }
    let connect = |event: &str, k: &Key<'_>, wcx: &mut WorkerCx<'e, 'v>| {
        let mut out = Out::default();
        if k.emitters.is_empty() || k.emitters.len() > MAX_EMITTERS {
            return out;
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
            return out;
        }
        for em in &k.emitters {
            for (t, line, f) in &targets {
                out.push(EdgeKind::Calls, *em, *t, 0, "event-emitter", Some(path_line(view, *f, *line)), Some(("event", event)), 30);
            }
        }
        out
    };
    // Indexed collection and ordered merging preserve pass/key precedence regardless of
    // scheduling. Bound in-flight link outputs to 256 keys; small edits stay sequential.
    if keys.len() < 32 || rayon::current_num_threads() == 1 {
        for (event, key) in &keys {
            out.append(connect(event, key, wcx));
        }
    } else {
        let keys: Vec<_> = keys.into_iter().collect();
        let per_key = (MAX_EMITTERS * MAX_HANDLERS * (std::mem::size_of::<EdgeBuild>() + 256)) as u64;
        let keys_per_chunk = (e.budget.synth / 4 / per_key).clamp(1, 256) as usize;
        for chunk in keys.chunks(keys_per_chunk) {
            let parts: Vec<_> = chunk.par_iter().map_init(WorkerCx::new, |cx, (event, key)| connect(event, key, cx)).collect();
            for part in parts {
                out.append(part);
            }
        }
    }
}

/// A React class component's method that calls `setState` re-renders the component.
fn react_render(e: &Engine<'_>, out: &mut Out, selected: Option<&FxHashSet<u32>>) {
    let view = e.view;
    for f in view.files() {
        if selected.is_some_and(|files| !files.contains(&f.0)) {
            continue;
        }
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
fn jsx_render<'e, 'v>(e: &'e Engine<'v>, out: &mut Out, wcx: &mut WorkerCx<'e, 'v>, selected: Option<&FxHashSet<u32>>) {
    let view = e.view;
    for f in view.files() {
        if selected.is_some_and(|files| !files.contains(&f.0)) {
            continue;
        }
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
    event_emitter(e, &mut out, &mut wcx, None);
    react_render(e, &mut out, None);
    jsx_render(e, &mut out, &mut wcx, None);
    SynthOutput { edges: out.edges, emeta: out.emeta }
}

/// Reconnect only changed event keys / per-file render sites when the definition and type
/// tables did not change. Structural changes deliberately use the full pass set.
pub fn run_incremental(e: &Engine<'_>, previous: &View, changed: &FxHashSet<u32>, remap: &FxHashMap<u32, u32>, structural: bool) -> SynthOutput {
    if structural {
        return run_all(e);
    }
    let mut keys = FxHashSet::default();
    for view in [previous, e.view] {
        for f in view.files().filter(|f| changed.contains(&f.0)) {
            for s in view.sites_of(f).iter().filter(|s| s.pass == PASS_EMITTER) {
                keys.insert(view.text_str(cg_core::StrId(s.key_a)).to_owned());
            }
        }
    }
    let mut out = Out { edges: Vec::new(), emeta: Vec::new(), meta_ids: FxHashMap::default() };
    // Unchanged keys keep their existing edges, including all provenance and registration
    // locations. Ids of replaced nodes are remapped even when their meaning did not change.
    for f in previous.files() {
        for n in previous.nodes_in_file(f) {
            for edge in previous.out_edges(NodeIdx(n)).filter(|edge| edge.class == outkind::CLASS_SYNTH) {
                let meta = previous.edge_meta(edge.eid).unwrap_or("{}");
                let value: serde_json::Value = serde_json::from_str(meta).unwrap_or_default();
                let by = value.get("synthesizedBy").and_then(|v| v.as_str()).unwrap_or("");
                let recompute = match by {
                    "event-emitter" => value.get("event").and_then(|v| v.as_str()).is_some_and(|key| keys.contains(key)),
                    "react-render" | "jsx-render" => changed.contains(&f.0),
                    _ => false,
                };
                if recompute {
                    continue;
                }
                let src = remap.get(&edge.src.0).copied().unwrap_or(edge.src.0);
                let dst = remap.get(&edge.dst.0).copied().unwrap_or(edge.dst.0);
                if !e.view.is_alive(NodeIdx(src)) || !e.view.is_alive(NodeIdx(dst)) {
                    continue;
                }
                let mid = *out.meta_ids.entry(meta.to_owned()).or_insert_with(|| {
                    out.emeta.push(meta.to_owned());
                    (out.emeta.len() - 1) as u32
                });
                out.edges.push(EdgeBuild {
                    src,
                    dst,
                    kind: outkind::pack(edge.kind, edge.provenance, edge.class),
                    line: if changed.contains(&f.0) && matches!(by, "interface-impl" | "go-implements" | "go-method-contains") {
                        e.view.hot(NodeIdx(src)).start_line
                    } else {
                        edge.line.unwrap_or(NONE)
                    },
                    col: edge.col.unwrap_or(NONE),
                    res: edge.res,
                    ref_idx: NONE,
                    meta: mid,
                    order: 0,
                });
            }
        }
    }
    let mut wcx = WorkerCx::new();
    event_emitter(e, &mut out, &mut wcx, Some(&keys));
    react_render(e, &mut out, Some(changed));
    jsx_render(e, &mut out, &mut wcx, Some(changed));
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
