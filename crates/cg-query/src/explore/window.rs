//! Source windows (design doc 8.3 step 7): which lines of a file to show. Symbols whose
//! ranges lie within `gap` lines of each other merge into one continuous window; oversize
//! bodies are cut to the parts the question is about; the whole file respects a character cap.

use crate::graph::Graph;
use cg_core::{NodeIdx, NodeKind};
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Clone, Debug)]
pub struct Want {
    pub start: u32,
    pub end: u32,
    pub imp: f32,
    /// Never dropped for budget (named / pinned / exact symbols).
    pub must: bool,
}

/// Bodies longer than this are shown as focus windows rather than in full.
const BODY_FULL_MAX: u32 = 150;
/// Containers longer than this show their header and the selected members only.
const CONTAINER_FULL_MAX: u32 = 40;
const MAX_FOCUS_WINDOWS: usize = 5;

pub struct WantCtx<'a> {
    pub g: &'a Graph,
    /// Symbols the question is about (exact, named, on the flow).
    pub question: &'a FxHashSet<NodeIdx>,
    /// Line anchors (`file.py:776`) for this file.
    pub anchors: &'a [u32],
    /// First child start line per container in this file.
    pub first_child: &'a FxHashMap<NodeIdx, u32>,
}

pub fn first_child_starts(g: &Graph, f: cg_core::FileId) -> FxHashMap<NodeIdx, u32> {
    let mut m: FxHashMap<NodeIdx, u32> = FxHashMap::default();
    for n in g.view.nodes_in_file(f) {
        let n = NodeIdx(n);
        let h = g.view.hot(n);
        if h.parent.is_some() && h.start_line > 0 {
            let e = m.entry(h.parent).or_insert(u32::MAX);
            if h.start_line < *e && !matches!(g.kind(n), NodeKind::Import | NodeKind::Export | NodeKind::Parameter) {
                *e = h.start_line;
            }
        }
    }
    m
}

impl<'a> WantCtx<'a> {
    pub fn for_node(&self, n: NodeIdx, imp: f32, must: bool, out: &mut Vec<Want>) {
        let g = self.g;
        let h = g.view.hot(n);
        let (s, e) = (h.start_line.max(1), h.end_line.max(h.start_line.max(1)));
        let kind = g.kind(n);
        let len = e - s + 1;
        if kind.is_container() && kind != NodeKind::File && len > CONTAINER_FULL_MAX {
            let first = self.first_child.get(&n).copied().unwrap_or(u32::MAX);
            let hdr_end = if first != u32::MAX && first > s { first - 1 } else { s };
            out.push(Want { start: s, end: hdr_end.min(s + 12).min(e), imp: imp * 0.6, must: false });
            return;
        }
        if len <= BODY_FULL_MAX {
            out.push(Want { start: s, end: e, imp, must });
            return;
        }
        // oversize body: signature + the lines the question is about
        out.push(Want { start: s, end: (s + 8).min(e), imp, must });
        let mut focus: Vec<u32> = Vec::new();
        for a in self.anchors {
            if *a > s && *a <= e {
                focus.push(*a);
            }
        }
        let mut calls: Vec<u32> = Vec::new();
        for ed in g.view.out_edges(n) {
            if let Some(l) = ed.line {
                if l > s && l <= e && self.question.contains(&ed.dst) && ed.dst != n {
                    calls.push(l);
                }
            }
        }
        calls.sort_unstable();
        calls.dedup();
        focus.extend(calls);
        focus.truncate(MAX_FOCUS_WINDOWS);
        for l in focus {
            out.push(Want { start: l.saturating_sub(6).max(s), end: (l + 14).min(e), imp, must });
        }
    }
}

#[derive(Clone, Debug)]
pub struct Range {
    pub start: u32,
    pub end: u32,
    pub imp: f32,
    pub must: bool,
}

fn merge(mut wants: Vec<Want>, gap: u32) -> Vec<Range> {
    wants.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out: Vec<Range> = Vec::new();
    for w in wants {
        match out.last_mut() {
            Some(r) if w.start <= r.end + gap + 1 => {
                r.end = r.end.max(w.end);
                r.imp = r.imp.max(w.imp);
                r.must |= w.must;
            }
            _ => out.push(Range { start: w.start, end: w.end, imp: w.imp, must: w.must }),
        }
    }
    out
}

/// Cost in characters of showing `[a, b]` (1-based, inclusive): `<n>\t<line>\n` per line.
pub fn range_cost(cost: &[u32], a: u32, b: u32) -> usize {
    let (a, b) = (a.max(1) as usize, (b as usize).min(cost.len().saturating_sub(1)));
    if a > b {
        return 0;
    }
    (cost[b] - cost[a - 1]) as usize
}

pub fn line_costs(lines: &[String]) -> Vec<u32> {
    let mut cost = Vec::with_capacity(lines.len() + 1);
    cost.push(0u32);
    let mut acc = 0u32;
    for (i, l) in lines.iter().enumerate() {
        let digits = ((i + 1) as f64).log10().floor() as u32 + 1;
        acc += l.len() as u32 + digits + 2;
        cost.push(acc);
    }
    cost
}

/// Merge wants and fit them in `cap` characters: lowest-importance windows go first, windows
/// marked `must` stay (the last one is clipped if even those do not fit).
pub fn plan(wants: Vec<Want>, gap: u32, cost: &[u32], cap: usize) -> Vec<Range> {
    let mut ranges = merge(wants, gap);
    let marker = 40usize;
    let total = |r: &[Range]| -> usize { r.iter().map(|x| range_cost(cost, x.start, x.end) + marker).sum() };
    while total(&ranges) > cap && ranges.len() > 1 {
        // drop the least important window that is not mandatory, else the least important one
        let pick = ranges
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.must)
            .min_by(|a, b| a.1.imp.partial_cmp(&b.1.imp).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        match pick {
            Some(i) => {
                ranges.remove(i);
            }
            None => {
                // all mandatory: drop from the end
                ranges.pop();
            }
        }
    }
    // a single window still too long: clip it
    if ranges.len() == 1 {
        let r = &mut ranges[0];
        while r.end > r.start + 11 && range_cost(cost, r.start, r.end) + marker > cap {
            r.end -= ((r.end - r.start) / 8).max(1);
        }
    }
    ranges
}

/// Render ranges as `<n>\t<line>` blocks joined by gap markers (named when the hole holds
/// indexed symbols).
pub fn render(g: &Graph, f: cg_core::FileId, lines: &[String], ranges: &[Range]) -> String {
    let mut out = String::new();
    let mut prev_end: Option<u32> = None;
    // symbols for named gaps
    let nodes: Vec<(String, u32, u32)> = g
        .view
        .nodes_in_file(f)
        .map(NodeIdx)
        .filter(|n| g.kind(*n).is_definition() && !matches!(g.kind(*n), NodeKind::Property | NodeKind::Field | NodeKind::Variable | NodeKind::Constant))
        .map(|n| (g.name(n).to_owned(), g.line(n), g.end_line(n)))
        .collect();
    for r in ranges {
        if let Some(pe) = prev_end {
            let elided: Vec<&(String, u32, u32)> = nodes.iter().filter(|(_, s, e)| *s > pe && *e < r.start).collect();
            if elided.is_empty() {
                out.push_str(crate::strings::BARE_GAP_MARKER);
            } else {
                let path = g.path(f);
                let shown: Vec<String> = elided.iter().take(6).map(|(n, s, _)| format!("{n} ({path}:{s})")).collect();
                let more = if elided.len() > 6 { format!(", +{} more", elided.len() - 6) } else { String::new() };
                out.push_str(&format!("\n\n... (gap: {}{more}) ...\n\n", shown.join(", ")));
            }
        }
        for n in r.start..=r.end.min(lines.len() as u32) {
            out.push_str(&n.to_string());
            out.push('\t');
            out.push_str(&lines[(n - 1) as usize]);
            if n != r.end.min(lines.len() as u32) {
                out.push('\n');
            }
        }
        prev_end = Some(r.end);
    }
    out
}
