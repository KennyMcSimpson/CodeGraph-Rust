//! Go structural interfaces (upstream `goImplementsEdges`, with #3185's type check): a type
//! implements an interface when its method set, embedded types' methods included, has every
//! method the interface asks for with the same parameter and result counts and types. An
//! unexported method name only matches within its package. A type is compared by its name with
//! the package qualifier left off (the two declarations may sit in different packages); a true
//! alias, a type parameter and a `func`/`struct`/`interface` literal type rule nothing out.

use super::sites::{PASS_GO_TYPES, ROLE_GO_ALIAS, ROLE_GO_GENERIC};
use super::{Out, MAX_GO_IMPLEMENTERS};
use crate::engine::Engine;
use crate::project::parent_dir;
use cg_core::{EdgeKind, Lang, NodeIdx, NodeKind};
use rustc_hash::{FxHashMap, FxHashSet};

/// A method's parameter and result types, each comparable or `None` (open).
type Sig = Vec<Option<String>>;

/// The top-level items of the bracketed list opening at `text[open]`, and where it closes.
fn list_items(text: &str, open: usize) -> Option<(Vec<&str>, usize)> {
    let b = text.as_bytes();
    let mut items = Vec::new();
    let (mut depth, mut start) = (0i32, open + 1);
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' | b'`' | b'\'' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if q != b'`' && b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    let last = text[start..i].trim();
                    if !last.is_empty() {
                        items.push(last);
                    }
                    return Some((items, i));
                }
            }
            b',' if depth == 1 => {
                let item = text[start..i].trim();
                if !item.is_empty() {
                    items.push(item);
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `(ctx context.Context, a, b int) (int, error)` -> parameters and results.
fn signature_parts(sig: &str) -> Option<(Vec<&str>, Vec<&str>)> {
    let sig = sig.trim();
    if !sig.starts_with('(') {
        return None;
    }
    let (params, end) = list_items(sig, 0)?;
    let rest = sig[end + 1..].trim();
    if !rest.starts_with('(') {
        return Some((params, if rest.is_empty() { vec![] } else { vec![rest] }));
    }
    let (results, end) = list_items(rest, 0)?;
    rest[end + 1..].trim().is_empty().then_some((params, results))
}

const TYPE_WORDS: &[&str] = &["func", "chan", "map", "struct", "interface"];

/// The types of a list's items: `a, b int` gives `a` the type `int`; a list with no names is
/// a list of types.
fn item_types<'a>(items: &[&'a str]) -> Vec<&'a str> {
    let named: Vec<Option<&str>> = items
        .iter()
        .map(|item| {
            let (name, ty) = item.split_once(char::is_whitespace)?;
            let ok = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && name.chars().all(|c| c.is_alphanumeric() || c == '_');
            (ok && !TYPE_WORDS.contains(&name)).then(|| ty.trim())
        })
        .collect();
    if named.iter().all(Option::is_none) {
        return items.to_vec();
    }
    let mut out = vec![""; items.len()];
    let mut ty = "";
    for i in (0..items.len()).rev() {
        if let Some(t) = named[i] {
            ty = t;
        }
        out[i] = ty;
    }
    out
}

/// A type as two declarations can compare it, or `None` for one that holds a list of its own.
fn comparable(written: &str) -> Option<String> {
    let flat: String = written.chars().filter(|c| !c.is_whitespace()).collect();
    let flat = flat.replace("interface{}", "any");
    if flat.contains("func(") || flat.contains("struct{") || flat.contains("interface{") {
        return None;
    }
    // drop package qualifiers: `*http.Request` -> `*Request`
    let mut out = String::with_capacity(flat.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String, next: Option<char>| {
        if next == Some('.') {
            word.clear();
        } else {
            out.push_str(match word.as_str() {
                "uint8" => "byte",
                "int32" => "rune",
                w => w,
            });
            word.clear();
        }
    };
    let chars: Vec<char> = flat.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            let qualifier = c == '.' && !word.is_empty();
            flush(&mut word, &mut out, Some(c));
            if !qualifier {
                out.push(c);
            }
        }
        i += 1;
    }
    flush(&mut word, &mut out, None);
    Some(out)
}

fn method_types(sig: &str) -> Option<(usize, usize, Sig)> {
    let (params, results) = signature_parts(sig)?;
    let types = item_types(&params).into_iter().chain(item_types(&results)).map(comparable).collect();
    Some((params.len(), results.len(), types))
}

/// What a method matches an interface method by: its name, plus its package when unexported.
fn method_key(view: &cg_store::View, m: NodeIdx) -> String {
    let node = view.node(m);
    let name = node.name();
    if name.chars().next().is_some_and(char::is_uppercase) {
        name.to_owned()
    } else {
        format!("{}\0{name}", parent_dir(view.file(node.file()).path()))
    }
}

pub(super) fn go_implements(e: &Engine<'_>, out: &mut Out) -> Vec<(NodeIdx, NodeIdx)> {
    let view = e.view;
    let mut implementers: Vec<NodeIdx> = Vec::new();
    let mut interfaces: Vec<NodeIdx> = Vec::new();
    let mut is_go_type: FxHashSet<NodeIdx> = FxHashSet::default();
    // type name -> declaring nodes, per package directory, for receiver methods of other files
    let mut types_in_dir: FxHashMap<(&str, &str), NodeIdx> = FxHashMap::default();
    let mut methods_of: FxHashMap<NodeIdx, Vec<NodeIdx>> = FxHashMap::default();
    // names standing for some other type: true aliases and type parameters
    let mut open_names: FxHashSet<&str> = FxHashSet::default();
    let mut receiver_methods: Vec<NodeIdx> = Vec::new();
    for f in view.files() {
        if view.file(f).lang() != Lang::Go {
            continue;
        }
        let dir = parent_dir(view.file(f).path());
        for s in view.sites_of(f).iter().filter(|s| s.pass == PASS_GO_TYPES) {
            match s.role {
                ROLE_GO_ALIAS => {
                    open_names.insert(view.text_str(cg_core::StrId(s.key_a)));
                }
                ROLE_GO_GENERIC => open_names.extend(view.text_str(s.payload).split(',').filter(|p| !p.is_empty())),
                _ => {}
            }
        }
        for n in view.nodes_in_file(f) {
            let n = NodeIdx(n);
            let node = view.node(n);
            match node.kind() {
                NodeKind::Interface => {
                    interfaces.push(n);
                    is_go_type.insert(n);
                    types_in_dir.entry((dir, node.name())).or_insert(n);
                }
                NodeKind::Struct | NodeKind::Class | NodeKind::TypeAlias => {
                    implementers.push(n);
                    is_go_type.insert(n);
                    types_in_dir.entry((dir, node.name())).or_insert(n);
                }
                NodeKind::Method | NodeKind::Function => {
                    if let Some((tq, name)) = node.qname().rsplit_once("::") {
                        if name == node.name() && !tq.contains("::") {
                            receiver_methods.push(n);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // a type's own methods: those in its body (interfaces) and the receiver methods its
    // package declares anywhere
    for &t in &interfaces {
        let own: Vec<NodeIdx> =
            view.out_edges(t).filter(|ed| ed.kind == EdgeKind::Contains && view.node(ed.dst).kind() == NodeKind::Method).map(|ed| ed.dst).collect();
        methods_of.insert(t, own);
    }
    for &m in &receiver_methods {
        let node = view.node(m);
        let tq = node.qname().rsplit_once("::").map_or("", |x| x.0);
        let dir = parent_dir(view.file(node.file()).path());
        if let Some(&t) = types_in_dir.get(&(dir, tq)) {
            if view.node(t).kind() != NodeKind::Interface {
                methods_of.entry(t).or_default().push(m);
            }
        }
    }
    // embedding: Go records it as `extends`
    let embeds = |t: NodeIdx| -> Vec<NodeIdx> { e.direct_supers(t).iter().copied().filter(|s| is_go_type.contains(s)).collect() };
    let closure = |t: NodeIdx| -> Vec<NodeIdx> {
        let mut seen = vec![t];
        let mut i = 0;
        while i < seen.len() && seen.len() < 64 {
            for s in embeds(seen[i]) {
                if !seen.contains(&s) {
                    seen.push(s);
                }
            }
            i += 1;
        }
        seen
    };
    // method set: key -> the signatures declared for it (None: a signature that doesn't read)
    type MethodSet = FxHashMap<String, Vec<Option<(usize, usize, Sig)>>>;
    let method_set = |t: NodeIdx| -> MethodSet {
        let mut set: MethodSet = FxHashMap::default();
        for ty in closure(t) {
            for &m in methods_of.get(&ty).map(Vec::as_slice).unwrap_or(&[]) {
                set.entry(method_key(view, m)).or_default().push(view.node(m).signature().and_then(method_types));
            }
        }
        set
    };
    let open = |t: &Option<String>| -> bool {
        match t {
            None => true,
            Some(t) => t
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| open_names.contains(w) || (w.len() == 1 && w.chars().all(|c| c.is_ascii_uppercase()))),
        }
    };
    let fits = |have: &Option<(usize, usize, Sig)>, want: &Option<(usize, usize, Sig)>| -> bool {
        let (Some(h), Some(w)) = (have, want) else { return true };
        h.0 == w.0 && h.1 == w.1 && h.2.iter().zip(&w.2).all(|(a, b)| a == b || open(a) || open(b))
    };
    let sets: Vec<(NodeIdx, MethodSet)> = implementers.iter().map(|&t| (t, method_set(t))).filter(|(_, s)| !s.is_empty()).collect();
    let mut pairs = Vec::new();
    for &iface in &interfaces {
        let want = method_set(iface);
        if want.is_empty() {
            continue; // `any` would match everything
        }
        let mut direct = Vec::new();
        let mut promoted = Vec::new();
        for (t, have) in &sets {
            let covers = want.iter().all(|(key, wanted)| have.get(key).is_some_and(|got| wanted.iter().any(|w| got.iter().any(|g| fits(g, w)))));
            if !covers || embeds(*t).contains(&iface) {
                continue;
            }
            let own_covers = want.keys().all(|key| methods_of.get(t).is_some_and(|ms| ms.iter().any(|m| &method_key(view, *m) == key)));
            if own_covers {
                direct.push(*t);
            } else {
                promoted.push(*t);
            }
        }
        for t in direct.into_iter().chain(promoted).take(MAX_GO_IMPLEMENTERS) {
            let node = view.node(t);
            let at = format!("{}:{}", view.file(node.file()).path(), node.start_line());
            out.push(
                EdgeKind::Implements,
                t,
                iface,
                node.start_line(),
                "go-implements",
                Some(at),
                Some(("via", view.node(iface).name())),
                super::order_of("go-implements"),
            );
            pairs.push((t, iface));
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_compare_by_count_and_type() {
        let (p, r, t) = method_types("(ctx context.Context, a, b int, rest ...string) (int, error)").unwrap();
        assert_eq!((p, r), (4, 2));
        assert_eq!(t, vec![Some("Context".into()), Some("int".into()), Some("int".into()), Some("...string".into()), Some("int".into()), Some("error".into())]);
        assert_eq!(method_types("() error").unwrap().2, vec![Some("error".into())]);
        assert_eq!(method_types("(*http.Request) bool").unwrap().2, vec![Some("*Request".into()), Some("bool".into())]);
        assert_eq!(method_types("(f func(int) error)").unwrap().2, vec![None]);
        assert_eq!(comparable("map[string]uint8").as_deref(), Some("map[string]byte"));
        assert_eq!(comparable("interface{}").as_deref(), Some("any"));
    }
}
