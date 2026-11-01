//! Facts for Python: imports, function / class scopes, parameters with annotations, assignments
//! (module variables, locals, class attributes, `self.x = ...` fields).

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
        "import_from_statement" => from_import(cx, n, scope),
        "function_definition" => function(cx, n, scope),
        "lambda" => lambda(cx, n, scope),
        "class_definition" => class(cx, n, scope),
        "assignment" => {
            assignment(cx, n, scope);
            walk_children(cx, n, scope);
        }
        "for_statement" | "for_in_clause" => {
            if let Some(l) = n.child_by_field_name("left") {
                bind_targets(cx, l, scope);
            }
            walk_children(cx, n, scope);
        }
        "as_pattern" => {
            if let Some(a) = n.child_by_field_name("alias") {
                bind_targets(cx, a, scope);
            } else if let Some(last) = n.named_child(n.named_child_count().saturating_sub(1)) {
                if last.kind() == "as_pattern_target" {
                    bind_targets(cx, last, scope);
                }
            }
            walk_children(cx, n, scope);
        }
        "named_expression" => {
            if let Some(nm) = n.child_by_field_name("name") {
                bind_targets(cx, nm, scope);
            }
            walk_children(cx, n, scope);
        }
        _ => walk_children(cx, n, scope),
    }
}

fn bind_targets(cx: &mut Cx, n: Node, scope: u32) {
    match n.kind() {
        "identifier" => {
            let nm = cx.text(n).to_owned();
            let line = cx.line(n);
            cx.add_binding(scope, &nm, BindKind::Local, "", "", line);
        }
        "as_pattern_target" | "pattern_list" | "tuple_pattern" | "list_pattern" | "list_splat_pattern" | "parenthesized_expression" => {
            for ch in kids(n) {
                bind_targets(cx, ch, scope);
            }
        }
        _ => {}
    }
}

/// `Optional[Foo]`, `Foo | None`, `"Foo"`, `list[Foo]` -> `Foo` / "" for builtins and containers.
pub fn py_type_name(text: &str) -> String {
    let mut t = text.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    for wrapper in ["Optional[", "Final[", "ClassVar[", "Annotated[", "Type[", "type["] {
        if let Some(inner) = t.strip_prefix(wrapper) {
            t = inner.trim_end_matches(']').split(',').next().unwrap_or("").trim();
        }
    }
    if let Some((a, b)) = t.split_once('|') {
        let (a, b) = (a.trim(), b.trim());
        t = if a == "None" { b } else { a };
    }
    let name = simple_type_name(t);
    match name.as_str() {
        "int" | "str" | "float" | "bool" | "bytes" | "list" | "dict" | "set" | "tuple" | "None" | "Any" | "object" | "List" | "Dict" | "Set" | "Tuple"
        | "Callable" | "Iterable" | "Iterator" | "Sequence" | "Mapping" => String::new(),
        _ => name,
    }
}

fn value_info(cx: &Cx, v: Node, scope: u32) -> (String, String) {
    match v.kind() {
        "call" => match v.child_by_field_name("function") {
            Some(f) if matches!(f.kind(), "identifier" | "attribute") => (String::new(), format!("{}()", cx.text(f))),
            _ => Default::default(),
        },
        "await" | "parenthesized_expression" => v.named_child(0).map(|c| value_info(cx, c, scope)).unwrap_or_default(),
        "identifier" => {
            // alias of another binding: copy what we know about it
            match cx.lookup_binding(scope, cx.text(v)) {
                Some((t, i)) => (t, i),
                None => Default::default(),
            }
        }
        _ => Default::default(),
    }
}

fn import_stmt(cx: &mut Cx, n: Node, scope: u32) {
    let line = cx.line(n);
    for nm in kids(n) {
        match nm.kind() {
            "dotted_name" => {
                let full = cx.text(nm).to_owned();
                let first = full.split('.').next().unwrap_or("").to_owned();
                cx.add_import(ImportFact { local: first, imported: "*".into(), spec: full, kind: ImportKind::Default, type_only: false, scope, line });
            }
            "aliased_import" => {
                let full = nm.child_by_field_name("name").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                let alias = nm.child_by_field_name("alias").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                cx.add_import(ImportFact { local: alias, imported: "*".into(), spec: full, kind: ImportKind::Default, type_only: false, scope, line });
            }
            _ => {}
        }
    }
}

fn from_import(cx: &mut Cx, n: Node, scope: u32) {
    let line = cx.line(n);
    let Some(m) = n.child_by_field_name("module_name") else { return };
    let spec = cx.text(m).split_whitespace().collect::<String>();
    let mut any = false;
    for nm in kids(n) {
        if nm.id() == m.id() {
            continue;
        }
        match nm.kind() {
            "dotted_name" => {
                any = true;
                let name = cx.text(nm).to_owned();
                cx.add_import(ImportFact { local: name.clone(), imported: name, spec: spec.clone(), kind: ImportKind::Named, type_only: false, scope, line });
            }
            "aliased_import" => {
                any = true;
                let name = nm.child_by_field_name("name").map(|c| cx.text(c).to_owned()).unwrap_or_default();
                let alias = nm.child_by_field_name("alias").map(|c| cx.text(c).to_owned()).unwrap_or_else(|| name.clone());
                cx.add_import(ImportFact { local: alias, imported: name, spec: spec.clone(), kind: ImportKind::Named, type_only: false, scope, line });
            }
            "wildcard_import" => {
                any = true;
                cx.add_import(ImportFact {
                    local: "*".into(),
                    imported: "*".into(),
                    spec: spec.clone(),
                    kind: ImportKind::Namespace,
                    type_only: false,
                    scope,
                    line,
                });
            }
            _ => {}
        }
    }
    let _ = any;
}

fn param_names(cx: &mut Cx, params: Node, sc: u32) {
    for p in kids(params) {
        match p.kind() {
            "identifier" => {
                let nm = cx.text(p).to_owned();
                let line = cx.line(p);
                cx.add_binding(sc, &nm, BindKind::Param, "", "", line);
            }
            "typed_parameter" | "typed_default_parameter" | "default_parameter" => {
                let ty = p.child_by_field_name("type").map(|t| py_type_name(cx.text(t))).unwrap_or_default();
                let name_node = p.child_by_field_name("name").or_else(|| p.named_child(0));
                if let Some(nn) = name_node {
                    let inner = if matches!(nn.kind(), "list_splat_pattern" | "dictionary_splat_pattern") { nn.named_child(0) } else { Some(nn) };
                    if let Some(i) = inner.filter(|i| i.kind() == "identifier") {
                        let nm = cx.text(i).to_owned();
                        let line = cx.line(p);
                        cx.add_binding(sc, &nm, BindKind::Param, &ty, "", line);
                    }
                }
            }
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                if let Some(i) = p.named_child(0) {
                    let nm = cx.text(i).to_owned();
                    let line = cx.line(p);
                    cx.add_binding(sc, &nm, BindKind::Param, "", "", line);
                }
            }
            _ => {}
        }
    }
}

fn function(cx: &mut Cx, n: Node, scope: u32) {
    if cx.facts.scopes[scope as usize].kind != ScopeKind::Class {
        if let Some(name) = n.child_by_field_name("name") {
            let nm = cx.text(name).to_owned();
            cx.add_binding(scope, &nm, BindKind::LocalFn, "", "", 0);
        }
    }
    let sc = cx.push_scope(ScopeKind::Function, scope, n);
    if let Some(params) = n.child_by_field_name("parameters") {
        param_names(cx, params, sc);
    }
    if let Some(body) = n.child_by_field_name("body") {
        walk(cx, body, sc);
    }
    if let Some(dec) = n.child_by_field_name("decorators") {
        walk(cx, dec, scope);
    }
}

fn lambda(cx: &mut Cx, n: Node, scope: u32) {
    let sc = cx.push_scope(ScopeKind::Function, scope, n);
    if let Some(params) = n.child_by_field_name("parameters") {
        param_names(cx, params, sc);
    }
    if let Some(body) = n.child_by_field_name("body") {
        walk(cx, body, sc);
    }
}

fn class(cx: &mut Cx, n: Node, scope: u32) {
    let sc = cx.push_scope(ScopeKind::Class, scope, n);
    if let Some(body) = n.child_by_field_name("body") {
        walk(cx, body, sc);
    }
}

fn assignment(cx: &mut Cx, n: Node, scope: u32) {
    let Some(left) = n.child_by_field_name("left") else { return };
    let ty = n.child_by_field_name("type").map(|t| py_type_name(cx.text(t))).unwrap_or_default();
    let (vt, init) = n.child_by_field_name("right").map(|r| value_info(cx, r, scope)).unwrap_or_default();
    let decl = if ty.is_empty() { vt } else { ty };
    let line = cx.line(n);
    match left.kind() {
        "identifier" => {
            let nm = cx.text(left).to_owned();
            let kind = if cx.facts.scopes[scope as usize].kind == ScopeKind::Class { BindKind::Field } else { BindKind::Local };
            cx.add_binding(scope, &nm, kind, &decl, &init, line);
        }
        "attribute" => {
            let obj = left.child_by_field_name("object");
            let attr = left.child_by_field_name("attribute");
            if let (Some(obj), Some(attr)) = (obj, attr) {
                if obj.kind() == "identifier" && matches!(cx.text(obj), "self" | "cls") {
                    if let Some(cs) = cx.class_scope_of(scope) {
                        let nm = cx.text(attr).to_owned();
                        // keep the first typed assignment of a field
                        let already = cx
                            .facts
                            .bindings
                            .iter()
                            .any(|b| b.scope == cs && b.name == nm && b.kind == BindKind::Field && (!b.decl_type.is_empty() || !b.init_ctor.is_empty()));
                        if !already || !(decl.is_empty() && init.is_empty()) {
                            cx.add_binding(cs, &nm, BindKind::Field, &decl, &init, line);
                        }
                    }
                }
            }
        }
        "pattern_list" | "tuple_pattern" | "list_pattern" => bind_targets(cx, left, scope),
        _ => {}
    }
}
