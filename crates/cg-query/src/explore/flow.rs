//! Named-symbol flow (design doc 8.3 step 4): the symbols a query names may be starts or ends
//! of a call chain. Two named symbols are linked directly or through at most one unnamed
//! bridge; the longest chain wins; synthesised (dynamic-dispatch) edges are followed too.

use crate::graph::Graph;
use crate::traverse::{is_call_like, is_synth};
use cg_core::{EdgeKind, NodeIdx};
use cg_store::EdgeRef;
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Clone, Debug)]
pub struct NamedTok {
    pub tok: usize,
    pub nodes: Vec<NodeIdx>,
}

#[derive(Clone)]
pub struct FlowStep {
    pub node: NodeIdx,
    pub edge: Option<EdgeRef>,
}

#[derive(Default)]
pub struct Flow {
    pub chain: Vec<FlowStep>,
    pub path_nodes: FxHashSet<NodeIdx>,
    pub named_nodes: FxHashSet<NodeIdx>,
    /// `src → tgt   [dynamic: ...]` lines for synthesised edges at named symbols.
    pub synth_lines: Vec<String>,
}

#[derive(Clone)]
struct Link {
    to: NodeIdx,
    /// 1 or 2 edges: via an optional bridge node.
    first: EdgeRef,
    bridge: Option<(NodeIdx, EdgeRef)>,
}

const MAX_FANOUT: usize = 400;
const MAX_EXPANSIONS: usize = 40_000;
const MAX_SYNTH_LINES: usize = 6;

pub fn synth_summary(by: &str) -> String {
    match by {
        "react-render" => "React re-render (setState → render)".into(),
        "jsx-render" => "JSX child render".into(),
        "callback" => "callback registered, then dispatched".into(),
        "event-emitter" => "event emitted, then handled".into(),
        "interface-impl" => "interface/abstract dispatch to an implementation".into(),
        "const-alias" => "alias of a function".into(),
        "object-registry" => "registry dispatch".into(),
        "go-implements" => "struct implements interface".into(),
        "go-method-contains" => "method of struct".into(),
        "go-func-field" => "function stored in a struct field".into(),
        "gin-middleware-chain" => "middleware chain dispatch".into(),
        "spring-event" => "Spring event published, then listened for".into(),
        other => other.to_owned(),
    }
}

impl Graph {
    /// `dynamic: summary @site` for a synthesised edge, or its kind.
    pub fn edge_note(&self, e: &EdgeRef) -> String {
        if let Some(by) = self.synth_label(e) {
            let site = self.synth_site(e).map(|s| format!(" @{s}")).unwrap_or_default();
            return format!("dynamic: {}{site}", synth_summary(&by));
        }
        e.kind.as_str().to_owned()
    }

    pub fn build_flow(&self, named: &[NamedTok]) -> Flow {
        let mut flow = Flow::default();
        let mut tok_of: FxHashMap<NodeIdx, usize> = FxHashMap::default();
        for nt in named {
            for n in &nt.nodes {
                tok_of.entry(*n).or_insert(nt.tok);
                flow.named_nodes.insert(*n);
            }
        }
        if tok_of.len() < 2 {
            return flow;
        }
        let mut all: Vec<NodeIdx> = tok_of.keys().copied().collect();
        all.sort_by_key(|n| (self.node_path(*n).to_owned(), self.line(*n)));

        // links between named nodes
        let mut succ: FxHashMap<NodeIdx, Vec<Link>> = FxHashMap::default();
        for &a in &all {
            let mut direct: FxHashMap<NodeIdx, Link> = FxHashMap::default();
            let mut via: FxHashMap<NodeIdx, Link> = FxHashMap::default();
            for e1 in self.view.out_edges(a).take(MAX_FANOUT) {
                if !is_call_like(&e1) || e1.dst == a {
                    continue;
                }
                if tok_of.contains_key(&e1.dst) {
                    if tok_of[&e1.dst] != tok_of[&a] {
                        direct.entry(e1.dst).or_insert(Link { to: e1.dst, first: e1, bridge: None });
                    }
                    continue;
                }
                if !self.kind(e1.dst).is_callable() && !self.kind(e1.dst).is_type() {
                    continue;
                }
                for e2 in self.view.out_edges(e1.dst).take(MAX_FANOUT) {
                    if !is_call_like(&e2) || e2.dst == a || e2.dst == e1.dst {
                        continue;
                    }
                    if tok_of.get(&e2.dst).is_some_and(|t| *t != tok_of[&a]) {
                        via.entry(e2.dst).or_insert(Link { to: e2.dst, first: e1, bridge: Some((e1.dst, e2)) });
                    }
                }
            }
            let mut links: Vec<Link> = direct.into_values().collect();
            for (k, l) in via {
                if !links.iter().any(|x| x.to == k) {
                    links.push(l);
                }
            }
            links.sort_by_key(|l| (l.bridge.is_some(), self.node_path(l.to).to_owned(), self.line(l.to)));
            succ.insert(a, links);
        }

        // longest chain, each token used once
        struct Search<'a> {
            g: &'a Graph,
            succ: &'a FxHashMap<NodeIdx, Vec<Link>>,
            tok_of: &'a FxHashMap<NodeIdx, usize>,
            best: Vec<(NodeIdx, Option<Link>)>,
            best_key: (usize, i64),
            expansions: usize,
        }
        impl<'a> Search<'a> {
            fn go(&mut self, path: &mut Vec<(NodeIdx, Option<Link>)>, used: u64, visited: &mut FxHashSet<NodeIdx>) {
                let named = path.len();
                let bridges = path.iter().filter(|p| p.1.as_ref().is_some_and(|l| l.bridge.is_some())).count() as i64;
                let key = (named, -bridges);
                if key > self.best_key {
                    self.best_key = key;
                    self.best = path.clone();
                }
                self.expansions += 1;
                if self.expansions > MAX_EXPANSIONS {
                    return;
                }
                let cur = path.last().unwrap().0;
                let Some(links) = self.succ.get(&cur) else { return };
                for l in links {
                    let t = self.tok_of[&l.to];
                    if used >> t.min(63) & 1 == 1 || visited.contains(&l.to) {
                        continue;
                    }
                    if let Some((b, _)) = &l.bridge {
                        if visited.contains(b) {
                            continue;
                        }
                        visited.insert(*b);
                    }
                    visited.insert(l.to);
                    path.push((l.to, Some(l.clone())));
                    self.go(path, used | 1u64 << t.min(63), visited);
                    path.pop();
                    visited.remove(&l.to);
                    if let Some((b, _)) = &l.bridge {
                        visited.remove(b);
                    }
                    if self.expansions > MAX_EXPANSIONS {
                        return;
                    }
                }
            }
        }
        let mut s = Search { g: self, succ: &succ, tok_of: &tok_of, best: Vec::new(), best_key: (1, 0), expansions: 0 };
        let _ = s.g;
        for &a in &all {
            let mut visited: FxHashSet<NodeIdx> = FxHashSet::default();
            visited.insert(a);
            let mut path = vec![(a, None)];
            s.go(&mut path, 1u64 << tok_of[&a].min(63), &mut visited);
        }
        if s.best.len() >= 2 {
            for (node, link) in &s.best {
                match link {
                    None => flow.chain.push(FlowStep { node: *node, edge: None }),
                    Some(l) => {
                        if let Some((b, e2)) = &l.bridge {
                            flow.chain.push(FlowStep { node: *b, edge: Some(l.first) });
                            flow.chain.push(FlowStep { node: *node, edge: Some(*e2) });
                        } else {
                            flow.chain.push(FlowStep { node: *node, edge: Some(l.first) });
                        }
                    }
                }
            }
            for st in &flow.chain {
                flow.path_nodes.insert(st.node);
            }
        }

        // synthesised edges at named symbols that the chain does not already show
        let mut shown: FxHashSet<(NodeIdx, NodeIdx)> = FxHashSet::default();
        for w in flow.chain.windows(2) {
            shown.insert((w[0].node, w[1].node));
        }
        let mut lines: Vec<(bool, String)> = Vec::new();
        for &n in &all {
            for e in self.view.out_edges(n).take(MAX_FANOUT) {
                if !is_synth(&e) || e.kind == EdgeKind::Contains || shown.contains(&(e.src, e.dst)) {
                    continue;
                }
                let both = tok_of.contains_key(&e.dst);
                lines.push((both, format!("- {} → {}   [{}]", self.name(e.src), self.name(e.dst), self.edge_note(&e))));
            }
            for e in self.view.in_edges(n).take(MAX_FANOUT) {
                if !is_synth(&e) || e.kind == EdgeKind::Contains || shown.contains(&(e.src, e.dst)) || tok_of.contains_key(&e.src) {
                    continue;
                }
                lines.push((false, format!("- {} → {}   [{}]", self.name(e.src), self.name(e.dst), self.edge_note(&e))));
            }
        }
        lines.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        lines.dedup_by(|a, b| a.1 == b.1);
        flow.synth_lines = lines.into_iter().map(|x| x.1).take(MAX_SYNTH_LINES).collect();
        flow
    }

    /// Render the Flow section (empty when there is neither a chain nor synthetic links).
    pub fn render_flow(&self, flow: &Flow) -> String {
        if flow.chain.is_empty() && flow.synth_lines.is_empty() {
            return String::new();
        }
        let mut out: Vec<String> = Vec::new();
        if !flow.chain.is_empty() {
            out.push(crate::strings::flow_header().to_owned());
            out.push(String::new());
            for (i, st) in flow.chain.iter().enumerate() {
                if let Some(e) = &st.edge {
                    out.push(format!("   ↓ {}", self.edge_note(e)));
                }
                out.push(format!("{}. {}", i + 1, self.locate(st.node)));
            }
            out.push(String::new());
        }
        if !flow.synth_lines.is_empty() {
            out.push(crate::strings::dynamic_links_header().to_owned());
            out.push(String::new());
            out.extend(flow.synth_lines.iter().cloned());
            out.push(String::new());
        }
        out.push(crate::strings::FLOW_FOOTER.to_owned());
        out.push(String::new());
        out.join("\n")
    }
}
