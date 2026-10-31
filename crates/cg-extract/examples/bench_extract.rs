// Dev tool: scan + extract a repo, print totals and timings; used for M1 measurements.
//   cargo run --profile fastrel -p cg-extract --example bench_extract -- <repo> [threads]
use cg_extract::*;
use std::time::Instant;

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = std::path::PathBuf::from(&args[1]);
    let threads: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(2);
    let t0 = Instant::now();
    let scan = cg_scan::scan(&cg_scan::ScanOptions::new(&root));
    let t_scan = t0.elapsed();
    let mut cfg = ExtractConfig::new(&root);
    cfg.threads = threads;
    let t1 = Instant::now();
    let batches = extract_all(&scan.files, &cfg);
    let t_ext = t1.elapsed();
    let (mut nodes, mut edges, mut refs, mut bytes, mut perr, mut panics, mut missing) = (0, 0, 0, 0u64, 0, 0, 0);
    let mut by_lang = std::collections::BTreeMap::<&str, (usize, usize)>::new();
    let mut digest = 0u64;
    let (mut imports, mut bindings) = (0usize, 0usize);
    for b in &batches {
        let Some(b) = b else { missing += 1; continue };
        let r = b.rows();
        nodes += r.node_count();
        edges += r.edge_count();
        refs += r.ref_count() + b.extra_refs.len();
        bytes += b.size as u64;
        imports += b.facts.imports.len();
        bindings += b.facts.bindings.len();
        if b.flags & fileflags::PARSE_ERROR != 0 { perr += 1; }
        if b.flags & fileflags::PANICKED != 0 { panics += 1; }
        let e = by_lang.entry(b.lang.as_str()).or_default();
        e.0 += 1;
        e.1 += r.node_count();
        digest ^= cg_core::hash::hash64(&b.emit.nodes) ^ cg_core::hash::hash64(&b.emit.refs).rotate_left(7) ^ cg_core::hash::hash64(&b.emit.arena).rotate_left(13);
    }
    println!("files={} (unsupported {}, too_large {}, missing {}) src={:.1}MB", scan.files.len(), scan.unsupported, scan.too_large.len(), missing, bytes as f64 / 1e6);
    println!("nodes={nodes} edges={edges} refs={refs} imports={imports} bindings={bindings} parse_err_files={perr} panicked={panics}");
    println!("scan={:?} extract={:?} ({:.1} MB/s) threads={threads} peak_rss={}MB digest={digest:016x}", t_scan, t_ext, bytes as f64 / 1e6 / t_ext.as_secs_f64(), rss_kb() / 1024);
    println!("by language: {by_lang:?}");
}
