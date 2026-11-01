//! Extraction pipeline (design doc 4.4, 4.6): read -> detect -> kernel -> facts -> post,
//! with panic isolation, a big-stack retry for deeply nested files and a parse timeout.
//! Files are processed in memory-bounded batches on a rayon pool; results are placed by
//! file index, so the output never depends on thread scheduling.

use crate::facts::{self, Facts};
use crate::post::{commonjs_require_refs, ExtraNode, ExtraRef, FileHook, HookCtx, HookOut, SiteFact};
use crate::rows::FileRows;
use cg_core::{hash, Lang};
use cg_kernel::{EmitOut, ExtractOptions, KernelError};
use cg_scan::{generated, lang as scanlang, testpath, FileEntry};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bits of `FileBatch::flags` (stored in `FileRec::flags`, design doc 5.3).
pub mod fileflags {
    pub const PARSE_ERROR: u8 = 1;
    pub const GENERATED: u8 = 2;
    pub const TEST: u8 = 4;
    pub const DEPRIORITIZED: u8 = 8;
    pub const PANICKED: u8 = 16;
    pub const TIMED_OUT: u8 = 32;
}

#[derive(Clone)]
pub struct ExtractConfig {
    pub root: PathBuf,
    pub threads: usize,
    /// Source bytes read per batch.
    pub batch_bytes: u64,
    pub parse_timeout: Duration,
    pub allow_error_trees: bool,
    pub overrides: BTreeMap<String, Lang>,
    /// Lower thread priority (Unix nice 10) so a build does not starve the editor.
    pub low_priority: bool,
    /// Framework extract hooks and synthesis site scanners.
    pub hooks: Vec<Arc<dyn FileHook>>,
}

impl ExtractConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ExtractConfig {
            root: root.into(),
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2),
            batch_bytes: 64 << 20,
            parse_timeout: Duration::from_secs(5),
            allow_error_trees: true,
            overrides: BTreeMap::new(),
            low_priority: false,
            hooks: Vec::new(),
        }
    }
}

/// Everything extraction learned about one file.
pub struct FileBatch {
    pub rel: String,
    pub lang: Lang,
    pub size: u32,
    pub mtime_ns: i64,
    pub hash: [u8; 16],
    pub flags: u8,
    /// Kernel tables (or a file-node-only table if the file could not be extracted).
    pub emit: EmitOut,
    pub extra_nodes: Vec<ExtraNode>,
    pub extra_refs: Vec<ExtraRef>,
    pub sites: Vec<SiteFact>,
    pub facts: Facts,
    /// Human-readable problems, e.g. `panic: ...`.
    pub errors: Vec<String>,
}

impl FileBatch {
    pub fn rows(&self) -> FileRows<'_> {
        FileRows::new(&self.emit)
    }
}

/// Extraction output for a file the kernel could not handle: the file node alone.
pub fn file_node_only(rel: &str, line_count: u32) -> EmitOut {
    use cg_kernel::{KERNEL_ABI_VERSION, META_SIZE, NODE_ROW_SIZE, NONE};
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let id = format!("file:{rel}");
    let mut arena = Vec::new();
    let mut put = |s: &str| -> (u32, u32) {
        let o = arena.len() as u32;
        arena.extend_from_slice(s.as_bytes());
        (o, s.len() as u32)
    };
    let (n, q, i) = (put(name), put(rel), put(&id));
    let mut node = Vec::with_capacity(NODE_ROW_SIZE);
    node.push(0); // NodeKind::File
    node.push(0);
    node.extend_from_slice(&0b01u16.to_le_bytes()); // isExported present, false
    node.extend_from_slice(&1u32.to_le_bytes());
    node.extend_from_slice(&line_count.max(1).to_le_bytes());
    node.extend_from_slice(&0u32.to_le_bytes());
    node.extend_from_slice(&0u32.to_le_bytes());
    for (o, l) in [n, q, i] {
        node.extend_from_slice(&o.to_le_bytes());
        node.extend_from_slice(&l.to_le_bytes());
    }
    for _ in 0..6 {
        node.extend_from_slice(&NONE.to_le_bytes());
        node.extend_from_slice(&0u32.to_le_bytes());
    }
    node.extend_from_slice(&NONE.to_le_bytes()); // extra_json
    node.extend_from_slice(&0u32.to_le_bytes());
    node.extend_from_slice(&0u32.to_le_bytes()); // metrics
    debug_assert_eq!(node.len(), NODE_ROW_SIZE);
    let mut meta = Vec::with_capacity(META_SIZE);
    meta.push(KERNEL_ABI_VERSION);
    meta.extend_from_slice(&[0u8; 3]);
    for v in [1u32, 0, 0, arena.len() as u32, NONE, 0] {
        meta.extend_from_slice(&v.to_le_bytes());
    }
    meta.extend_from_slice(&0f64.to_le_bytes());
    EmitOut { meta, nodes: node, edges: Vec::new(), refs: Vec::new(), arena }
}

enum KernelOutcome {
    Ok { emit: EmitOut, facts: Facts, had_error_tree: bool, facts_failed: bool },
    TooDeep,
    TimedOut,
    Failed(String),
}

fn run_kernel(rel: &str, src: &str, lang: Lang, cfg: &ExtractConfig) -> KernelOutcome {
    cg_kernel::hooks::set_parse_timeout(Some(cfg.parse_timeout));
    cg_kernel::hooks::clear();
    let opts = ExtractOptions { allow_error_trees: cfg.allow_error_trees };
    let res = catch_unwind(AssertUnwindSafe(|| cg_kernel::extract_file(rel, src, lang.as_str(), &opts)));
    let timed_out = cg_kernel::hooks::parse_timed_out();
    let tree = cg_kernel::hooks::take_tree();
    match res {
        Err(p) => KernelOutcome::Failed(format!("panic: {}", panic_message(&p))),
        Ok(Err(KernelError::Defer(m))) => {
            if m.contains("nesting too deep") {
                KernelOutcome::TooDeep
            } else {
                // `allow_error_trees = false` path: treat as a parse error with no tables
                KernelOutcome::Failed(m)
            }
        }
        Ok(Err(KernelError::Failed(m))) => {
            if timed_out {
                KernelOutcome::TimedOut
            } else {
                KernelOutcome::Failed(m)
            }
        }
        Ok(Ok(emit)) => {
            let had_error_tree = tree.as_ref().map(|t| t.root_node().has_error()).unwrap_or(false);
            let mut facts_failed = false;
            let facts = match &tree {
                Some(t) => {
                    let r = catch_unwind(AssertUnwindSafe(|| facts::extract(lang, t, src, FileRows::new(&emit))));
                    match r {
                        Ok(f) => f,
                        Err(_) => {
                            facts_failed = true;
                            Facts::default()
                        }
                    }
                }
                None => Facts::default(),
            };
            KernelOutcome::Ok { emit, facts, had_error_tree, facts_failed }
        }
    }
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}

/// Only one dedicated big-stack retry at a time (design doc 4.4).
static BIG_STACK: Mutex<()> = Mutex::new(());
const BIG_STACK_BYTES: usize = 512 << 20;

fn run_kernel_big_stack(rel: &str, src: &str, lang: Lang, cfg: &ExtractConfig) -> KernelOutcome {
    let _g = BIG_STACK.lock().unwrap_or_else(|e| e.into_inner());
    std::thread::scope(|s| {
        match std::thread::Builder::new().stack_size(BIG_STACK_BYTES).spawn_scoped(s, || run_kernel(rel, src, lang, cfg))
        {
            Ok(h) => h.join().unwrap_or_else(|_| KernelOutcome::Failed("panic in big-stack retry".into())),
            Err(e) => KernelOutcome::Failed(format!("cannot spawn big-stack thread: {e}")),
        }
    })
}

/// Extract one file. `None` when the file vanished or is unreadable.
pub fn extract_one(entry: &FileEntry, cfg: &ExtractConfig) -> Option<FileBatch> {
    let bytes = std::fs::read(cfg.root.join(&entry.rel)).ok()?;
    Some(extract_bytes(&entry.rel, &bytes, entry.mtime_ns, entry.lang, cfg))
}

/// Extract from file contents already in memory (`lang_hint` is used if detection from the
/// text alone finds no supported language).
pub fn extract_bytes(rel: &str, bytes: &[u8], mtime_ns: i64, lang_hint: Lang, cfg: &ExtractConfig) -> FileBatch {
    let h = hash::hash128(bytes);
    let src = String::from_utf8_lossy(bytes);
    let lang = scanlang::detect(rel, Some(&src), &cfg.overrides);
    let lang = if lang.is_supported() { lang } else { lang_hint };
    let mut flags = 0u8;
    if generated::detect_generated(rel, &src) {
        flags |= fileflags::GENERATED;
    }
    if testpath::is_test_path(rel) {
        flags |= fileflags::TEST;
    }
    let mut errors = Vec::new();
    let mut outcome = run_kernel(rel, &src, lang, cfg);
    if matches!(outcome, KernelOutcome::TooDeep) {
        outcome = run_kernel_big_stack(rel, &src, lang, cfg);
    }
    let line_count = src.bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let (emit, facts) = match outcome {
        KernelOutcome::Ok { emit, facts, had_error_tree, facts_failed } => {
            if had_error_tree {
                flags |= fileflags::PARSE_ERROR;
            }
            if facts_failed {
                errors.push("panic while building facts".to_owned());
            }
            (emit, facts)
        }
        KernelOutcome::TooDeep => {
            errors.push("nesting too deep for the walker".to_owned());
            flags |= fileflags::PANICKED;
            (file_node_only(rel, line_count), Facts::default())
        }
        KernelOutcome::TimedOut => {
            errors.push(format!("parse timed out after {:?}", cfg.parse_timeout));
            flags |= fileflags::TIMED_OUT;
            (file_node_only(rel, line_count), Facts::default())
        }
        KernelOutcome::Failed(m) => {
            if m.starts_with("panic") {
                flags |= fileflags::PANICKED;
            } else {
                flags |= fileflags::PARSE_ERROR;
            }
            errors.push(m);
            (file_node_only(rel, line_count), Facts::default())
        }
    };
    let mut extra_refs = commonjs_require_refs(&src, lang);
    let (mut extra_nodes, mut sites) = (Vec::new(), Vec::new());
    if !cfg.hooks.is_empty() && flags & (fileflags::PANICKED | fileflags::TIMED_OUT) == 0 {
        let rows = FileRows::new(&emit);
        let ctx = HookCtx { rel, lang, src: &src, rows, facts: &facts };
        let kcount = rows.node_count() as u32;
        for hook in &cfg.hooks {
            if !hook.applies(lang, rel) || (!hook.triggers().is_empty() && !hook.triggers().iter().any(|t| src.contains(t))) {
                continue;
            }
            let mut out = HookOut::default();
            let r = catch_unwind(AssertUnwindSafe(|| hook.run(&ctx, &mut out)));
            if r.is_err() {
                errors.push(format!("panic in hook {}", hook.name()));
                continue;
            }
            // Hook-created nodes are numbered after the kernel's (and after earlier hooks').
            let base = kcount + extra_nodes.len() as u32;
            for mut r in out.refs {
                // refs from hook-created nodes are expressed relative to `kcount`
                if r.from_row >= kcount {
                    r.from_row = base + (r.from_row - kcount);
                }
                extra_refs.push(r);
            }
            for mut s in out.sites {
                if s.node_row != cg_core::NONE && s.node_row >= kcount {
                    s.node_row = base + (s.node_row - kcount);
                }
                sites.push(s);
            }
            for mut n in out.nodes {
                if n.parent_row >= kcount {
                    n.parent_row = base + (n.parent_row - kcount);
                }
                extra_nodes.push(n);
            }
        }
    }
    FileBatch {
        rel: rel.to_owned(),
        lang,
        size: bytes.len() as u32,
        mtime_ns,
        hash: h,
        flags,
        emit,
        extra_nodes,
        extra_refs,
        sites,
        facts,
        errors,
    }
}

fn build_pool(cfg: &ExtractConfig) -> rayon::ThreadPool {
    let low = cfg.low_priority;
    rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.threads.max(1))
        .stack_size(64 << 20)
        .thread_name(|i| format!("cg-extract-{i}"))
        .start_handler(move |_| {
            if low {
                lower_thread_priority();
            }
        })
        .build()
        .expect("extract thread pool")
}

/// Unix nice 10 for the calling thread (Linux applies `who = 0` to the thread; other
/// platforms keep their default priority until the platform-specific calls land in M6).
fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: setpriority only changes scheduling attributes of the calling thread.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// Extract all `entries`, feeding results to `sink` in input order. Files that vanished
/// are reported as `None`.
pub fn extract_stream(entries: &[FileEntry], cfg: &ExtractConfig, mut sink: impl FnMut(usize, Option<FileBatch>)) {
    let pool = build_pool(cfg);
    let mut start = 0usize;
    while start < entries.len() {
        let mut end = start;
        let mut bytes = 0u64;
        while end < entries.len() && (end == start || bytes + entries[end].size <= cfg.batch_bytes) {
            bytes += entries[end].size;
            end += 1;
        }
        let results: Vec<Option<FileBatch>> =
            pool.install(|| entries[start..end].par_iter().map(|e| extract_one(e, cfg)).collect());
        for (i, r) in results.into_iter().enumerate() {
            sink(start + i, r);
        }
        start = end;
    }
}

/// Convenience: all batches in memory, aligned with `entries`.
pub fn extract_all(entries: &[FileEntry], cfg: &ExtractConfig) -> Vec<Option<FileBatch>> {
    let mut out = Vec::with_capacity(entries.len());
    extract_stream(entries, cfg, |_, b| out.push(b));
    out
}

