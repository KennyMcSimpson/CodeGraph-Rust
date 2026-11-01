use cg_core::Lang;
use cg_extract::*;

fn run(rel: &str, src: &str, lang: Lang) -> Facts {
    let b = extract_bytes(rel, src.as_bytes(), 0, lang, &ExtractConfig::new("."));
    assert_eq!(b.flags & fileflags::PANICKED, 0, "{:?}", b.errors);
    b.facts
}

fn imp<'a>(f: &'a Facts, local: &str) -> &'a ImportFact {
    f.imports.iter().find(|i| i.local == local).unwrap_or_else(|| panic!("no import {local}: {:?}", f.imports))
}
fn bind<'a>(f: &'a Facts, name: &str) -> &'a BindingFact {
    f.bindings.iter().find(|b| b.name == name).unwrap_or_else(|| panic!("no binding {name}: {:?}", f.bindings))
}

#[test]
fn python() {
    let f = run(
        "a.py",
        r#"import os
import pkg.sub as ps
import a.b.c
from .sibling import helper, Thing as T
from ..up import *

class Child(Base):
    count: int = 0
    def __init__(self, repo: Repo, n: "Cfg" = None):
        self.repo = repo
        self.cache = Cache()
    def run(self):
        x = T()
        for item in self.items:
            item.go()
        def inner(): pass
"#,
        Lang::Python,
    );
    assert_eq!((imp(&f, "os").spec.as_str(), imp(&f, "os").kind), ("os", ImportKind::Default));
    assert_eq!(imp(&f, "ps").spec, "pkg.sub");
    assert_eq!((imp(&f, "a").spec.as_str(), imp(&f, "a").imported.as_str()), ("a.b.c", "*"));
    assert_eq!((imp(&f, "T").imported.as_str(), imp(&f, "T").spec.as_str()), ("Thing", ".sibling"));
    assert_eq!(imp(&f, "*").spec, "..up");
    let fields: Vec<_> = f.bindings.iter().filter(|b| b.name == "repo").collect();
    assert!(fields.iter().any(|b| b.kind == BindKind::Param && b.decl_type == "Repo"));
    assert!(fields.iter().any(|b| b.kind == BindKind::Field && b.decl_type == "Repo"), "{fields:?}");
    assert_eq!(bind(&f, "cache").init_ctor, "Cache()");
    assert_eq!(bind(&f, "x").init_ctor, "T()");
    assert_eq!(bind(&f, "item").kind, BindKind::Local);
    assert_eq!(bind(&f, "inner").kind, BindKind::LocalFn);
    assert_eq!(bind(&f, "n").decl_type, "Cfg");
}

#[test]
fn go() {
    let f = run(
        "a.go",
        r#"package svc

import (
	"fmt"
	str "strings"
	"example.com/proj/pkg/util/v2"
	_ "embed"
)

type Server struct {
	repo Repo
	util.Base
}

func (s *Server) Run(n int) {
	helper := func() {}
	t := NewServer(nil)
	u := &Server{}
	var w Repo
	for _, v := range s.items { _ = v }
}
"#,
        Lang::Go,
    );
    assert_eq!(bind(&f, "svc").kind, BindKind::Package);
    assert_eq!(imp(&f, "fmt").spec, "fmt");
    assert_eq!(imp(&f, "str").spec, "strings");
    assert_eq!(imp(&f, "util").spec, "example.com/proj/pkg/util/v2");
    assert!(f.imports.iter().any(|i| i.kind == ImportKind::SideEffect && i.spec == "embed"));
    assert_eq!(bind(&f, "repo").decl_type, "Repo");
    assert_eq!((bind(&f, "Base").init_ctor.as_str(), bind(&f, "Base").decl_type.as_str()), ("embedded", "util.Base"));
    assert_eq!(bind(&f, "s").decl_type, "Server");
    assert_eq!(bind(&f, "helper").kind, BindKind::LocalFn);
    assert_eq!(bind(&f, "t").init_ctor, "NewServer()");
    assert_eq!(bind(&f, "u").init_ctor, "Server");
    assert_eq!(bind(&f, "w").decl_type, "Repo");
    assert_eq!(bind(&f, "v").kind, BindKind::Local);
}

#[test]
fn java() {
    let f = run(
        "A.java",
        r#"package com.x.svc;
import com.x.util.Helper;
import static com.x.util.Statics.go;
import java.util.*;
public class A {
    private final Repo repo;
    A(Repo repo) { this.repo = repo; }
    void run(Other o) {
        Helper h = new Helper();
        var v = new Widget();
        List<String> l = new ArrayList<>();
        for (Item i : items) { i.go(); }
    }
}
"#,
        Lang::Java,
    );
    assert_eq!(bind(&f, "com.x.svc").kind, BindKind::Package);
    assert_eq!((imp(&f, "Helper").spec.as_str(), imp(&f, "Helper").kind), ("com.x.util.Helper", ImportKind::Named));
    assert_eq!((imp(&f, "go").spec.as_str(), imp(&f, "go").kind), ("com.x.util.Statics.go", ImportKind::Static));
    assert_eq!((imp(&f, "*").spec.as_str(), imp(&f, "*").kind), ("java.util", ImportKind::Namespace));
    assert_eq!(bind(&f, "repo").decl_type, "Repo");
    assert_eq!(bind(&f, "o").decl_type, "Other");
    assert_eq!(bind(&f, "h").decl_type, "Helper");
    assert_eq!(bind(&f, "v").decl_type, "Widget");
    assert_eq!(bind(&f, "l").decl_type, "List");
    assert_eq!(bind(&f, "i").decl_type, "Item");
}

#[test]
fn rust() {
    let f = run(
        "a.rs",
        r#"use std::collections::HashMap;
use crate::util::{helper, Thing as T, sub::{self, deep}};
pub use super::sibling::*;
pub use crate::api::Public;

pub struct Server { repo: Repo, n: u32 }
impl Server {
    pub fn run(&self, other: &mut Other) {
        let t = T::new();
        let m: HashMap<String, u32> = HashMap::new();
        let s = Server { repo: r };
        for item in other.items() { item.go(); }
        let f = |x: Foo| x;
    }
}
"#,
        Lang::Rust,
    );
    assert_eq!((imp(&f, "HashMap").spec.as_str(), imp(&f, "HashMap").imported.as_str()), ("std::collections", "HashMap"));
    assert_eq!((imp(&f, "T").imported.as_str(), imp(&f, "T").spec.as_str()), ("Thing", "crate::util"));
    assert_eq!((imp(&f, "deep").spec.as_str()), "crate::util::sub");
    assert_eq!((imp(&f, "sub").imported.as_str(), imp(&f, "sub").spec.as_str()), ("sub", "crate::util"));
    assert_eq!((imp(&f, "*").kind, imp(&f, "*").spec.as_str()), (ImportKind::Namespace, "super::sibling"));
    assert!(f.exports.iter().any(|e| e.star && e.from_spec == "super::sibling"));
    assert!(f.exports.iter().any(|e| e.name == "Public" && e.from_spec == "crate::api"));
    assert_eq!(bind(&f, "repo").kind, BindKind::Field);
    assert_eq!(bind(&f, "self").decl_type, "Self");
    assert_eq!(bind(&f, "other").decl_type, "Other");
    assert_eq!(bind(&f, "t").init_ctor, "T::new()");
    assert_eq!(bind(&f, "m").decl_type, "HashMap<String, u32>");
    assert_eq!(bind(&f, "s").init_ctor, "Server");
    assert_eq!(bind(&f, "item").kind, BindKind::Local);
    assert_eq!(bind(&f, "x").decl_type, "Foo");
}

#[test]
fn kotlin() {
    let f = run(
        "A.kt",
        r#"package com.x.svc

import com.x.util.Helper
import com.x.util.Other as O
import java.util.*

class A(private val repo: Repo, name: String) : Base() {
    val cache: Cache = Cache()
    fun run(arg: Param) {
        val h = Helper()
        val typed: Widget = make()
        list.forEach { item -> item.go() }
    }
}
"#,
        Lang::Kotlin,
    );
    assert_eq!(bind(&f, "com.x.svc").kind, BindKind::Package);
    assert_eq!(imp(&f, "Helper").spec, "com.x.util.Helper");
    assert_eq!((imp(&f, "O").imported.as_str(), imp(&f, "O").spec.as_str()), ("Other", "com.x.util.Other"));
    assert_eq!((imp(&f, "*").kind, imp(&f, "*").spec.as_str()), (ImportKind::Namespace, "java.util"));
    assert_eq!((bind(&f, "repo").kind, bind(&f, "repo").decl_type.as_str()), (BindKind::Field, "Repo"));
    assert!(f.bindings.iter().all(|b| !(b.name == "name" && b.kind == BindKind::Field)), "plain ctor params are not fields");
    assert_eq!(bind(&f, "cache").decl_type, "Cache");
    assert_eq!(bind(&f, "arg").decl_type, "Param");
    assert_eq!(bind(&f, "h").init_ctor, "Helper()");
    assert_eq!(bind(&f, "typed").decl_type, "Widget");
    assert_eq!(bind(&f, "item").kind, BindKind::Param);
}
