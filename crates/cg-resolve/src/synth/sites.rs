//! Synthesis sites (design doc 7.2): observations recorded while the source is in memory, keyed
//! for the later, source-free "link" step. One scanner covers the JS/TS passes: event emitters,
//! React `setState` and JSX child components.

use cg_core::{Lang, LangFamily, NodeKind};
use cg_extract::{FileHook, HookCtx, HookOut, SiteFact};
use regex::Regex;
use std::sync::OnceLock;

pub const PASS_EMITTER: u16 = 1;
pub const PASS_REACT_RENDER: u16 = 2;
pub const PASS_JSX: u16 = 3;

pub const ROLE_EMIT: u8 = 0;
pub const ROLE_ON: u8 = 1;

/// Event names too generic to link on (design doc 7.3).
const GENERIC_EVENTS: &[&str] = &[
    "error",
    "data",
    "end",
    "close",
    "open",
    "message",
    "connect",
    "disconnect",
    "ready",
    "finish",
    "drain",
    "timeout",
    "abort",
    "exit",
    "load",
    "resize",
    "click",
    "change",
];

fn event_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"\.\s*(emit|on|once|addListener|prependListener)\s*\(\s*(['"`])([^'"`\n]{1,80})['"`](?:\s*,\s*((?:this\.)?[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*)\s*[,)])?"#).expect("static regex")
    })
}

fn component_class_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\bclass\s+[A-Za-z_$][\w$]*\s+extends\s+(?:React\s*\.\s*)?(?:Pure)?Component\b").expect("static regex"))
}

fn set_state_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\bthis\s*\.\s*setState\s*\(").expect("static regex"))
}

fn jsx_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<([A-Z][A-Za-z0-9_$]*)[\s/>]").expect("static regex"))
}

pub struct SiteScanner;

/// Row of the innermost node of one of `kinds` that contains `line`.
fn enclosing(ctx: &HookCtx<'_>, line: u32, kinds: &[NodeKind]) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None; // (span, row)
    for (row, n) in ctx.rows.nodes().enumerate() {
        if !kinds.contains(&n.kind()) {
            continue;
        }
        let (s, e) = (n.start_line(), n.end_line());
        if s <= line && line <= e {
            let span = e - s;
            if best.is_none_or(|(b, _)| span < b) {
                best = Some((span, row as u32));
            }
        }
    }
    best.map(|b| b.1)
}

fn line_of(src: &str, off: usize) -> u32 {
    src[..off].bytes().filter(|b| *b == b'\n').count() as u32 + 1
}

impl FileHook for SiteScanner {
    fn name(&self) -> &'static str {
        "synth-sites"
    }
    fn applies(&self, lang: Lang, _rel: &str) -> bool {
        lang.family() == LangFamily::Js
    }
    fn triggers(&self) -> &'static [&'static str] {
        &[".emit(", ".on(", ".once(", ".addListener(", ".prependListener(", "setState(", "</", "/>"]
    }
    fn run(&self, ctx: &HookCtx<'_>, out: &mut HookOut) {
        let src = ctx.src;
        let code_kinds = [NodeKind::Method, NodeKind::Function, NodeKind::Component, NodeKind::Class];
        if src.contains(".emit(") || src.contains(".on(") || src.contains(".once(") || src.contains(".addListener(") || src.contains(".prependListener(") {
            for m in event_re().captures_iter(src) {
                let event = &m[3];
                if GENERIC_EVENTS.contains(&event) {
                    continue;
                }
                let whole = m.get(0).unwrap();
                let line = line_of(src, whole.start());
                let is_emit = &m[1] == "emit";
                let row = enclosing(ctx, line, &code_kinds);
                if is_emit {
                    // an emit must happen inside a method or function
                    let Some(row) = row else { continue };
                    out.sites.push(SiteFact {
                        pass: PASS_EMITTER,
                        role: ROLE_EMIT,
                        key_kind: 0,
                        key_a: event.to_owned(),
                        key_b: String::new(),
                        node_row: row,
                        line,
                        payload: String::new(),
                    });
                } else if let Some(h) = m.get(4) {
                    // a registration may sit at the top level of the file
                    out.sites.push(SiteFact {
                        pass: PASS_EMITTER,
                        role: ROLE_ON,
                        key_kind: 0,
                        key_a: event.to_owned(),
                        key_b: String::new(),
                        node_row: row.unwrap_or(cg_core::NONE),
                        line,
                        payload: h.as_str().to_owned(),
                    });
                }
            }
        }
        if src.contains("setState(") {
            // role 1: the class extends React's Component (the kernel keeps no `extends` reference
            // for a qualified base such as `React.Component`)
            for m in component_class_re().find_iter(src) {
                let line = line_of(src, m.start());
                if let Some(row) = enclosing(ctx, line, &[NodeKind::Class]) {
                    out.sites.push(SiteFact {
                        pass: PASS_REACT_RENDER,
                        role: 1,
                        key_kind: 2,
                        key_a: String::new(),
                        key_b: String::new(),
                        node_row: row,
                        line,
                        payload: String::new(),
                    });
                }
            }
            for m in set_state_re().find_iter(src) {
                let line = line_of(src, m.start());
                if let Some(row) = enclosing(ctx, line, &[NodeKind::Method]) {
                    out.sites.push(SiteFact {
                        pass: PASS_REACT_RENDER,
                        role: 0,
                        key_kind: 2,
                        key_a: String::new(),
                        key_b: String::new(),
                        node_row: row,
                        line,
                        payload: String::new(),
                    });
                }
            }
        }
        if matches!(ctx.lang, Lang::Tsx | Lang::Jsx) || src.contains("</") || src.contains("/>") {
            let mut seen: std::collections::HashSet<(u32, String)> = std::collections::HashSet::new();
            let jsx_kinds = [NodeKind::Function, NodeKind::Method, NodeKind::Component, NodeKind::Class, NodeKind::Variable, NodeKind::Constant];
            for m in jsx_re().captures_iter(src) {
                let name = m[1].to_owned();
                if name == "Fragment" {
                    continue;
                }
                let line = line_of(src, m.get(0).unwrap().start());
                let Some(row) = enclosing(ctx, line, &jsx_kinds) else { continue };
                if seen.insert((row, name.clone())) {
                    out.sites.push(SiteFact {
                        pass: PASS_JSX,
                        role: 0,
                        key_kind: 0,
                        key_a: name,
                        key_b: String::new(),
                        node_row: row,
                        line,
                        payload: String::new(),
                    });
                }
            }
        }
    }
}
