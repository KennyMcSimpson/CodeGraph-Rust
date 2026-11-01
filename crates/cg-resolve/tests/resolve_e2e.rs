use cg_resolve::{build_seg, IndexOptions};
use cg_store::{dump, Seg, View};
use std::path::Path;
use std::sync::Arc;

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, View) {
    let dir = tempfile::tempdir().unwrap();
    for (p, c) in files {
        let path = dir.path().join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, c).unwrap();
    }
    let built = build_seg(dir.path(), &IndexOptions { threads: Some(2), ..Default::default() }).unwrap();
    let seg: Arc<Seg> = Arc::new(built.seg);
    let root = Path::new(dir.path()).to_path_buf();
    (dir, View::new(seg).with_root(root))
}

fn has(edges: &[String], needle: &str) -> bool {
    edges.iter().any(|e| e == needle)
}

#[test]
fn typescript_imports_classes_and_receivers() {
    let (_d, view) = project(&[
        ("src/service.ts", "export class Service { run(): void {} }\nexport function helper() {}\n"),
        ("src/index.ts", "export * from './service';\n"),
        (
            "src/widget.ts",
            r#"import { Service, helper } from './index';
import * as svc from './service';
export class Base { base() {} }
export class Widget extends Base {
  private svc: Service;
  constructor() { super(); this.svc = new Service(); }
  render(): void {
    this.svc.run();
    helper();
    svc.helper();
    this.base();
    const s = new Service();
    s.run();
    localFn();
    function localFn() {}
  }
}
"#,
        ),
    ]);
    let mut edges = dump::dump_edges(&view);
    edges.dedup();
    for e in &edges {
        eprintln!("{e}");
    }
    assert!(has(&edges, "src/widget.ts::Widget -extends-> src/widget.ts::Base"));
    assert!(has(&edges, "src/widget.ts::Widget::render -calls-> src/service.ts::Service::run"), "this.svc.run through the re-export barrel");
    assert!(has(&edges, "src/widget.ts::Widget::render -calls-> src/service.ts::helper"));
    assert!(has(&edges, "src/widget.ts::Widget::render -calls-> src/widget.ts::Base::base"), "inherited method");
    assert!(has(&edges, "src/widget.ts::Widget::render -instantiates-> src/service.ts::Service"));
    assert!(has(&edges, "src/widget.ts::Widget::render -calls-> src/widget.ts::Widget::render::localFn"));
    assert!(has(&edges, "src/widget.ts -imports-> src/index.ts"));
    assert!(has(&edges, "src/index.ts -imports-> src/service.ts"));
}

#[test]
fn python_modules_and_self() {
    let (_d, view) = project(&[
        ("pkg/__init__.py", "from .core import Engine\n"),
        ("pkg/core.py", "class Engine:\n    def start(self):\n        self.warm()\n    def warm(self):\n        pass\n\ndef make():\n    return Engine()\n"),
        ("app.py", "from pkg import Engine, core\nimport pkg.core as pc\n\ndef main():\n    e = Engine()\n    e.start()\n    pc.make()\n    core.make()\n    print('x')\n"),
    ]);
    let mut edges = dump::dump_edges(&view);
    edges.dedup();
    for e in &edges {
        eprintln!("{e}");
    }
    assert!(has(&edges, "app.py::main -calls-> pkg/core.py::Engine::start"), "typed local from constructor call through the package re-export");
    assert!(has(&edges, "pkg/core.py::Engine::start -calls-> pkg/core.py::Engine::warm"));
    assert!(has(&edges, "app.py::main -calls-> pkg/core.py::make"));
    assert!(has(&edges, "app.py::main -instantiates-> pkg/core.py::Engine"));
}

#[test]
fn go_packages_and_interfaces() {
    let (_d, view) = project(&[
        ("go.mod", "module example.com/proj\n"),
        ("pkg/util/util.go", "package util\n\nfunc Do(n int) int { return n }\n\nfunc helper() {}\n"),
        (
            "cmd/main.go",
            r#"package main

import "example.com/proj/pkg/util"

type Repo interface { Find(id int) string }
type Server struct { repo Repo }

func (s *Server) Run() {
	s.repo.Find(1)
	util.Do(3)
	helper := func() {}
	helper()
	other()
}

func other() {}
func main() { (&Server{}).Run() }
"#,
        ),
    ]);
    let mut edges = dump::dump_edges(&view);
    edges.dedup();
    for e in &edges {
        eprintln!("{e}");
    }
    assert!(has(&edges, "cmd/main.go::Server::Run -calls-> cmd/main.go::Repo::Find"));
    assert!(has(&edges, "cmd/main.go::Server::Run -calls-> pkg/util/util.go::Do"));
    assert!(has(&edges, "cmd/main.go::Server::Run -calls-> cmd/main.go::other"));
    assert!(!edges.iter().any(|e| e.contains("util.go::helper")), "the local closure must shadow the package function");
}

#[test]
fn java_packages_imports_and_members() {
    let (_d, view) = project(&[
        ("src/com/x/util/Helper.java", "package com.x.util;\npublic class Helper { public void assist() {} public static void s() {} }\n"),
        ("src/com/x/util/Statics.java", "package com.x.util;\npublic class Statics { public static void go() {} }\n"),
        (
            "src/com/x/svc/A.java",
            r#"package com.x.svc;
import com.x.util.Helper;
import static com.x.util.Statics.go;
public class A extends Base {
    private final Helper repo;
    public void run() {
        repo.assist();
        Helper h = new Helper();
        h.assist();
        go();
        this.other();
        Helper.s();
    }
    private void other() {}
}
class Base {}
"#,
        ),
    ]);
    let mut edges = dump::dump_edges(&view);
    edges.dedup();
    for e in &edges {
        eprintln!("{e}");
    }
    let run = "src/com/x/svc/A.java::com.x.svc::A::run";
    assert!(has(&edges, &format!("{run} -calls-> src/com/x/util/Helper.java::com.x.util::Helper::assist")));
    assert!(has(&edges, &format!("{run} -calls-> src/com/x/util/Statics.java::com.x.util::Statics::go")), "static import");
    assert!(has(&edges, &format!("{run} -calls-> src/com/x/svc/A.java::com.x.svc::A::other")));
    assert!(has(&edges, &format!("{run} -calls-> src/com/x/util/Helper.java::com.x.util::Helper::s")));
    assert!(has(&edges, &format!("{run} -instantiates-> src/com/x/util/Helper.java::com.x.util::Helper")));
}

#[test]
fn rust_modules_use_and_impl() {
    let (_d, view) = project(&[
        ("Cargo.toml", "[package]\nname = \"demo\"\n"),
        ("src/lib.rs", "pub mod util;\npub mod server;\n"),
        ("src/util.rs", "pub fn helper(n: u32) -> u32 { n }\npub struct Thing;\nimpl Thing { pub fn new() -> Thing { Thing } pub fn go(&self) {} }\n"),
        (
            "src/server.rs",
            r#"use crate::util::{helper, Thing as T};
pub trait Repo { fn find(&self, id: u32) -> u32; }
pub struct Server { repo: Box<dyn Repo> }
impl Server {
    pub fn run(&self) {
        helper(3);
        let t = T::new();
        t.go();
        Self::other();
    }
    fn other() {}
}
"#,
        ),
    ]);
    let mut edges = dump::dump_edges(&view);
    edges.dedup();
    for e in &edges {
        eprintln!("{e}");
    }
    assert!(has(&edges, "src/server.rs::Server::run -calls-> src/util.rs::helper"));
    assert!(has(&edges, "src/server.rs::Server::run -calls-> src/util.rs::Thing::new"));
    assert!(has(&edges, "src/server.rs::Server::run -calls-> src/util.rs::Thing::go"), "type of `t` comes from T::new()");
    assert!(has(&edges, "src/server.rs::Server::run -calls-> src/server.rs::Server::other"));
}

#[test]
fn kotlin_members_and_imports() {
    let (_d, view) = project(&[
        ("src/com/x/util/Helper.kt", "package com.x.util\nclass Helper { fun assist() {} }\n"),
        (
            "src/com/x/svc/A.kt",
            "package com.x.svc\nimport com.x.util.Helper\nclass A(private val repo: Helper) {\n    fun run() {\n        repo.assist()\n        val h = Helper()\n        h.assist()\n        other()\n    }\n    private fun other() {}\n}\n",
        ),
    ]);
    let mut edges = dump::dump_edges(&view);
    edges.dedup();
    for e in &edges {
        eprintln!("{e}");
    }
    let run = "src/com/x/svc/A.kt::com.x.svc::A::run";
    assert!(has(&edges, &format!("{run} -calls-> src/com/x/util/Helper.kt::com.x.util::Helper::assist")));
    assert!(has(&edges, &format!("{run} -calls-> src/com/x/svc/A.kt::com.x.svc::A::other")));
}

#[test]
fn debug_ts_unresolved() {
    let (_d, view) = project(&[
        ("src/service.ts", "export class Service { run(): void {} }\nexport function helper() {}\n"),
        (
            "src/widget.ts",
            "import { Service } from './service';\nexport class Base { base() {} }\nexport class Widget extends Base {\n  render(): void {\n    this.base();\n    this.render();\n  }\n}\n",
        ),
    ]);
    for u in dump::dump_unresolved(&view) {
        eprintln!("UNRESOLVED {u}");
    }
    for e in dump::dump_edges(&view) {
        eprintln!("EDGE {e}");
    }
}

#[test]
fn debug_python_typing_and_package() {
    let (_d, view) = project(&[
        ("src/flask/__init__.py", "from .app import Flask as Flask\n"),
        ("src/flask/app.py", "import typing as t\nT = t.TypeVar('T')\nclass Flask:\n    def route(self): pass\n"),
        ("tests/test_a.py", "import flask\nimport typing as t\n\ndef test():\n    app = flask.Flask('x')\n    app.route()\n    x = t.cast(int, 1)\n"),
    ]);
    for u in dump::dump_unresolved(&view) {
        eprintln!("UNRESOLVED {u}");
    }
    for e in dump::dump_edges(&view) {
        eprintln!("EDGE {e}");
    }
    for f in view.files() {
        for i in view.imports_of(f) {
            eprintln!("IMPORT {} local={} imported={} spec={} kind={} scope={} line={}", view.file(f).path(), view.text_str(i.local), view.text_str(i.imported), view.text_str(i.spec), i.kind, i.scope.0, i.line);
        }
    }
}
