//! Post-processing that upstream does in TypeScript after the kernel call (design doc 4.3).

use cg_core::{text, EdgeKind, Lang, RefKind};

/// A node created outside the kernel's walkers (framework `extract` hooks: routes, components).
/// Its row is `kernel_node_count + index in FileBatch::extra_nodes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraNode {
    pub kind: cg_core::NodeKind,
    pub name: String,
    pub qname: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub end_col: u32,
    pub signature: Option<String>,
    /// Row of the containing node (0 = the file node).
    pub parent_row: u32,
    pub exported: Option<bool>,
    /// First 16 bytes of sha256(path:kind:name:line), as the kernel computes node ids.
    pub digest: [u8; 16],
}

/// A reference found outside the kernel's walkers (a CommonJS `require`, a framework hook).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraRef {
    /// Kernel node row the reference hangs off; 0 is the file node.
    pub from_row: u32,
    pub kind: RefKind,
    pub name: String,
    pub line: u32,
    pub col: u32,
}

/// Blank comments with spaces, keeping every offset and newline (string literals survive:
/// the specifier lives in one). Mirrors upstream `stripCommentsForRegex` for JS/TS.
pub fn strip_js_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    if b[i] != b'\r' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    if b[i] != b'\n' && b[i] != b'\r' && b[i] & 0xC0 != 0x80 {
                        out[i] = b' ';
                    } else if b[i] & 0xC0 == 0x80 {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                if i < b.len() {
                    out[i] = b' ';
                    if i + 1 < b.len() {
                        out[i + 1] = b' ';
                    }
                    i += 2;
                }
            }
            q @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    } else if b[i] == b'\n' && q != b'`' {
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    // Multi-byte characters inside block comments were replaced byte by byte with spaces,
    // which keeps the string valid UTF-8 (all ASCII).
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn is_local_specifier(s: &str) -> bool {
    s.starts_with("./")
        || s.starts_with("../")
        || s == "."
        || s == ".."
        || s.starts_with('/')
        || s.starts_with("@/")
        || s.starts_with("~/")
        || s.starts_with('#')
}

/// `require('./x')` as an `imports` reference from the file (upstream `commonJsRequireRefs`).
pub fn commonjs_require_refs(src: &str, lang: Lang) -> Vec<ExtraRef> {
    if !lang.is_js_family() || !src.contains("require") {
        return Vec::new();
    }
    let safe = strip_js_comments(src);
    let b = safe.as_bytes();
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(rel) = safe[from..].find("require") {
        let at = from + rel;
        from = at + 7;
        // not a member's (`foo.require(...)`), a property, or part of a longer identifier
        if at > 0 {
            let p = b[at - 1];
            if p.is_ascii_alphanumeric() || p == b'_' || p == b'$' || p == b'.' {
                continue;
            }
        }
        let mut j = at + 7;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'(') {
            continue;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let Some(&q) = b.get(j) else { continue };
        if !matches!(q, b'\'' | b'"' | b'`') {
            continue;
        }
        let s0 = j + 1;
        let mut k = s0;
        while k < b.len() && b[k] != q && b[k] != b'\n' && !(q == b'`' && b[k] == b'$') {
            k += 1;
        }
        if b.get(k) != Some(&q) {
            continue;
        }
        let spec = safe[s0..k].trim();
        let mut m = k + 1;
        while m < b.len() && b[m].is_ascii_whitespace() {
            m += 1;
        }
        if b.get(m) != Some(&b')') || spec.is_empty() {
            continue;
        }
        if !is_local_specifier(spec) || seen.iter().any(|s| s == spec) {
            continue;
        }
        let line_start = safe[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        // `import x = require('y')` is an import statement already
        let before = &safe[line_start..at];
        if import_equals_tail(before) {
            continue;
        }
        seen.push(spec.to_owned());
        let line = safe[..at].bytes().filter(|c| *c == b'\n').count() as u32 + 1;
        out.push(ExtraRef {
            from_row: 0,
            kind: RefKind::Edge(EdgeKind::Imports),
            name: spec.to_owned(),
            line,
            col: text::col16(&safe[line_start..], at - line_start),
        });
    }
    out
}

/// Does `before` end with `import [type] name =` ?
fn import_equals_tail(before: &str) -> bool {
    let t = before.trim_end();
    let Some(t) = t.strip_suffix('=') else { return false };
    let t = t.trim_end();
    let name_end = t.len();
    let name_start = t
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .map(|i| i + 1)
        .unwrap_or(0);
    if name_start == name_end {
        return false;
    }
    let head = t[..name_start].trim_end();
    let head = head.strip_suffix("type").map(str::trim_end).unwrap_or(head);
    head.ends_with("import") && head[..head.len() - 6].chars().next_back().map_or(true, |c| !(c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires() {
        let src = "const a = require('./a');\n// require('./commented')\nconst p = require('pkg');\nfoo.require('./x');\nimport z = require('./z');\nconst b = require(`./b`);\nrequire( \"../c\" )";
        let refs = commonjs_require_refs(src, Lang::JavaScript);
        let names: Vec<_> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["./a", "./b", "../c"]);
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[0].col, 10);
        assert!(commonjs_require_refs(src, Lang::Python).is_empty());
    }

    #[test]
    fn strip_keeps_offsets() {
        let s = "a /* é */ b // c\nd '//x'";
        let t = strip_js_comments(s);
        assert_eq!(t.len(), s.len());
        assert!(t.contains("'//x'"));
        assert!(!t.contains("// c"));
    }
}

/// A synthesis "site" found while the source was in memory (design doc 7.2): a pass-specific
/// observation (a `.emit('x')` call, a `setState` call, ...) keyed for later linking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteFact {
    pub pass: u16,
    pub role: u8,
    /// 0 = name, 1 = file, 2 = node, 3 = pair (see `SiteKey` in cg-store).
    pub key_kind: u8,
    pub key_a: String,
    pub key_b: String,
    /// Kernel node row the site belongs to (`NONE` = the file).
    pub node_row: u32,
    pub line: u32,
    pub payload: String,
}

/// What a file hook may add to a file's extraction result.
#[derive(Default)]
pub struct HookOut {
    pub nodes: Vec<ExtraNode>,
    pub refs: Vec<ExtraRef>,
    pub sites: Vec<SiteFact>,
}

pub struct HookCtx<'a> {
    pub rel: &'a str,
    pub lang: Lang,
    pub src: &'a str,
    pub rows: crate::rows::FileRows<'a>,
    pub facts: &'a crate::facts::Facts,
}

/// A per-file extraction add-on: framework `extract` hooks (route nodes, middleware refs) and
/// synthesis-pass site scanners. Hooks must be deterministic and panic-free on any input.
pub trait FileHook: Send + Sync {
    fn name(&self) -> &'static str;
    fn applies(&self, lang: Lang, rel: &str) -> bool;
    /// Cheap literal prefilter: skip `run` unless the source contains one of these.
    fn triggers(&self) -> &'static [&'static str] {
        &[]
    }
    fn run(&self, ctx: &HookCtx<'_>, out: &mut HookOut);
}

/// First 16 bytes of sha256(`path:kind:name:line`), as the kernel computes node ids.
pub fn node_digest(path: &str, kind: cg_core::NodeKind, name: &str, line: u32) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(path.as_bytes());
    h.update(b":");
    h.update(kind.as_str().as_bytes());
    h.update(b":");
    h.update(name.as_bytes());
    h.update(b":");
    h.update(line.to_string().as_bytes());
    let d = h.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}
