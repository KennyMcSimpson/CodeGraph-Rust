//! Reference resolution (design doc 6.3-6.5): the strategy order is
//! framework rules, lexical scope, import binding, receiver type, qualified name,
//! same package / module, global name match. A reference is resolved by the first strategy that
//! gives an answer, and "better unresolved than wrong": ambiguity produces no edge.

use crate::builtins::{is_builtin, is_common_method};
use crate::engine::{Bound, Engine};
use crate::modules::{ImportTarget, PkgId};
use crate::shape::{parse_shape, split_head, Shape};
use cg_core::{text::pack_pos, EdgeKind, FileId, Lang, LangFamily, NodeIdx, NodeKind, RefKind, Visibility, NONE};
use cg_extract::{BindKind, ImportKind, ScopeKind};
use cg_store::{status, BindingRec, ImportRec, RefRec, ScopeRec, View};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::rc::Rc;

/// Which strategy produced an edge (stored in `out_res`, design doc 5.3).
pub mod by {
    pub const FRAMEWORK: u8 = 1;
    pub const LEXICAL: u8 = 2;
    pub const IMPORT: u8 = 3;
    pub const RECEIVER: u8 = 4;
    pub const QUALIFIED: u8 = 5;
    pub const PACKAGE: u8 = 6;
    pub const GLOBAL: u8 = 7;
    pub const EXTRACT: u8 = 8;

    pub fn name(b: u8) -> &'static str {
        match b {
            FRAMEWORK => "framework",
            LEXICAL => "scope",
            IMPORT => "import",
            RECEIVER => "receiver-type",
            QUALIFIED => "qualified-name",
            PACKAGE => "package",
            GLOBAL => "global-name",
            EXTRACT => "extraction",
            _ => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub status: u8,
    pub by: u8,
    /// Target node, or the candidate count for `Ambiguous`.
    pub target: u32,
    /// Confidence 0-100.
    pub conf: u8,
}

impl Outcome {
    pub const PENDING: Outcome = Outcome { status: status::PENDING, by: 0, target: NONE, conf: 0 };
    pub fn found(n: NodeIdx, by: u8, conf: u8) -> Outcome {
        Outcome { status: status::RESOLVED, by, target: n.0, conf }
    }
    pub fn st(s: u8) -> Outcome {
        Outcome { status: s, by: 0, target: NONE, conf: 0 }
    }
    pub fn ambiguous(k: u32) -> Outcome {
        Outcome { status: status::AMBIGUOUS, by: 0, target: k, conf: 0 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accept {
    Type,
    Callable,
    Any,
}

impl Accept {
    fn ok(self, k: NodeKind) -> bool {
        match self {
            Accept::Type => k.is_type() || matches!(k, NodeKind::Namespace | NodeKind::Module),
            Accept::Callable => k.is_callable() || k.is_type() || matches!(k, NodeKind::Variable | NodeKind::Constant | NodeKind::Property | NodeKind::Field),
            Accept::Any => k.is_definition(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameRes {
    Found(NodeIdx, u8, u8),
    Module(FileId),
    Pkg(PkgId),
    External,
    Local,
    Builtin,
    Ambiguous(u32),
    NotFound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRes {
    Node(NodeIdx),
    External,
    Builtin,
    Unknown,
}

/// Per-file lookup structures, built once per file and reused for all its references.
pub struct FileCx<'e, 'v> {
    pub e: &'e Engine<'v>,
    pub view: &'v View,
    pub file: FileId,
    pub lang: Lang,
    pub family: LangFamily,
    pub path: &'v str,
    pub pkg: PkgId,
    pub is_test: bool,
    scopes: &'v [ScopeRec],
    scope_base: u32,
    bindings: FxHashMap<(u32, &'v str), SmallVec<[&'v BindingRec; 2]>>,
    pub imports: &'v [ImportRec],
    import_by_local: FxHashMap<&'v str, SmallVec<[u32; 2]>>,
    imports_by_line: FxHashMap<u32, SmallVec<[u32; 4]>>,
    wildcards: SmallVec<[u32; 2]>,
    imported_files: FxHashSet<FileId>,
    top_dir: &'v str,
}

/// Fields of a class: name and declared type (when it resolves).
type FieldList = Vec<(Box<str>, Option<NodeIdx>)>;

pub struct WorkerCx<'e, 'v> {
    cache: Vec<(FileId, Rc<FileCx<'e, 'v>>, usize)>,
    cache_bytes: usize,
    field_cache: FxHashMap<NodeIdx, FieldList>,
    field_cache_bytes: usize,
}

impl<'e, 'v> WorkerCx<'e, 'v> {
    pub fn new() -> Self {
        WorkerCx { cache: Vec::new(), cache_bytes: 0, field_cache: FxHashMap::default(), field_cache_bytes: 0 }
    }

    pub fn file_cx(&mut self, e: &'e Engine<'v>, f: FileId) -> Rc<FileCx<'e, 'v>> {
        if let Some(pos) = self.cache.iter().position(|(id, _, _)| *id == f) {
            let item = self.cache.remove(pos);
            let rc = item.1.clone();
            self.cache.push(item);
            return rc;
        }
        let rc = Rc::new(FileCx::new(e, f));
        let bytes = rc.heap_bytes();
        let limit = e.budget.worker_cache_bytes();
        if bytes > limit {
            return rc;
        }
        while self.cache.len() >= 6 || self.cache_bytes.saturating_add(bytes) > limit {
            let (_, _, evicted) = self.cache.remove(0);
            self.cache_bytes -= evicted;
        }
        self.cache.push((f, rc.clone(), bytes));
        self.cache_bytes += bytes;
        rc
    }

    fn cache_field(&mut self, t: NodeIdx, name: &str, value: Option<NodeIdx>, limit: usize) {
        if name.len().saturating_add(256) > limit {
            return;
        }
        if self.field_cache_bytes.saturating_add(name.len() + 256) > limit {
            self.field_cache = FxHashMap::default();
            self.field_cache_bytes = 0;
        }
        let old_map_capacity = self.field_cache.capacity();
        let list = self.field_cache.entry(t).or_default();
        let old_capacity = list.capacity();
        list.push((name.into(), value));
        self.field_cache_bytes += name.len()
            + (list.capacity() - old_capacity) * std::mem::size_of::<(Box<str>, Option<NodeIdx>)>()
            + (self.field_cache.capacity() - old_map_capacity) * (std::mem::size_of::<(NodeIdx, FieldList)>() + 8);
        if self.field_cache_bytes > limit {
            self.field_cache = FxHashMap::default();
            self.field_cache_bytes = 0;
        }
    }
}

impl<'e, 'v> Default for WorkerCx<'e, 'v> {
    fn default() -> Self {
        Self::new()
    }
}

fn kind_of(view: &View, n: NodeIdx) -> NodeKind {
    NodeKind::from_u8(view.hot(n).kind).unwrap_or(NodeKind::Variable)
}

impl<'e, 'v> FileCx<'e, 'v> {
    fn heap_bytes(&self) -> usize {
        fn table<K, V>(map: &FxHashMap<K, V>) -> usize {
            map.capacity() * (std::mem::size_of::<(K, V)>() + 8)
        }
        let extra_bindings = self.bindings.values().filter(|v| v.spilled()).map(|v| v.capacity() * std::mem::size_of::<&BindingRec>()).sum::<usize>();
        let extra_imports = self.import_by_local.values().filter(|v| v.spilled()).map(|v| v.capacity() * 4).sum::<usize>()
            + self.imports_by_line.values().filter(|v| v.spilled()).map(|v| v.capacity() * 4).sum::<usize>();
        table(&self.bindings)
            + table(&self.import_by_local)
            + table(&self.imports_by_line)
            + extra_bindings
            + extra_imports
            + self.imported_files.capacity() * (std::mem::size_of::<FileId>() + 8)
            + if self.wildcards.spilled() { self.wildcards.capacity() * 4 } else { 0 }
    }
    pub fn new(e: &'e Engine<'v>, file: FileId) -> Self {
        let view = e.view;
        let fr = view.file(file);
        let path = fr.path();
        let lang = fr.lang();
        let scopes = view.scopes_of(file);
        let scope_base = view.scope_base(file);
        let mut bindings: FxHashMap<(u32, &'v str), SmallVec<[&'v BindingRec; 2]>> = FxHashMap::default();
        for b in view.bindings_of(file) {
            bindings.entry((b.scope.0.wrapping_sub(scope_base), view.text_str(b.name))).or_default().push(b);
        }
        let imports = view.imports_of(file);
        let mut import_by_local: FxHashMap<&'v str, SmallVec<[u32; 2]>> = FxHashMap::default();
        let mut imports_by_line: FxHashMap<u32, SmallVec<[u32; 4]>> = FxHashMap::default();
        let mut wildcards = SmallVec::new();
        let mut imported_files = FxHashSet::default();
        for (i, imp) in imports.iter().enumerate() {
            let local = view.text_str(imp.local);
            if local == "*" {
                wildcards.push(i as u32);
            } else if !local.is_empty() {
                import_by_local.entry(local).or_default().push(i as u32);
            }
            imports_by_line.entry(imp.line).or_default().push(i as u32);
            if let Some(ImportTarget::File(tf)) = e.imp_targets[file.idx()].get(i) {
                imported_files.insert(*tf);
            }
        }
        let top_dir = path.split('/').next().unwrap_or("");
        FileCx {
            e,
            view,
            file,
            lang,
            family: lang.family(),
            path,
            pkg: e.modules.pkg_of(file),
            is_test: fr.flags() & cg_extract::fileflags::TEST != 0,
            scopes,
            scope_base,
            bindings,
            imports,
            import_by_local,
            imports_by_line,
            wildcards,
            imported_files,
            top_dir,
        }
    }

    // ------------------------------------------------------------------ scopes
    pub fn scope_at(&self, line: u32, col: u32) -> u32 {
        if self.scopes.is_empty() {
            return 0;
        }
        let pos = pack_pos(line, col);
        let i = self.scopes.partition_point(|s| s.start <= pos);
        if i == 0 {
            return 0;
        }
        let mut cur = (i - 1) as u32;
        loop {
            let s = &self.scopes[cur as usize];
            if s.start <= pos && pos <= s.end {
                return cur;
            }
            if s.parent.0 == NONE {
                return 0;
            }
            cur = s.parent.0.wrapping_sub(self.scope_base);
            if cur as usize >= self.scopes.len() {
                return 0;
            }
        }
    }

    fn scope_parent(&self, s: u32) -> Option<u32> {
        let p = self.scopes.get(s as usize)?.parent.0;
        (p != NONE).then(|| p.wrapping_sub(self.scope_base))
    }

    fn scope_kind(&self, s: u32) -> u8 {
        self.scopes.get(s as usize).map_or(ScopeKind::File as u8, |x| x.kind)
    }

    fn scope_within(&self, mut inner: u32, outer: u32) -> bool {
        loop {
            if inner == outer {
                return true;
            }
            match self.scope_parent(inner) {
                Some(p) => inner = p,
                None => return outer == 0,
            }
        }
    }

    pub fn lookup_binding(&self, name: &str, scope: u32, line: u32, allow_class: bool) -> Option<&'v BindingRec> {
        let mut s = scope;
        loop {
            if allow_class || self.scope_kind(s) != ScopeKind::Class as u8 {
                if let Some(list) = self.bindings.get(&(s, name)) {
                    let mut best = None;
                    for b in list {
                        if b.kind == BindKind::Package as u8 {
                            continue;
                        }
                        if b.line == 0 || b.line <= line {
                            best = Some(*b);
                        }
                    }
                    if best.is_some() {
                        return best;
                    }
                }
            }
            s = self.scope_parent(s)?;
        }
    }

    fn visible_import(&self, name: &str, scope: u32) -> Option<u32> {
        let list = self.import_by_local.get(name)?;
        list.iter().copied().find(|i| {
            let imp = &self.imports[*i as usize];
            imp.scope.0 == NONE || self.scope_within(scope, imp.scope.0.wrapping_sub(self.scope_base))
        })
    }

    // ----------------------------------------------------------- type context
    pub fn enclosing_type(&self, from: NodeIdx) -> Option<NodeIdx> {
        let view = self.view;
        let mut cur = from;
        for _ in 0..24 {
            let n = view.node(cur);
            if n.kind().is_type() {
                return Some(cur);
            }
            match n.parent() {
                Some(p) if kind_of(view, p) != NodeKind::File => cur = p,
                _ => break,
            }
        }
        // methods declared outside their type's body (Go receivers, Rust impls): `Type::method`
        let q = view.node(from).qname();
        let prefix = &q[..q.rfind("::")?];
        let mut best = None;
        for cand in view.by_qname(prefix) {
            if !view.node(cand).kind().is_type() {
                continue;
            }
            let cf = view.hot(cand).file;
            if cf == self.file {
                return Some(cand);
            }
            // a candidate in the caller's package wins over earlier ones; otherwise the first stays
            if (self.pkg.0 != NONE && self.e.modules.pkg_of(cf) == self.pkg) || best.is_none() {
                best = Some(cand);
            }
        }
        best
    }

    /// Node of the class scope that corresponds to type node `t` (same start position).
    fn class_scope_for(&self, t: NodeIdx) -> Option<u32> {
        let n = self.view.node(t);
        let pos = pack_pos(n.start_line(), n.start_col());
        let end = pack_pos(n.end_line(), n.end_col());
        let mut best: Option<(u32, u64)> = None;
        for (i, s) in self.scopes.iter().enumerate() {
            if s.kind != ScopeKind::Class as u8 {
                continue;
            }
            if s.start == pos {
                return Some(i as u32);
            }
            if s.start <= pos && end <= s.end {
                let size = s.end - s.start;
                if best.is_none_or(|(_, b)| size < b) {
                    best = Some((i as u32, size));
                }
            }
        }
        best.map(|b| b.0)
    }

    /// Binding of a file-level variable called `name` (module singletons such as `app = Flask()`).
    pub fn file_binding(&self, name: &str) -> Option<&'v BindingRec> {
        self.bindings.get(&(0, name)).and_then(|l| l.iter().copied().find(|b| b.kind == BindKind::Local as u8 || b.kind == BindKind::Field as u8))
    }

    /// Follow `segs` from a module / type: exports, top-level definitions, submodules, members.
    fn walk_path(&self, start: Bound, segs: &[&str]) -> Option<Bound> {
        let mut cur = start;
        for seg in segs {
            cur = match cur {
                Bound::Module(m) => match self.e.lookup_export(m, seg, 0) {
                    Some(b) => b,
                    None => match self.e.lookup_top_level(m, seg) {
                        Some(n) => Bound::Node(n),
                        None => Bound::Module(self.e.submodule(m, seg)?),
                    },
                },
                Bound::Node(n) => Bound::Node(self.e.direct_member(n, seg).or_else(|| self.e.find_member(n, seg))?),
                Bound::Pkg(p) => {
                    let n = self.e.pkg_defs.get(p.0 as usize).and_then(|m| m.get(*seg)).and_then(|l| l.iter().find(|n| self.visible(**n, seg)))?;
                    Bound::Node(*n)
                }
                _ => return None,
            };
        }
        Some(cur)
    }

    // ------------------------------------------------------------ type lookup
    /// Type (or namespace) denoted by `name` in this file's context.
    pub fn lookup_type(&self, wcx: &mut WorkerCx<'e, 'v>, name: &str) -> TypeRes {
        let name = name.trim();
        if name.is_empty() {
            return TypeRes::Unknown;
        }
        if let Some((head, rest)) = split_head(name) {
            return self.qualified_type(wcx, head, rest, name);
        }
        match self.resolve_name(wcx, name, Accept::Type, 0, u32::MAX, None, true) {
            NameRes::Found(n, ..) => TypeRes::Node(n),
            NameRes::External => TypeRes::External,
            NameRes::Builtin => TypeRes::Builtin,
            _ => TypeRes::Unknown,
        }
    }

    fn qualified_type(&self, wcx: &mut WorkerCx<'e, 'v>, head: &str, rest: &str, full: &str) -> TypeRes {
        let segs: Vec<&str> = rest.split(['.', ':']).filter(|s| !s.is_empty()).collect();
        match self.resolve_name(wcx, head, Accept::Any, 0, u32::MAX, None, false) {
            NameRes::Module(tf) => match self.walk_path(Bound::Module(tf), &segs) {
                Some(Bound::Node(n)) => TypeRes::Node(n),
                _ => TypeRes::Unknown,
            },
            NameRes::Pkg(p) => match self.walk_path(Bound::Pkg(p), &segs) {
                Some(Bound::Node(n)) => TypeRes::Node(n),
                _ => TypeRes::Unknown,
            },
            NameRes::Found(n, ..) => self.nested_type(wcx, n, rest),
            NameRes::External => TypeRes::External,
            _ => {
                // fully qualified Java / Kotlin / Rust name
                let q = full.replace('.', "::");
                if let Some(n) = self.view.by_qname(&q).find(|n| kind_of(self.view, *n).is_type()) {
                    return TypeRes::Node(n);
                }
                let segs: Vec<&str> = full.split(['.', ':']).filter(|s| !s.is_empty()).collect();
                for i in (1..segs.len()).rev() {
                    let q = format!("{}::{}", segs[..i].join("."), segs[i..].join("::"));
                    if let Some(n) = self.view.by_qname(&q).find(|n| kind_of(self.view, *n).is_type()) {
                        return TypeRes::Node(n);
                    }
                }
                TypeRes::Unknown
            }
        }
    }

    fn nested_type(&self, _wcx: &mut WorkerCx<'e, 'v>, mut n: NodeIdx, rest: &str) -> TypeRes {
        for seg in rest.split(['.', ':']).filter(|s| !s.is_empty()) {
            match self.e.direct_member(n, seg) {
                Some(m) if self.view.node(m).kind().is_type() => n = m,
                _ => return TypeRes::Unknown,
            }
        }
        TypeRes::Node(n)
    }

    /// The type a binding holds, from its declared type or initialiser.
    pub fn binding_type(&self, wcx: &mut WorkerCx<'e, 'v>, b: &BindingRec) -> Option<NodeIdx> {
        let dt = self.view.text_str(b.decl_type);
        if !dt.is_empty() {
            return match self.lookup_type(wcx, dt) {
                TypeRes::Node(n) => Some(n),
                _ => None,
            };
        }
        let init = self.view.text_str(b.init_ctor);
        if init.is_empty() || init == "embedded" {
            return None;
        }
        if let Some(call) = init.strip_suffix("()") {
            return self.call_result_type(wcx, call);
        }
        match self.lookup_type(wcx, init) {
            TypeRes::Node(n) => Some(n),
            _ => None,
        }
    }

    /// Type produced by calling `callee` (a constructor, or a function with a return type).
    fn call_result_type(&self, wcx: &mut WorkerCx<'e, 'v>, callee: &str) -> Option<NodeIdx> {
        // `Type::new` / `Type.create`: assume factory methods of a type produce that type
        if let Some((head, rest)) = split_head(callee) {
            if let TypeRes::Node(t) = self.lookup_type(wcx, head) {
                if self.view.node(t).kind().is_type() {
                    if let Some(m) = self.e.find_member(t, rest.rsplit(['.', ':']).next().unwrap_or(rest)) {
                        if let Some(rt) = self.view.node(m).return_type() {
                            let rt_name = rt.trim();
                            if rt_name != "Self" && rt_name != "self" {
                                let owner = self.e.modules.view.hot(m).file;
                                let cx = wcx.file_cx(self.e, owner);
                                if let TypeRes::Node(n) = cx.lookup_type(wcx, rt_name) {
                                    return Some(n);
                                }
                            }
                        }
                    }
                    return Some(t);
                }
            }
        }
        let scope = 0;
        match self.resolve_name(wcx, callee, Accept::Callable, scope, u32::MAX, None, true) {
            NameRes::Found(n, ..) => {
                let node = self.view.node(n);
                if node.kind().is_type() {
                    return Some(n);
                }
                let rt = node.return_type()?;
                let owner = self.view.hot(n).file;
                let cx = wcx.file_cx(self.e, owner);
                match cx.lookup_type(wcx, rt) {
                    TypeRes::Node(t) => Some(t),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Declared type of field `name` of type `t` (through the hierarchy).
    pub fn field_type(&self, wcx: &mut WorkerCx<'e, 'v>, t: NodeIdx, name: &str) -> Option<NodeIdx> {
        if let Some(list) = wcx.field_cache.get(&t) {
            if let Some((_, v)) = list.iter().find(|(n, _)| &**n == name) {
                return *v;
            }
        }
        let mut result = None;
        let mut chain = vec![t];
        chain.extend(self.e.ancestors(t));
        for ty in chain {
            let owner = self.view.hot(ty).file;
            let cx = wcx.file_cx(self.e, owner);
            if let Some(cs) = cx.class_scope_for(ty) {
                if let Some(list) = cx.bindings.get(&(cs, name)) {
                    if let Some(b) = list.iter().find(|b| b.kind == BindKind::Field as u8) {
                        if let Some(ft) = cx.binding_type(wcx, b) {
                            result = Some(ft);
                            break;
                        }
                    }
                }
            }
        }
        wcx.cache_field(t, name, result, self.e.budget.worker_cache_bytes());
        result
    }

    // ------------------------------------------------------------ name lookup
    fn visible(&self, cand: NodeIdx, name: &str) -> bool {
        let h = self.view.hot(cand);
        if h.file == self.file {
            return true;
        }
        match self.family {
            LangFamily::Go => {
                let same_pkg = self.pkg.0 != NONE && self.e.modules.pkg_of(h.file) == self.pkg;
                same_pkg || name.chars().next().is_some_and(|c| c.is_uppercase())
            }
            LangFamily::Jvm => h.vis != Visibility::Private as u8,
            _ => true,
        }
    }

    fn score(&self, cand: NodeIdx) -> u32 {
        let h = self.view.hot(cand);
        let cf = h.file;
        let cpath = self.view.file(cf).path();
        let mut s = if cf == self.file {
            100
        } else if crate::project::parent_dir(cpath) == crate::project::parent_dir(self.path) || (self.pkg.0 != NONE && self.e.modules.pkg_of(cf) == self.pkg) {
            80
        } else if self.imported_files.contains(&cf) {
            60
        } else if cpath.split('/').next().unwrap_or("") == self.top_dir {
            40
        } else {
            20
        };
        let node = self.view.node(cand);
        if node.is_exported() == Some(true) {
            s += 5;
        }
        let cflags = self.view.file(cf).flags();
        if (cflags & cg_extract::fileflags::TEST != 0) == self.is_test {
            s += 5;
        }
        if cflags & cg_extract::fileflags::GENERATED == 0 {
            s += 3;
        }
        s
    }

    /// Name match over the whole project (design doc 6.5).
    fn global_match(&self, name: &str, accept: Accept, bare_callable: bool) -> NameRes {
        let mut best: SmallVec<[NodeIdx; 4]> = SmallVec::new();
        let mut best_score = 0u32;
        let mut seen = 0usize;
        for cand in self.view.by_name(name) {
            seen += 1;
            if seen > 4000 {
                break;
            }
            let node = self.view.node(cand);
            let k = node.kind();
            if !accept.ok(k) || (bare_callable && matches!(k, NodeKind::Method | NodeKind::Property | NodeKind::Field)) {
                continue;
            }
            let cf = node.file();
            if self.view.file(cf).lang().family() != self.family {
                continue;
            }
            if !self.visible(cand, name) {
                continue;
            }
            let sc = self.score(cand);
            if sc > best_score {
                best_score = sc;
                best.clear();
                best.push(cand);
            } else if sc == best_score {
                best.push(cand);
            }
        }
        match best.len() {
            0 => NameRes::NotFound,
            1 => {
                let (by_, conf) = if best_score >= 180 {
                    (by::LEXICAL, 98)
                } else if best_score >= 80 {
                    (by::PACKAGE, 85)
                } else if best_score >= 60 {
                    (by::GLOBAL, 70)
                } else if best_score >= 40 {
                    (by::GLOBAL, 55)
                } else {
                    (by::GLOBAL, 40)
                };
                NameRes::Found(best[0], by_, conf)
            }
            k => {
                // overloads under one parent: the first declaration wins
                let p0 = self.view.hot(best[0]).parent;
                if best.iter().all(|c| self.view.hot(*c).parent == p0 && self.view.node(*c).file() == self.view.node(best[0]).file()) {
                    return NameRes::Found(best[0], by::PACKAGE, 80);
                }
                NameRes::Ambiguous(k as u32)
            }
        }
    }

    /// Member `name` of the type enclosing `from` (and of its outer types for Java / Kotlin).
    fn enclosing_member(&self, from: Option<NodeIdx>, name: &str) -> Option<NodeIdx> {
        let view = self.view;
        let t = self.enclosing_type(from?)?;
        if let Some(m) = self.e.find_member(t, name) {
            return Some(m);
        }
        if self.family == LangFamily::Jvm {
            let mut outer = view.node(t).parent();
            while let Some(o) = outer {
                if kind_of(view, o) == NodeKind::File {
                    break;
                }
                if kind_of(view, o).is_type() {
                    if let Some(m) = self.e.find_member(o, name) {
                        return Some(m);
                    }
                }
                outer = view.node(o).parent();
            }
        }
        None
    }

    /// Resolve a simple name (design doc 6.3 steps 2, 3, 6, 7).
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_name(
        &self,
        _wcx: &mut WorkerCx<'e, 'v>,
        name: &str,
        accept: Accept,
        scope: u32,
        line: u32,
        from: Option<NodeIdx>,
        allow_global: bool,
    ) -> NameRes {
        let view = self.view;
        let e = self.e;
        // 2. lexical scope
        if accept != Accept::Type {
            if let Some(b) = self.lookup_binding(name, scope, line, false) {
                if b.kind == BindKind::LocalFn as u8 && b.node.0 != NONE {
                    return NameRes::Found(b.node, by::LEXICAL, 98);
                }
                if b.kind == BindKind::LocalFn as u8 {
                    // a local function declared in this scope: its node is the same-named nested definition
                    let hit = view.by_name(name).find(|n| view.hot(*n).file == self.file && matches!(kind_of(view, *n), NodeKind::Function | NodeKind::Method));
                    if let Some(n) = hit {
                        return NameRes::Found(n, by::LEXICAL, 95);
                    }
                }
                return NameRes::Local;
            }
            // members of the enclosing class are visible by bare name in these languages
            if matches!(self.family, LangFamily::Jvm | LangFamily::Python) {
                if let Some(m) = self.enclosing_member(from, name) {
                    return NameRes::Found(m, by::RECEIVER, 90);
                }
            }
        }
        // 3. import binding
        if let Some(i) = self.visible_import(name, scope) {
            match e.bind_import(self.file, i as usize) {
                Bound::Node(n) => {
                    if accept.ok(kind_of(view, n)) || accept == Accept::Any {
                        return NameRes::Found(n, by::IMPORT, 95);
                    }
                    // imported value used as a namespace (rare)
                    return NameRes::Found(n, by::IMPORT, 80);
                }
                Bound::Module(m) => return NameRes::Module(m),
                Bound::Pkg(p) => return NameRes::Pkg(p),
                Bound::External => return NameRes::External,
                Bound::Unknown => {}
            }
        }
        // JS / TS: `this.method()` is emitted as a bare `method`; an explicit import wins over it
        if accept != Accept::Type && self.family == LangFamily::Js {
            if let Some(m) = self.enclosing_member(from, name) {
                return NameRes::Found(m, by::RECEIVER, 88);
            }
        }
        // 4. same file
        let bare_callable = accept == Accept::Callable;
        let mut same: SmallVec<[NodeIdx; 4]> = SmallVec::new();
        for cand in view.by_name(name) {
            let h = view.hot(cand);
            if h.file != self.file {
                continue;
            }
            let k = NodeKind::from_u8(h.kind).unwrap_or(NodeKind::Variable);
            if !accept.ok(k) || (bare_callable && matches!(k, NodeKind::Method | NodeKind::Property | NodeKind::Field)) {
                continue;
            }
            same.push(cand);
        }
        if !same.is_empty() {
            // prefer top-level definitions, then the first declared
            let pick = same.iter().copied().find(|n| view.node(*n).parent().is_some_and(|p| kind_of(view, p) == NodeKind::File)).unwrap_or(same[0]);
            return NameRes::Found(pick, by::LEXICAL, 92);
        }
        // 5. same package / module
        if self.pkg.0 != NONE {
            if let Some(list) = e.pkg_defs.get(self.pkg.0 as usize).and_then(|m| m.get(name)) {
                let hits: SmallVec<[NodeIdx; 2]> = list.iter().copied().filter(|n| accept.ok(kind_of(view, *n)) && view.hot(*n).file != self.file).collect();
                if let Some(n) = hits.first() {
                    return NameRes::Found(*n, by::PACKAGE, 85);
                }
            }
        }
        // Rust: a child module is visible by name
        if self.family == LangFamily::Rust && accept != Accept::Callable {
            if let Some(m) = e.submodule(self.file, name) {
                return NameRes::Module(m);
            }
        }
        // 6. wildcard imports
        for &wi in &self.wildcards {
            let imp = &self.imports[wi as usize];
            let tgt = e.imp_targets[self.file.idx()].get(wi as usize).copied().unwrap_or(ImportTarget::External);
            match tgt {
                ImportTarget::Pkg(p) => {
                    if let Some(list) = e.pkg_defs.get(p.0 as usize).and_then(|m| m.get(name)) {
                        if let Some(n) = list.iter().find(|n| accept.ok(kind_of(view, **n))) {
                            return NameRes::Found(*n, by::IMPORT, 85);
                        }
                    }
                }
                ImportTarget::File(tf) => {
                    if let Some(Bound::Node(n)) = e.lookup_export(tf, name, 0) {
                        if accept.ok(kind_of(view, n)) {
                            return NameRes::Found(n, by::IMPORT, 85);
                        }
                    }
                }
                ImportTarget::Node(c) if imp.kind == ImportKind::Static as u8 => {
                    if let Some(m) = e.find_member(c, name) {
                        return NameRes::Found(m, by::IMPORT, 85);
                    }
                }
                _ => {}
            }
        }
        // 7. global name match
        if is_builtin(self.family, name) {
            return NameRes::Builtin;
        }
        if allow_global {
            return self.global_match(name, accept, bare_callable);
        }
        NameRes::NotFound
    }

    // ------------------------------------------------------------ call forms
    fn member_of(&self, wcx: &mut WorkerCx<'e, 'v>, mut t: NodeIdx, path: &[&str], method: &str, conf: u8) -> Outcome {
        for f in path {
            match self.field_type(wcx, t, f) {
                Some(ft) => t = ft,
                None => return Outcome::st(status::NO_CANDIDATE),
            }
        }
        match self.e.find_member(t, method) {
            Some(m) => Outcome::found(m, by::RECEIVER, conf),
            None => Outcome::st(status::NO_CANDIDATE),
        }
    }

    /// Method of an untyped receiver: only when the name is unique in the language family.
    fn fallback_method(&self, method: &str) -> Outcome {
        if is_common_method(self.family, method) || is_builtin(self.family, method) {
            return Outcome::st(status::NO_CANDIDATE);
        }
        let mut found: SmallVec<[NodeIdx; 2]> = SmallVec::new();
        for cand in self.view.by_name(method) {
            let node = self.view.node(cand);
            if !matches!(node.kind(), NodeKind::Method | NodeKind::Function | NodeKind::Property | NodeKind::Field) {
                continue;
            }
            if self.view.file(node.file()).lang().family() != self.family || !self.visible(cand, method) {
                continue;
            }
            found.push(cand);
            if found.len() > 1 {
                break;
            }
        }
        match found.len() {
            1 => Outcome::found(found[0], by::GLOBAL, 45),
            0 => Outcome::st(status::NO_CANDIDATE),
            _ => Outcome::ambiguous(2),
        }
    }

    fn in_module(&self, wcx: &mut WorkerCx<'e, 'v>, tf: FileId, path: &[&str], method: &str) -> Outcome {
        // walk module / type segments; the last segment may be a member of a type reached on the way
        let mut segs: Vec<&str> = path.to_vec();
        let mut cur = Bound::Module(tf);
        let mut consumed = 0;
        for (i, seg) in segs.iter().enumerate() {
            match self.walk_path(cur, &[seg]) {
                Some(next) => {
                    cur = next;
                    consumed = i + 1;
                }
                None => return Outcome::st(status::NO_CANDIDATE),
            }
        }
        segs.truncate(consumed);
        match cur {
            Bound::Module(m) => match self.walk_path(Bound::Module(m), &[method]) {
                Some(Bound::Node(n)) => Outcome::found(n, by::IMPORT, 92),
                _ => Outcome::st(status::NO_CANDIDATE),
            },
            Bound::Node(n) => {
                if self.view.node(n).kind().is_type() {
                    self.member_of(wcx, n, &[], method, 90)
                } else {
                    // a variable holding an instance: use the type of its initialiser
                    let owner = self.view.hot(n).file;
                    let cx = wcx.file_cx(self.e, owner);
                    let nm = self.view.node(n).name();
                    if let Some(b) = cx.file_binding(nm) {
                        if let Some(t) = cx.binding_type(wcx, b) {
                            return self.member_of(wcx, t, &[], method, 88);
                        }
                    }
                    Outcome::st(status::NO_CANDIDATE)
                }
            }
            _ => Outcome::st(status::NO_CANDIDATE),
        }
    }

    fn call_bare(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, name: &str, accept: Accept) -> Outcome {
        let scope = self.scope_at(r.line, r.col);
        match self.resolve_name(wcx, name, accept, scope, r.line, Some(r.from), true) {
            NameRes::Found(n, b, c) => Outcome::found(n, b, c),
            NameRes::Local => Outcome::st(status::LOCAL_BINDING),
            NameRes::External | NameRes::Module(_) | NameRes::Pkg(_) => Outcome::st(status::EXTERNAL),
            NameRes::Builtin => Outcome::st(status::BUILTIN),
            NameRes::Ambiguous(k) => Outcome::ambiguous(k),
            NameRes::NotFound => Outcome::st(status::NO_CANDIDATE),
        }
    }

    fn call_member(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, root: &str, path: &[&str], method: &str) -> Outcome {
        let scope = self.scope_at(r.line, r.col);
        let view = self.view;
        // (a) a local variable, parameter or field
        if let Some(b) = self.lookup_binding(root, scope, r.line, false) {
            if b.kind != BindKind::LocalFn as u8 {
                return match self.binding_type(wcx, b) {
                    Some(t) => {
                        let o = self.member_of(wcx, t, path, method, 90);
                        if o.status == status::RESOLVED {
                            o
                        } else {
                            Outcome::st(status::NO_CANDIDATE)
                        }
                    }
                    None => {
                        let o = self.fallback_method(method);
                        if o.status == status::RESOLVED && path.is_empty() {
                            o
                        } else {
                            Outcome::st(status::LOCAL_BINDING)
                        }
                    }
                };
            }
        }
        // (b) imported root
        if let Some(i) = self.visible_import(root, scope) {
            match self.e.bind_import(self.file, i as usize) {
                Bound::Module(tf) => return self.in_module(wcx, tf, path, method),
                Bound::Pkg(p) => {
                    let first = path.first().copied().unwrap_or(method);
                    let list = self.e.pkg_defs.get(p.0 as usize).and_then(|m| m.get(first));
                    let hit = list.and_then(|l| l.iter().find(|n| self.visible(**n, first)).copied());
                    return match hit {
                        Some(n) if path.is_empty() => Outcome::found(n, by::IMPORT, 92),
                        Some(n) => self.member_of(wcx, n, &path[1..], method, 90),
                        None => Outcome::st(status::NO_CANDIDATE),
                    };
                }
                Bound::Node(n) => {
                    let k = kind_of(view, n);
                    if k.is_type() {
                        return self.member_of(wcx, n, path, method, 92);
                    }
                    // an imported module-level variable: the type of its initialiser
                    {
                        let owner = view.hot(n).file;
                        let cx = wcx.file_cx(self.e, owner);
                        let nm = view.node(n).name();
                        if let Some(b) = cx.file_binding(nm) {
                            if let Some(t) = cx.binding_type(wcx, b) {
                                return self.member_of(wcx, t, path, method, 88);
                            }
                        }
                    }
                    if let Some(rt) = view.node(n).return_type() {
                        let owner = view.hot(n).file;
                        let cx = wcx.file_cx(self.e, owner);
                        if let TypeRes::Node(t) = cx.lookup_type(wcx, rt) {
                            return self.member_of(wcx, t, path, method, 85);
                        }
                    }
                    return Outcome::st(status::NO_CANDIDATE);
                }
                Bound::External => return Outcome::st(status::EXTERNAL),
                Bound::Unknown => {}
            }
        }
        // (c) a type (static member) or, in Rust, a child module
        if self.family == LangFamily::Rust {
            if let Some(mf) = self.e.submodule(self.file, root) {
                return self.in_module(wcx, mf, path, method);
            }
        }
        match self.lookup_type(wcx, root) {
            TypeRes::Node(t) => {
                if view.node(t).kind().is_type() {
                    return self.member_of(wcx, t, path, method, 90);
                }
            }
            TypeRes::External => return Outcome::st(status::EXTERNAL),
            TypeRes::Builtin => return Outcome::st(status::BUILTIN),
            TypeRes::Unknown => {}
        }
        if is_builtin(self.family, root) {
            return Outcome::st(status::BUILTIN);
        }
        // (d) nothing known about the receiver
        if path.is_empty() {
            self.fallback_method(method)
        } else {
            Outcome::st(status::NO_CANDIDATE)
        }
    }

    fn call_this(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, kw: &str, fields: &[&str], method: &str) -> Outcome {
        let Some(t) = self.enclosing_type(r.from) else {
            return if fields.is_empty() { self.fallback_method(method) } else { Outcome::st(status::NO_CANDIDATE) };
        };
        let _ = kw;
        let o = self.member_of(wcx, t, fields, method, 95);
        if o.status == status::RESOLVED {
            return o;
        }
        if fields.is_empty() {
            // a callable stored in a field of this type
            let tf = self.view.hot(t).file;
            let cx = wcx.file_cx(self.e, tf);
            if let Some(cs) = cx.class_scope_for(t) {
                if let Some(list) = cx.bindings.get(&(cs, self.view.name_str(self.view.hot(t).name))) {
                    let _ = list;
                }
                for key in cx.bindings.keys() {
                    if key.0 == cs && key.1 == method {
                        return Outcome::st(status::LOCAL_BINDING);
                    }
                }
            }
            return self.fallback_method(method);
        }
        Outcome::st(status::NO_CANDIDATE)
    }

    fn call_super(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, method: &str) -> Outcome {
        let _ = wcx;
        let Some(t) = self.enclosing_type(r.from) else { return Outcome::st(status::NO_CANDIDATE) };
        for a in self.e.ancestors(t) {
            if let Some(m) = self.e.direct_member(a, method) {
                return Outcome::found(m, by::RECEIVER, 90);
            }
        }
        Outcome::st(status::NO_CANDIDATE)
    }

    fn call_result(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, inner: &str, method: &str) -> Outcome {
        // type produced by calling `inner`
        let t = match self.call_result_type(wcx, inner) {
            Some(t) => t,
            None => {
                // `inner` may be a method of a typed receiver: resolve it as a call first
                let o = self.resolve_call_name(wcx, r, inner);
                if o.status != status::RESOLVED {
                    return Outcome::st(status::NO_CANDIDATE);
                }
                let n = NodeIdx(o.target);
                let node = self.view.node(n);
                if node.kind().is_type() {
                    n
                } else {
                    let Some(rt) = node.return_type() else { return Outcome::st(status::NO_CANDIDATE) };
                    let owner = self.view.hot(n).file;
                    let cx = wcx.file_cx(self.e, owner);
                    match cx.lookup_type(wcx, rt) {
                        TypeRes::Node(t) => t,
                        _ => return Outcome::st(status::NO_CANDIDATE),
                    }
                }
            }
        };
        self.member_of(wcx, t, &[], method, 85)
    }

    pub fn resolve_call_name(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, name: &str) -> Outcome {
        match parse_shape(name) {
            Shape::Bare(n) => self.call_bare(wcx, r, n, Accept::Callable),
            Shape::Member { root, path, method } => self.call_member(wcx, r, root, &path, method),
            Shape::This { kw, fields, method } => self.call_this(wcx, r, kw, &fields, method),
            Shape::Super { method } => self.call_super(wcx, r, method),
            Shape::New { class, method } => match self.lookup_type(wcx, class) {
                TypeRes::Node(t) => self.member_of(wcx, t, &[], method, 90),
                TypeRes::External => Outcome::st(status::EXTERNAL),
                _ => Outcome::st(status::NO_CANDIDATE),
            },
            Shape::CallResult { inner, method } => self.call_result(wcx, r, inner, method),
        }
    }

    // ------------------------------------------------------------ reference kinds
    fn resolve_type_ref(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec, name: &str, accept: Accept) -> Outcome {
        if let Some(c) = self.view.list(r.candidates).first() {
            // walkers sometimes hand over qualified candidates
            if let Some(n) = self.view.by_qname(c).next() {
                return Outcome::found(n, by::QUALIFIED, 90);
            }
        }
        let scope = self.scope_at(r.line, r.col);
        let res = if let Some((head, rest)) = split_head(name) {
            match self.qualified_type(wcx, head, rest, name) {
                TypeRes::Node(n) => NameRes::Found(n, by::QUALIFIED, 90),
                TypeRes::External => NameRes::External,
                TypeRes::Builtin => NameRes::Builtin,
                TypeRes::Unknown => NameRes::NotFound,
            }
        } else {
            let first = self.resolve_name(wcx, name, accept, scope, r.line, Some(r.from), true);
            // `Self` and friends
            match (first, name) {
                (NameRes::NotFound | NameRes::Builtin, "Self") => match self.enclosing_type(r.from) {
                    Some(t) => NameRes::Found(t, by::RECEIVER, 95),
                    None => first,
                },
                _ => first,
            }
        };
        match res {
            NameRes::Found(n, b, c) => Outcome::found(n, b, c),
            NameRes::External | NameRes::Module(_) | NameRes::Pkg(_) => Outcome::st(status::EXTERNAL),
            NameRes::Builtin => Outcome::st(status::BUILTIN),
            NameRes::Local => Outcome::st(status::LOCAL_BINDING),
            NameRes::Ambiguous(k) => Outcome::ambiguous(k),
            NameRes::NotFound => Outcome::st(status::NO_CANDIDATE),
        }
    }

    fn file_node(&self, f: FileId) -> NodeIdx {
        self.view.file(f).rec.node_start
    }

    fn bound_outcome(&self, b: Bound) -> Outcome {
        match b {
            Bound::Node(n) => Outcome::found(n, by::IMPORT, 95),
            Bound::Module(m) => Outcome::found(self.file_node(m), by::IMPORT, 95),
            Bound::Pkg(p) => {
                let pk = &self.e.modules.pkgs[p.0 as usize];
                match pk.files.first() {
                    Some(f) => Outcome::found(self.file_node(*f), by::IMPORT, 70),
                    None => Outcome::st(status::NO_CANDIDATE),
                }
            }
            Bound::External => Outcome::st(status::EXTERNAL),
            Bound::Unknown => Outcome::st(status::NO_CANDIDATE),
        }
    }

    fn resolve_import_ref(&self, name: &str, r: &RefRec) -> Outcome {
        let Some(idxs) = self.imports_by_line.get(&r.line) else { return Outcome::st(status::NO_CANDIDATE) };
        let view = self.view;
        let tgt = |i: u32| self.e.imp_targets[self.file.idx()].get(i as usize).copied().unwrap_or(ImportTarget::External);
        // 1. the module specifier itself
        for &i in idxs {
            let imp = &self.imports[i as usize];
            if view.text_str(imp.spec) == name {
                return match tgt(i) {
                    ImportTarget::File(tf) => Outcome::found(self.file_node(tf), by::IMPORT, 95),
                    ImportTarget::Node(n) => Outcome::found(n, by::IMPORT, 95),
                    ImportTarget::Pkg(p) => self.bound_outcome(Bound::Pkg(p)),
                    ImportTarget::External => Outcome::st(status::EXTERNAL),
                    ImportTarget::NotFound => Outcome::st(status::NO_CANDIDATE),
                };
            }
        }
        // 2. a name bound by the import
        for &i in idxs {
            let imp = &self.imports[i as usize];
            let local = view.text_str(imp.local);
            let spec = view.text_str(imp.spec);
            let imported = view.text_str(imp.imported);
            let full_rs = format!("{spec}::{imported}");
            let full_dot = format!("{spec}.{imported}");
            if (!local.is_empty() && local == name) || full_rs == name || full_dot == name {
                return self.bound_outcome(self.e.bind_import(self.file, i as usize));
            }
        }
        // 3. Rust: the root crate / module segment of a `use` path
        if self.family == LangFamily::Rust {
            for &i in idxs {
                let imp = &self.imports[i as usize];
                let spec = view.text_str(imp.spec);
                if spec.split("::").next() == Some(name) {
                    return match tgt(i) {
                        ImportTarget::External => Outcome::st(status::EXTERNAL),
                        _ => {
                            let first = self.e.modules.resolve_import(self.lang, name, self.file);
                            match first {
                                ImportTarget::File(tf) => Outcome::found(self.file_node(tf), by::IMPORT, 90),
                                _ => Outcome::st(status::EXTERNAL),
                            }
                        }
                    };
                }
            }
        }
        Outcome::st(status::NO_CANDIDATE)
    }

    /// Resolve one reference. `name` is the reference's text.
    pub fn resolve_ref(&self, wcx: &mut WorkerCx<'e, 'v>, r: &RefRec) -> Outcome {
        let name = self.view.text_str(r.name);
        let Some(kind) = RefKind::from_wire(r.kind) else { return Outcome::st(status::NO_CANDIDATE) };
        match kind {
            RefKind::Edge(EdgeKind::Imports) => self.resolve_import_ref(name, r),
            RefKind::Edge(EdgeKind::Extends | EdgeKind::Implements | EdgeKind::TypeOf | EdgeKind::Returns) => self.resolve_type_ref(wcx, r, name, Accept::Type),
            RefKind::Edge(EdgeKind::Instantiates) => {
                let o = self.resolve_type_ref(wcx, r, name, Accept::Type);
                if o.status == status::NO_CANDIDATE {
                    self.resolve_type_ref(wcx, r, name, Accept::Callable)
                } else {
                    o
                }
            }
            RefKind::Edge(EdgeKind::References) => {
                let o = self.resolve_type_ref(wcx, r, name, Accept::Type);
                if matches!(o.status, status::NO_CANDIDATE) {
                    self.resolve_type_ref(wcx, r, name, Accept::Any)
                } else {
                    o
                }
            }
            RefKind::FunctionRef => self.resolve_call_name(wcx, r, name),
            RefKind::Edge(EdgeKind::Calls | EdgeKind::Decorates | EdgeKind::Navigates | EdgeKind::Overrides | EdgeKind::Exports | EdgeKind::Contains) => {
                self.resolve_call_name(wcx, r, name)
            }
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn optional_field_cache_releases_capacity_at_its_budget() {
        let mut worker = WorkerCx::new();
        for i in 0..1000 {
            worker.cache_field(NodeIdx(i / 3), &format!("field{i}"), Some(NodeIdx(i)), 1024);
            assert!(worker.field_cache_bytes <= 1024);
        }
        let before = worker.field_cache_bytes;
        worker.cache_field(NodeIdx(0), &"x".repeat(2048), None, 1024);
        assert_eq!(worker.field_cache_bytes, before);
    }
}
