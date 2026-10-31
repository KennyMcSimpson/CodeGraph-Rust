//! Facts for TypeScript / TSX / JavaScript / JSX: ES imports and exports, CommonJS `require`,
//! function / class scopes, parameters, locals and class fields with their types.

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
        "import_statement" => import_stmt(cx, n, scope),
        "export_statement" => {
            export_stmt(cx, n);
            walk_children(cx, n, scope);
        }
        "function_declaration" | "generator_function_declaration" => {
            if scope != 0 {
                if let Some(name) = n.child_by_field_name("name") {
                    let nm = cx.text(name).to_owned();
                    cx.add_binding(scope, &nm, BindKind::LocalFn, "", "", 0);
                }
            }
            function(cx, n, scope);
        }
        "function_expression" | "function" | "generator_function" | "arrow_function" | "method_definition" => {
            function(cx, n, scope)
        }
        "class_declaration" | "abstract_class_declaration" | "class" => class(cx, n, scope),
        "lexical_declaration" | "variable_declaration" => var_decl(cx, n, scope),
        "for_in_statement" => {
            if let Some(l) = n.child_by_field_name("left") {
                bind_pattern(cx, l, scope, BindKind::Local, "", "");
            }
            walk_children(cx, n, scope);
        }
        "catch_clause" => {
            if let Some(p) = n.child_by_field_name("parameter") {
                bind_pattern(cx, p, scope, BindKind::Local, "", "");
            }
            walk_children(cx, n, scope);
        }
        _ => walk_children(cx, n, scope),
    }
}

fn names_in_pattern(cx: &Cx, n: Node, out: &mut Vec<String>) {
    match n.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(cx.text(n).to_owned()),
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(l) = n.child_by_field_name("left") {
                names_in_pattern(cx, l, out);
            }
        }
        "pair_pattern" => {
            if let Some(v) = n.child_by_field_name("value") {
                names_in_pattern(cx, v, out);
            }
        }
        "object_pattern" | "array_pattern" | "rest_pattern" | "parenthesized_expression" => {
            for ch in kids(n) {
                names_in_pattern(cx, ch, out);
            }
        }
        _ => {}
    }
}

fn bind_pattern(cx: &mut Cx, pat: Node, scope: u32, kind: BindKind, decl_type: &str, init: &str) {
    let mut names = Vec::new();
    names_in_pattern(cx, pat, &mut names);
    let single = names.len() == 1 && pat.kind() == "identifier";
    let line = cx.line(pat);
    for nm in names {
        if single {
            cx.add_binding(scope, &nm, kind, decl_type, init, line);
        } else {
            cx.add_binding(scope, &nm, kind, "", "", line);
        }
    }
}

/// Type name from a type node (`Foo`, `Foo<T>`, `ns.Foo`, `Foo | null`); empty for builtins and
/// structural types.
fn ts_type_name(cx: &Cx, n: Node) -> String {
    match n.kind() {
        "type_annotation" => n.named_child(0).map(|c| ts_type_name(cx, c)).unwrap_or_default(),
        "type_identifier" | "identifier" => cx.text(n).to_owned(),
        "nested_type_identifier" | "member_expression" => cx.text(n).to_owned(),
        "generic_type" => n.child_by_field_name("name").map(|c| ts_type_name(cx, c)).unwrap_or_default(),
        "parenthesized_type" | "readonly_type" | "required_parameter" => {
            n.named_child(0).map(|c| ts_type_name(cx, c)).unwrap_or_default()
        }
        "union_type" | "intersection_type" => {
            for ch in kids(n) {
                if !ch.is_named() {
                    continue;
                }
                let t = ts_type_name(cx, ch);
                if !t.is_empty() && t != "null" && t != "undefined" {
                    return t;
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// (declared type, initialiser) of a value expression: `new Foo()` -> ("", "Foo"),
/// `foo()` -> ("", "foo()"), `x as Foo` -> ("Foo", "").
fn value_info(cx: &Cx, v: Node) -> (String, String) {
    match v.kind() {
        "new_expression" => {
            let c = v.child_by_field_name("constructor").map(|c| cx.text(c).to_owned()).unwrap_or_default();
            (String::new(), c)
        }
        "await_expression" | "parenthesized_expression" | "non_null_expression" => {
            v.named_child(0).map(|c| value_info(cx, c)).unwrap_or_default()
        }
        "as_expression" | "satisfies_expression" => {
            let t = v.named_child(1).map(|c| ts_type_name(cx, c)).unwrap_or_default();
            (t, String::new())
        }
        "call_expression" => match v.child_by_field_name("function") {
            Some(f) if matches!(f.kind(), "identifier" | "member_expression") => {
                (String::new(), format!("{}()", cx.text(f)))
            }
            _ => Default::default(),
        },
        _ => Default::default(),
    }
}

fn import_stmt(cx: &mut Cx, n: Node, scope: u32) {
    let Some(src) = n.child_by_field_name("source") else { return };
    let spec = unquote(cx.text(src)).to_owned();
    let line = cx.line(n);
    let stmt_type_only = kids(n).any(|c| c.kind() == "type");
    let mut any = false;
    for clause in kids(n) {
        match clause.kind() {
            "import_clause" => {
                for part in kids(clause) {
                    match part.kind() {
                        "identifier" => {
                            any = true;
                            let local = cx.text(part).to_owned();
                            cx.add_import(ImportFact {
                                local,
                                imported: "default".into(),
                                spec: spec.clone(),
                                kind: ImportKind::Default,
                                type_only: stmt_type_only,
                                scope,
                                line,
                            });
                        }
                        "named_imports" => {
                            for sp in kids(part) {
                                if sp.kind() != "import_specifier" {
                                    continue;
                                }
                                any = true;
                                let name = sp.child_by_field_name("name").map(|c| unquote(cx.text(c)).to_owned()).unwrap_or_default();
                                let alias = sp.child_by_field_name("alias").map(|c| cx.text(c).to_owned());
                                let t_only = stmt_type_only || kids(sp).any(|c| c.kind() == "type");
                                cx.add_import(ImportFact {
                                    local: alias.unwrap_or_else(|| name.clone()),
                                    imported: name,
                                    spec: spec.clone(),
                                    kind: ImportKind::Named,
                                    type_only: t_only,
                                    scope,
                                    line,
                                });
                            }
                        }
                        "namespace_import" => {
                            any = true;
                            let local = kids(part).find(|c| c.kind() == "identifier").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                            cx.add_import(ImportFact {
                                local,
                                imported: "*".into(),
                                spec: spec.clone(),
                                kind: ImportKind::Namespace,
                                type_only: stmt_type_only,
                                scope,
                                line,
                            });
                        }
                        _ => {}
                    }
                }
            }
            "import_require_clause" => {
                any = true;
                let local = kids(clause).find(|c| c.kind() == "identifier").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                cx.add_import(ImportFact {
                    local,
                    imported: "*".into(),
                    spec: spec.clone(),
                    kind: ImportKind::Require,
                    type_only: false,
                    scope,
                    line,
                });
            }
            _ => {}
        }
    }
    if !any {
        cx.add_import(ImportFact {
            local: String::new(),
            imported: "*".into(),
            spec,
            kind: ImportKind::SideEffect,
            type_only: false,
            scope,
            line,
        });
    }
}

fn export_stmt(cx: &mut Cx, n: Node) {
    let source = n.child_by_field_name("source").map(|s| unquote(cx.text(s)).to_owned());
    let is_default = kids(n).any(|c| c.kind() == "default");
    let mut had_clause = false;
    for ch in kids(n) {
        match ch.kind() {
            "export_clause" => {
                had_clause = true;
                for sp in kids(ch) {
                    if sp.kind() != "export_specifier" {
                        continue;
                    }
                    let name = sp.child_by_field_name("name").map(|c| unquote(cx.text(c)).to_owned()).unwrap_or_default();
                    let alias = sp.child_by_field_name("alias").map(|c| unquote(cx.text(c)).to_owned());
                    cx.facts.exports.push(ExportFact {
                        name: alias.unwrap_or_else(|| name.clone()),
                        node_row: NONE,
                        from_spec: source.clone().unwrap_or_default(),
                        from_name: name,
                        star: false,
                    });
                }
            }
            "namespace_export" => {
                had_clause = true;
                let alias = kids(ch).find(|c| c.kind() == "identifier").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                cx.facts.exports.push(ExportFact {
                    name: alias,
                    node_row: NONE,
                    from_spec: source.clone().unwrap_or_default(),
                    from_name: "*".into(),
                    star: false,
                });
            }
            "*" if source.is_some() && !had_clause => {
                cx.facts.exports.push(ExportFact {
                    name: "*".into(),
                    node_row: NONE,
                    from_spec: source.clone().unwrap_or_default(),
                    from_name: "*".into(),
                    star: true,
                });
            }
            _ => {}
        }
    }
    if is_default {
        // `export default Foo;` / `export default function foo() {}` / `export default class {}`
        if let Some(decl) = n.child_by_field_name("declaration") {
            let pos = cx.start(decl);
            let row = cx.row_at(pos).unwrap_or(NONE);
            cx.facts.exports.push(ExportFact { name: "default".into(), node_row: row, from_spec: String::new(), from_name: String::new(), star: false });
        } else if let Some(v) = n.child_by_field_name("value") {
            if v.kind() == "identifier" {
                let from_name = cx.text(v).to_owned();
                cx.facts.exports.push(ExportFact { name: "default".into(), node_row: NONE, from_spec: String::new(), from_name, star: false });
            }
        }
    }
}

fn function(cx: &mut Cx, n: Node, scope: u32) {
    let sc = cx.push_scope(ScopeKind::Function, scope, n);
    let line = cx.line(n);
    if let Some(p) = n.child_by_field_name("parameter") {
        bind_pattern(cx, p, sc, BindKind::Param, "", ""); // single-identifier arrow parameter
    }
    if let Some(params) = n.child_by_field_name("parameters") {
        for p in kids(params) {
            match p.kind() {
                "required_parameter" | "optional_parameter" => {
                    let ty = p.child_by_field_name("type").map(|t| ts_type_name(cx, t)).unwrap_or_default();
                    if let Some(pat) = p.child_by_field_name("pattern") {
                        bind_pattern(cx, pat, sc, BindKind::Param, &ty, "");
                    }
                }
                "identifier" | "assignment_pattern" | "rest_pattern" | "object_pattern" | "array_pattern" => {
                    bind_pattern(cx, p, sc, BindKind::Param, "", "");
                }
                _ => {}
            }
        }
    }
    let _ = line;
    for ch in kids(n) {
        if matches!(ch.kind(), "formal_parameters") {
            // default values may contain closures
            for p in kids(ch) {
                walk(cx, p, sc);
            }
        } else {
            walk(cx, ch, sc);
        }
    }
}

fn class(cx: &mut Cx, n: Node, scope: u32) {
    let sc = cx.push_scope(ScopeKind::Class, scope, n);
    let Some(body) = n.child_by_field_name("body") else {
        walk_children(cx, n, scope);
        return;
    };
    for m in kids(body) {
        match m.kind() {
            "public_field_definition" | "field_definition" | "property_signature" => {
                let name = m.child_by_field_name("name").or_else(|| m.child_by_field_name("property"));
                let ty = m.child_by_field_name("type").map(|t| ts_type_name(cx, t)).unwrap_or_default();
                let (vt, init) = m.child_by_field_name("value").map(|v| value_info(cx, v)).unwrap_or_default();
                if let Some(name) = name {
                    let nm = cx.text(name).trim_start_matches('#').to_owned();
                    let line = cx.line(m);
                    cx.add_binding(sc, &nm, BindKind::Field, if ty.is_empty() { &vt } else { &ty }, &init, line);
                }
                if let Some(v) = m.child_by_field_name("value") {
                    walk(cx, v, sc);
                }
            }
            "method_definition" => {
                let is_ctor = m.child_by_field_name("name").map(|c| cx.text(c) == "constructor").unwrap_or(false);
                if is_ctor {
                    if let Some(params) = m.child_by_field_name("parameters") {
                        for p in kids(params) {
                            let has_mod = kids(p).any(|c| matches!(c.kind(), "accessibility_modifier" | "readonly"));
                            if has_mod && matches!(p.kind(), "required_parameter" | "optional_parameter") {
                                let ty = p.child_by_field_name("type").map(|t| ts_type_name(cx, t)).unwrap_or_default();
                                if let Some(pat) = p.child_by_field_name("pattern") {
                                    let nm = cx.text(pat).to_owned();
                                    let line = cx.line(p);
                                    cx.add_binding(sc, &nm, BindKind::Field, &ty, "", line);
                                }
                            }
                        }
                    }
                }
                function(cx, m, sc);
            }
            _ => walk(cx, m, sc),
        }
    }
}

fn require_target(cx: &Cx, v: Node) -> Option<String> {
    let v = if v.kind() == "await_expression" { v.named_child(0)? } else { v };
    if v.kind() != "call_expression" {
        return None;
    }
    let f = v.child_by_field_name("function")?;
    if f.kind() != "identifier" || cx.text(f) != "require" {
        return None;
    }
    let args = v.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    if first.kind() != "string" && first.kind() != "template_string" {
        return None;
    }
    Some(unquote(cx.text(first)).to_owned())
}

fn var_decl(cx: &mut Cx, n: Node, scope: u32) {
    for d in kids(n) {
        if d.kind() != "variable_declarator" {
            continue;
        }
        let Some(name) = d.child_by_field_name("name") else { continue };
        let ty = d.child_by_field_name("type").map(|t| ts_type_name(cx, t)).unwrap_or_default();
        let value = d.child_by_field_name("value");
        if let Some(v) = value {
            if let Some(spec) = require_target(cx, v) {
                let line = cx.line(d);
                match name.kind() {
                    "identifier" => {
                        let local = cx.text(name).to_owned();
                        cx.add_import(ImportFact { local, imported: "*".into(), spec, kind: ImportKind::Require, type_only: false, scope, line });
                    }
                    "object_pattern" => {
                        for pr in kids(name) {
                            let (imported, local) = match pr.kind() {
                                "shorthand_property_identifier_pattern" => (cx.text(pr).to_owned(), cx.text(pr).to_owned()),
                                "pair_pattern" => {
                                    let k = pr.child_by_field_name("key").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                                    let v = pr.child_by_field_name("value").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                                    (k, v)
                                }
                                _ => continue,
                            };
                            cx.add_import(ImportFact { local, imported, spec: spec.clone(), kind: ImportKind::Require, type_only: false, scope, line });
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let is_fn = matches!(v.kind(), "arrow_function" | "function_expression" | "function" | "generator_function");
            if is_fn && name.kind() == "identifier" {
                let nm = cx.text(name).to_owned();
                let line = cx.line(d);
                cx.add_binding(scope, &nm, BindKind::LocalFn, "", "", line);
            } else {
                let (vt, init) = value_info(cx, v);
                let dt = if ty.is_empty() { vt } else { ty };
                bind_pattern(cx, name, scope, BindKind::Local, &dt, &init);
            }
            walk(cx, v, scope);
        } else {
            bind_pattern(cx, name, scope, BindKind::Local, &ty, "");
        }
    }
}
