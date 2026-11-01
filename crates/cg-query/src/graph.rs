//! `Graph`: a snapshot view plus the project root and per-project settings the query side needs.

use cg_core::config::ProjectConfig;
use cg_core::{fileflags, EdgeKind, FileId, NodeIdx, NodeKind};
use cg_store::View;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct Graph {
    pub view: View,
    pub root: PathBuf,
    pub config: ProjectConfig,
    deprioritized: GlobSet,
    avg_len: OnceLock<f32>,
    n_indexed_files: OnceLock<usize>,
}

impl Graph {
    pub fn new(view: View, root: PathBuf) -> Graph {
        let view = view.with_root(root.clone());
        let config = ProjectConfig::load(&root);
        let mut b = GlobSetBuilder::new();
        for p in &config.deprioritize {
            let p = p.trim_matches('/');
            for g in [format!("{p}"), format!("{p}/**"), format!("**/{p}"), format!("**/{p}/**")] {
                if let Ok(g) = Glob::new(&g) {
                    b.add(g);
                }
            }
        }
        let deprioritized = b.build().unwrap_or_else(|_| GlobSet::empty());
        Graph { view, root, config, deprioritized, avg_len: OnceLock::new(), n_indexed_files: OnceLock::new() }
    }

    /// Number of live indexed files (budget tiers are keyed on it).
    pub fn indexed_files(&self) -> usize {
        *self.n_indexed_files.get_or_init(|| self.view.files().count())
    }

    /// Mean token count per node (BM25 length normalisation).
    pub fn avg_node_len(&self) -> f32 {
        *self.avg_len.get_or_init(|| {
            let norm = &self.view.base.p.node_norm;
            if norm.is_empty() {
                return 1.0;
            }
            let sum: u64 = norm.iter().map(|v| *v as u64).sum();
            (sum as f32 / norm.len() as f32).max(1.0)
        })
    }

    pub fn path(&self, f: FileId) -> &str {
        self.view.file(f).path()
    }

    pub fn node_path(&self, n: NodeIdx) -> &str {
        self.path(self.view.hot(n).file)
    }

    pub fn kind(&self, n: NodeIdx) -> NodeKind {
        NodeKind::from_u8(self.view.hot(n).kind).unwrap_or(NodeKind::Variable)
    }

    pub fn name(&self, n: NodeIdx) -> &str {
        self.view.name_str(self.view.hot(n).name)
    }

    pub fn qname(&self, n: NodeIdx) -> &str {
        self.view.name_str(self.view.hot(n).qname)
    }

    pub fn line(&self, n: NodeIdx) -> u32 {
        self.view.hot(n).start_line
    }

    pub fn end_line(&self, n: NodeIdx) -> u32 {
        self.view.hot(n).end_line
    }

    /// `name (path:line)`: the shape flow and blast-radius lines use.
    pub fn locate(&self, n: NodeIdx) -> String {
        format!("{} ({}:{})", self.name(n), self.node_path(n), self.line(n))
    }

    pub fn file_flags(&self, f: FileId) -> u8 {
        self.view.file(f).flags()
    }

    pub fn is_test_file(&self, f: FileId) -> bool {
        self.file_flags(f) & fileflags::TEST != 0
    }

    pub fn is_generated_file(&self, f: FileId) -> bool {
        self.file_flags(f) & fileflags::GENERATED != 0
    }

    /// Rank multiplier for a file (design doc 8.2): tests, generated code, configured
    /// `deprioritize` paths and example/sample/fixture/benchmark/demo folders rank lower.
    pub fn file_weight(&self, f: FileId) -> f32 {
        let flags = self.file_flags(f);
        let mut w = 1.0f32;
        if flags & fileflags::TEST != 0 {
            w *= 0.5;
        }
        if flags & fileflags::GENERATED != 0 {
            w *= 0.3;
        }
        let path = self.path(f);
        if flags & fileflags::DEPRIORITIZED != 0 || self.deprioritized.is_match(path) {
            w *= 0.4;
        }
        if is_example_path(path) {
            w *= 0.6;
        }
        w
    }

    /// The node's containing symbol chain, outermost last: `[method, class]`.
    pub fn containers(&self, n: NodeIdx) -> Vec<NodeIdx> {
        let mut out = Vec::new();
        let mut cur = self.view.hot(n).parent;
        while cur.is_some() && out.len() < 16 {
            if self.kind(cur) == NodeKind::File {
                break;
            }
            out.push(cur);
            cur = self.view.hot(cur).parent;
        }
        out
    }

    /// Is `n` a definition users search for (not a file / import / export / parameter)?
    pub fn is_definition(&self, n: NodeIdx) -> bool {
        self.kind(n).is_definition()
    }

    /// Absolute path of an indexed file, only if it stays inside the project root.
    pub fn abs_path(&self, rel: &str) -> Option<PathBuf> {
        safe_join(&self.root, rel)
    }

    /// File node of a file.
    pub fn file_node(&self, f: FileId) -> NodeIdx {
        NodeIdx(self.view.nodes_in_file(f).start)
    }

    /// Edge kinds that carry a call/dataflow relationship between symbols.
    pub fn is_flow_edge(k: EdgeKind) -> bool {
        matches!(k, EdgeKind::Calls | EdgeKind::Instantiates | EdgeKind::References | EdgeKind::Navigates)
    }
}

pub fn is_example_path(path: &str) -> bool {
    path.split('/')
        .any(|seg| matches!(seg, "example" | "examples" | "sample" | "samples" | "fixture" | "fixtures" | "benchmark" | "benchmarks" | "demo" | "demos"))
}

/// `root/rel` if it does not escape `root` (no `..` components, no absolute `rel`).
pub fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if rel.is_absolute() {
        return None;
    }
    let mut out = root.to_path_buf();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(p) => out.push(p),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}
