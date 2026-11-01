//! `cgr`: CLI entry point (design doc 11.1).

use anyhow::{Context, Result};
use cg_core::brand;
use cg_store::{dump, Store};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "cgr", version, about = "codegraph-rust: fast code graph for AI agents")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create `.cgr/` and build the index
    Init {
        path: Option<PathBuf>,
        #[arg(long)]
        local_only: bool,
    },
    /// Build the index from scratch
    Index {
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        quiet: bool,
        #[arg(long)]
        timings: bool,
    },
    /// Show index statistics
    Status {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        verify: bool,
    },
    /// Development helpers
    #[command(subcommand)]
    Debug(DebugCmd),
}

#[derive(Subcommand)]
enum DebugCmd {
    /// Print edges (except `contains`) as sorted lines
    Dump { path: Option<PathBuf> },
    /// Print unresolved references
    Unresolved { path: Option<PathBuf> },
}

fn root_of(p: &Option<PathBuf>) -> Result<PathBuf> {
    let p = p.clone().unwrap_or_else(|| PathBuf::from("."));
    p.canonicalize().with_context(|| format!("cannot open {}", p.display()))
}

fn open_store(root: &Path) -> Result<Store> {
    Store::locate(root).with_context(|| format!("no {} index found from {} (run `cgr init`)", brand::DATA_DIR, root.display()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Init { path, local_only } => {
            let root = root_of(&path)?;
            let report = cg_resolve::index_project(&root, &Default::default())?;
            if local_only {
                let exclude = root.join(".git/info/exclude");
                if exclude.parent().map_or(false, |d| d.is_dir()) {
                    let cur = std::fs::read_to_string(&exclude).unwrap_or_default();
                    if !cur.lines().any(|l| l.trim() == ".cgr/") {
                        std::fs::write(&exclude, format!("{cur}{}.cgr/\n", if cur.ends_with('\n') || cur.is_empty() { "" } else { "\n" }))?;
                    }
                }
            }
            println!("indexed {} files, {} nodes, {} edges in {:.2}s", report.files, report.nodes, report.edges, report.total().as_secs_f64());
        }
        Cmd::Index { path, quiet, timings, .. } => {
            let root = root_of(&path)?;
            let report = cg_resolve::index_project(&root, &Default::default())?;
            if !quiet {
                println!(
                    "indexed {} files ({} unsupported, {} too large), {} nodes, {} edges, {}/{} references resolved",
                    report.files, report.unsupported_files, report.too_large_files, report.nodes, report.edges, report.resolved_refs, report.refs
                );
            }
            if timings || std::env::var(brand::env::TIMINGS).is_ok() {
                print!("{}", report.render_timings());
            }
        }
        Cmd::Status { path, json, verify } => {
            let root = root_of(&path)?;
            let store = open_store(&root)?;
            let snap = store.open_snapshot()?;
            if verify {
                snap.view.base.verify()?;
            }
            let m = &snap.manifest;
            if json {
                println!("{}", serde_json::to_string_pretty(m)?);
            } else {
                println!("files {}  nodes {}  edges {}  generation {}", m.n_files, m.n_nodes, m.n_edges, m.generation);
                for (name, n) in dump::ref_stats(&snap.view) {
                    println!("  refs {name:<12} {n}");
                }
            }
        }
        Cmd::Debug(DebugCmd::Dump { path }) => {
            let root = root_of(&path)?;
            let snap = open_store(&root)?.open_snapshot()?;
            for l in dump::dump_edges(&snap.view) {
                println!("{l}");
            }
        }
        Cmd::Debug(DebugCmd::Unresolved { path }) => {
            let root = root_of(&path)?;
            let snap = open_store(&root)?.open_snapshot()?;
            for l in dump::dump_unresolved(&snap.view) {
                println!("{l}");
            }
        }
    }
    Ok(())
}
