//! Facts for Rust: `use` trees flattened to (module path, item) pairs, `pub use` re-exports,
//! function / closure / impl / struct scopes, parameters, `let` bindings and struct fields.

use super::util::*;
use super::*;

pub fn extract(cx: &mut Cx, root: Node) {
    walk(cx, root, 0);
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
        "generic_type" => n.child_by_field_name("type").map(|c| rust_type_name(cx, c)).unwrap_or_default(),
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
    match v.kind() {
        "struct_expression" => v.child_by_field_name("name").map(|n| cx.text(n).to_owned()).unwrap_or_default(),
        "call_expression" => match v.child_by_field_name("function") {
            Some(f) if matches!(f.kind(), "identifier" | "scoped_identifier") => format!("{}()", cx.text(f)),
            _ => String::new(),
        },
        "try_expression" | "await_expression" | "reference_expression" | "parenthesized_expression" => {
            v.named_child(0).map(|c| value_info(cx, c)).unwrap_or_default()
        }
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
