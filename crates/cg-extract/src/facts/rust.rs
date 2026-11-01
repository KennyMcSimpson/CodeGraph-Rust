//! Facts for Rust: `use` trees flattened to (module path, item) pairs, `pub use` re-exports,
//! function / closure / impl / struct scopes, parameters, `let` bindings and struct fields.

use super::util::*;
use super::*;

pub fn extract(cx: &mut Cx, root: Node) {
    // Conditional item wrappers such as Tokio's cfg_rt! hide `pub use` from
    // tree-sitter. Reparse a position-preserving view with just those wrappers
    // blanked out. Never expand arbitrary macros or change the kernel's nodes.
    if let Some(tree) = conditional_items(cx.src, root) {
        walk(cx, tree.root_node(), 0);
    } else {
        walk(cx, root, 0);
    }
}

fn conditional_items(src: &str, root: Node) -> Option<Tree> {
    use std::time::{Duration, Instant};
    use tree_sitter::{ParseOptions, Parser};

    if src.len() > 1024 * 1024 || !src.contains("cfg_") {
        return None;
    }
    fn wrapper(name: &str) -> bool {
        name.starts_with("cfg_") && name != "cfg_if" && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    }
    fn blank(bytes: &mut [u8]) {
        for b in bytes {
            if *b != b'\n' && *b != b'\r' {
                *b = b' ';
            }
        }
    }
    fn collect(n: Node, src: &str, ranges: &mut Vec<(usize, usize)>, depth: usize) {
        if depth > 128 || n.kind() == "macro_definition" {
            return;
        }
        if matches!(n.kind(), "macro_invocation" | "token_tree") {
            let mut c = n.walk();
            let children: Vec<_> = n.children(&mut c).collect();
            let mut i = 0;
            while i < children.len() {
                if i + 2 < children.len() && children[i + 1].kind() == "!" && children[i + 2].kind() == "token_tree" {
                    let name = &src[children[i].byte_range()];
                    let body = children[i + 2];
                    if wrapper(name) && src.as_bytes().get(body.start_byte()) == Some(&b'{') {
                        ranges.push((children[i].start_byte(), body.start_byte() + 1));
                        ranges.push((body.end_byte() - 1, body.end_byte()));
                        collect(body, src, ranges, depth + 1);
                    }
                    // An unknown macro's tokens are not ordinary Rust items.
                    i += 3;
                } else {
                    collect(children[i], src, ranges, depth + 1);
                    i += 1;
                }
            }
        } else {
            for ch in kids(n) {
                collect(ch, src, ranges, depth + 1);
            }
        }
    }
    let mut ranges = Vec::new();
    collect(root, src, &mut ranges, 0);
    if ranges.is_empty() {
        return None;
    }
    let mut masked = src.as_bytes().to_vec();
    for (start, end) in ranges {
        blank(&mut masked[start..end]);
    }
    let mut parser = Parser::new();
    parser.set_language(&cg_kernel::hooks::language("rust")?).ok()?;
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut cancel = |_: &tree_sitter::ParseState| Instant::now() >= deadline;
    let tree =
        parser.parse_with_options(&mut |offset, _| masked.get(offset..).unwrap_or_default(), None, Some(ParseOptions::new().progress_callback(&mut cancel)))?;
    // Non-item wrappers (cfg_if!, DSLs, malformed syntax) are left opaque.
    (!tree.root_node().has_error()).then_some(tree)
}

fn walk_children(cx: &mut Cx, n: Node, scope: u32) {
    for ch in kids(n) {
        walk(cx, ch, scope);
    }
}

fn walk(cx: &mut Cx, n: Node, scope: u32) {
    match n.kind() {
        "use_declaration" => use_decl(cx, n, scope),
        "function_item" | "function_signature_item" => function(cx, n, scope),
        "closure_expression" => closure(cx, n, scope),
        "impl_item" | "trait_item" => {
            let sc = cx.push_scope(ScopeKind::Class, scope, n);
            walk_children(cx, n, sc);
        }
        "struct_item" => struct_item(cx, n, scope),
        "let_declaration" => {
            let_decl(cx, n, scope);
            walk_children(cx, n, scope);
        }
        "for_expression" => {
            if let Some(p) = n.child_by_field_name("pattern") {
                bind_pattern(cx, p, scope, BindKind::Local);
            }
            walk_children(cx, n, scope);
        }
        "let_condition" => {
            if let Some(p) = n.child_by_field_name("pattern") {
                bind_pattern(cx, p, scope, BindKind::Local);
            }
            walk_children(cx, n, scope);
        }
        _ => walk_children(cx, n, scope),
    }
}

fn rust_type_name(cx: &Cx, n: Node) -> String {
    match n.kind() {
        "type_identifier" | "scoped_type_identifier" => cx.text(n).to_owned(),
        "generic_type" => {
            let base = n.child_by_field_name("type").map(|c| rust_type_name(cx, c)).unwrap_or_default();
            if matches!(base.rsplit("::").next(), Some("Result" | "Option")) {
                cx.text(n).to_owned()
            } else {
                base
            }
        }
        "reference_type" | "pointer_type" => n.child_by_field_name("type").map(|c| rust_type_name(cx, c)).unwrap_or_default(),
        "abstract_type" | "dynamic_type" | "bounded_type" => {
            n.child_by_field_name("trait").or_else(|| n.named_child(0)).map(|c| rust_type_name(cx, c)).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn bind_pattern(cx: &mut Cx, p: Node, scope: u32, kind: BindKind) {
    match p.kind() {
        "identifier" => {
            let nm = cx.text(p).to_owned();
            let line = cx.line(p);
            cx.add_binding(scope, &nm, kind, "", "", line);
        }
        "mut_pattern" | "reference_pattern" | "tuple_pattern" | "tuple_struct_pattern" | "slice_pattern" | "struct_pattern" | "ref_pattern" => {
            for c in kids(p) {
                bind_pattern(cx, c, scope, kind);
            }
        }
        "field_pattern" => {
            if let Some(c) = p.child_by_field_name("pattern").or_else(|| p.child_by_field_name("name")) {
                bind_pattern(cx, c, scope, kind);
            }
        }
        _ => {}
    }
}

fn value_info(cx: &Cx, v: Node) -> String {
    value_path(cx, v, 0)
}

fn value_path(cx: &Cx, v: Node, depth: u8) -> String {
    if depth >= 16 || v.end_byte() - v.start_byte() > 4096 {
        return String::new();
    }
    match v.kind() {
        "identifier" | "scoped_identifier" => cx.text(v).to_owned(),
        "generic_function" => v.child_by_field_name("function").map(|f| value_path(cx, f, depth + 1)).unwrap_or_default(),
        "struct_expression" => v.child_by_field_name("name").map(|n| cx.text(n).to_owned()).unwrap_or_default(),
        "call_expression" => {
            v.child_by_field_name("function").map(|f| value_path(cx, f, depth + 1)).filter(|p| !p.is_empty()).map(|p| format!("{p}()")).unwrap_or_default()
        }
        "field_expression" => {
            let base = v.child_by_field_name("value").map(|v| value_path(cx, v, depth + 1)).unwrap_or_default();
            match v.child_by_field_name("field") {
                Some(field) if !base.is_empty() => format!("{base}.{}", cx.text(field)),
                _ => String::new(),
            }
        }
        "try_expression" => v.named_child(0).map(|c| value_path(cx, c, depth + 1)).filter(|p| !p.is_empty()).map(|p| format!("{p}?")).unwrap_or_default(),
        "await_expression" | "reference_expression" | "parenthesized_expression" => v.named_child(0).map(|c| value_path(cx, c, depth + 1)).unwrap_or_default(),
        _ => String::new(),
    }
}

fn let_decl(cx: &mut Cx, n: Node, scope: u32) {
    let Some(p) = n.child_by_field_name("pattern") else { return };
    let ty = n.child_by_field_name("type").map(|t| rust_type_name(cx, t)).unwrap_or_default();
    let init = n.child_by_field_name("value").map(|v| value_info(cx, v)).unwrap_or_default();
    let line = cx.line(n);
    let target = if p.kind() == "mut_pattern" { p.named_child(0).unwrap_or(p) } else { p };
    if target.kind() == "identifier" {
        let nm = cx.text(target).to_owned();
        cx.add_binding(scope, &nm, BindKind::Local, &ty, &init, line);
    } else {
        bind_pattern(cx, p, scope, BindKind::Local);
    }
}

fn params(cx: &mut Cx, list: Node, sc: u32) {
    for p in kids(list) {
        match p.kind() {
            "self_parameter" => {
                let line = cx.line(p);
                cx.add_binding(sc, "self", BindKind::Param, "Self", "", line);
            }
            "parameter" => {
                let ty = p.child_by_field_name("type").map(|t| rust_type_name(cx, t)).unwrap_or_default();
                if let Some(pat) = p.child_by_field_name("pattern") {
                    let target = if pat.kind() == "mut_pattern" { pat.named_child(0).unwrap_or(pat) } else { pat };
                    if target.kind() == "identifier" {
                        let nm = cx.text(target).to_owned();
                        let line = cx.line(p);
                        cx.add_binding(sc, &nm, BindKind::Param, &ty, "", line);
                    } else {
                        bind_pattern(cx, pat, sc, BindKind::Param);
                    }
                }
            }
            "identifier" => {
                let nm = cx.text(p).to_owned();
                let line = cx.line(p);
                cx.add_binding(sc, &nm, BindKind::Param, "", "", line);
            }
            _ => {}
        }
    }
}

fn function(cx: &mut Cx, n: Node, scope: u32) {
    if let (Some(name), Some(ret)) = (n.child_by_field_name("name"), n.child_by_field_name("return_type")) {
        if let Some(row) = cx.row_at(cx.start(n)) {
            cx.facts.bindings.push(BindingFact {
                scope,
                name: cx.text(name).to_owned(),
                kind: BindKind::ReturnType,
                decl_type: cx.text(ret).to_owned(),
                init_ctor: String::new(),
                line: cx.line(n),
                node_row: row,
            });
        }
    }
    if cx.facts.scopes[scope as usize].kind == ScopeKind::Function {
        if let Some(name) = n.child_by_field_name("name") {
            let nm = cx.text(name).to_owned();
            cx.add_binding(scope, &nm, BindKind::LocalFn, "", "", 0);
        }
    }
    let sc = cx.push_scope(ScopeKind::Function, scope, n);
    if let Some(p) = n.child_by_field_name("parameters") {
        params(cx, p, sc);
    }
    if let Some(body) = n.child_by_field_name("body") {
        walk(cx, body, sc);
    }
}

fn closure(cx: &mut Cx, n: Node, scope: u32) {
    let sc = cx.push_scope(ScopeKind::Function, scope, n);
    if let Some(p) = n.child_by_field_name("parameters") {
        params(cx, p, sc);
    }
    if let Some(body) = n.child_by_field_name("body") {
        walk(cx, body, sc);
    }
}

fn struct_item(cx: &mut Cx, n: Node, scope: u32) {
    let sc = cx.push_scope(ScopeKind::Class, scope, n);
    let Some(body) = n.child_by_field_name("body") else { return };
    for f in kids(body) {
        if f.kind() == "field_declaration" {
            let ty = f.child_by_field_name("type").map(|t| rust_type_name(cx, t)).unwrap_or_default();
            if let Some(nm) = f.child_by_field_name("name") {
                let t = cx.text(nm).to_owned();
                let line = cx.line(f);
                cx.add_binding(sc, &t, BindKind::Field, &ty, "", line);
            }
        }
    }
}

// ----------------------------------------------------------------------------- use trees

fn use_decl(cx: &mut Cx, n: Node, scope: u32) {
    let public = kids(n).any(|c| c.kind() == "visibility_modifier");
    let Some(arg) = n.child_by_field_name("argument") else { return };
    let line = cx.line(n);
    let mut out = Vec::new();
    flatten(cx, arg, &[], &mut out);
    for (path, local, wildcard) in out {
        // path = full item path segments; the item is the last one
        if wildcard {
            let spec = path.join("::");
            cx.add_import(ImportFact {
                local: "*".into(),
                imported: "*".into(),
                spec: spec.clone(),
                kind: ImportKind::Namespace,
                type_only: false,
                scope,
                line,
            });
            if public {
                cx.facts.exports.push(ExportFact { name: "*".into(), node_row: NONE, from_spec: spec, from_name: "*".into(), star: true });
            }
            continue;
        }
        let Some((item, module)) = path.split_last() else { continue };
        let spec = module.join("::");
        let local = local.unwrap_or_else(|| item.clone());
        cx.add_import(ImportFact { local: local.clone(), imported: item.clone(), spec: spec.clone(), kind: ImportKind::Named, type_only: false, scope, line });
        if public {
            cx.facts.exports.push(ExportFact { name: local, node_row: NONE, from_spec: spec, from_name: item.clone(), star: false });
        }
    }
}

/// (full path, alias, is_wildcard) triples of a use tree.
fn flatten(cx: &Cx, n: Node, prefix: &[String], out: &mut Vec<(Vec<String>, Option<String>, bool)>) {
    let segs = |node: Node| -> Vec<String> { cx.text(node).split("::").map(|s| s.trim().to_owned()).collect() };
    match n.kind() {
        "identifier" | "crate" | "super" | "self" | "scoped_identifier" => {
            let mut p = prefix.to_vec();
            let s = segs(n);
            if s.len() == 1 && s[0] == "self" && !p.is_empty() {
                // `use a::b::{self}` imports module `b`
            } else {
                p.extend(s);
            }
            out.push((p, None, false));
        }
        "use_as_clause" => {
            let alias = n.child_by_field_name("alias").map(|a| cx.text(a).to_owned());
            if let Some(path) = n.child_by_field_name("path") {
                let mut p = prefix.to_vec();
                let s = segs(path);
                if !(s.len() == 1 && s[0] == "self" && !p.is_empty()) {
                    p.extend(s);
                }
                out.push((p, alias, false));
            }
        }
        "scoped_use_list" => {
            let mut p = prefix.to_vec();
            if let Some(path) = n.child_by_field_name("path") {
                p.extend(segs(path));
            }
            if let Some(list) = n.child_by_field_name("list") {
                for c in kids(list) {
                    if c.is_named() {
                        flatten(cx, c, &p, out);
                    }
                }
            }
        }
        "use_list" => {
            for c in kids(n) {
                if c.is_named() {
                    flatten(cx, c, prefix, out);
                }
            }
        }
        "use_wildcard" => {
            let mut p = prefix.to_vec();
            if let Some(path) = n.named_child(0) {
                p.extend(segs(path));
            }
            out.push((p, None, true));
        }
        _ => {}
    }
}
