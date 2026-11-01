//! `cgr`: CLI entry point (design doc 11.1).

mod cmds;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "cgr", version, about = "codegraph-rust: fast code graph for AI agents")]
struct Cli {
    /// Project directory (default: the current directory; the nearest `.cgr/` above it is used)
    #[arg(short = 'p', long = "path", global = true)]
    project: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create `.cgr/` and build the index
    Init {
        path: Option<PathBuf>,
        /// Keep `.cgr/` out of git through `.git/info/exclude` instead of its own `.gitignore`
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
        /// Print per-phase timings and peak memory
        #[arg(long)]
        timings: bool,
    },
    /// Incrementally update the index from the files that changed
    Sync {
        path: Option<PathBuf>,
        #[arg(long)]
        quiet: bool,
    },
    /// Show index statistics
    Status {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        /// Exit 1 when files changed since the last sync
        #[arg(long)]
        strict: bool,
        /// Verify every partition checksum
        #[arg(long)]
        verify: bool,
    },
    /// Search for symbols
    Query {
        search: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Explore: source, call paths and blast radius for a question or a bag of names
    Explore {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        #[arg(long)]
        max_files: Option<usize>,
    },
    /// Read a symbol (with trail) or a file
    Node {
        symbol_or_file: String,
        /// Include the symbol's source
        #[arg(long)]
        code: bool,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        symbols_only: bool,
    },
    /// Indexed file tree
    Files {
        dir: Option<String>,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long, default_value = "tree")]
        format: String,
        #[arg(long)]
        no_metadata: bool,
        #[arg(long)]
        max_depth: Option<usize>,
    },
    /// Functions that call a symbol
    Callers {
        symbol: String,
        #[arg(long)]
        file: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Functions a symbol calls
    Callees {
        symbol: String,
        #[arg(long)]
        file: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Symbols affected by changing a symbol
    Impact {
        symbol: String,
        #[arg(long)]
        file: Option<String>,
        #[arg(short, long, default_value_t = 2)]
        depth: u32,
    },
    /// Implementations / subtypes of an interface, abstract class or method
    Implementations {
        symbol: String,
    },
    /// Test files affected by changes to the given files
    Affected {
        files: Vec<String>,
        /// Read file names from stdin, one per line
        #[arg(long)]
        stdin: bool,
        #[arg(short, long, default_value_t = 5)]
        depth: u32,
        /// Only report files matching this glob
        #[arg(short, long)]
        filter: Option<String>,
        #[arg(short = 'j', long)]
        json: bool,
        #[arg(short, long)]
        quiet: bool,
    },
    /// Import cycles (exit 1 when any exist)
    Cycles {
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Print version information
    Version {
        #[arg(long)]
        verbose: bool,
    },
    /// Development helpers
    #[command(subcommand)]
    Debug(DebugCmd),
}

#[derive(Subcommand)]
enum DebugCmd {
    /// Print edges (except `contains`) as sorted lines
    Dump,
    /// Print unresolved references
    Unresolved,
}

/// Restore the default SIGPIPE action so `cgr ... | head` ends quietly instead of panicking.
fn reset_sigpipe() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn main() {
    reset_sigpipe();
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> Result<i32> {
    let base = cli.project;
    match cli.cmd {
        Cmd::Init { path, local_only } => cmds::init(path.or(base), local_only),
        Cmd::Index { path, force, quiet, timings } => cmds::index(path.or(base), force, quiet, timings),
        Cmd::Sync { path, quiet } => cmds::sync(path.or(base), quiet),
        Cmd::Status { path, json, strict, verify } => cmds::status(path.or(base), json, strict, verify),
        Cmd::Query { search, kind, limit, json } => cmds::query(base, &search, kind, limit, json),
        Cmd::Explore { query, max_files } => cmds::explore(base, &query.join(" "), max_files),
        Cmd::Node { symbol_or_file, code, file, offset, limit, symbols_only } => cmds::node(base, &symbol_or_file, code, file, offset, limit, symbols_only),
        Cmd::Files { dir, pattern, format, no_metadata, max_depth } => cmds::files(base, dir, pattern, &format, !no_metadata, max_depth),
        Cmd::Callers { symbol, file, limit } => cmds::callers(base, &symbol, file, limit, true),
        Cmd::Callees { symbol, file, limit } => cmds::callers(base, &symbol, file, limit, false),
        Cmd::Impact { symbol, file, depth } => cmds::impact(base, &symbol, file, depth),
        Cmd::Implementations { symbol } => cmds::implementations(base, &symbol),
        Cmd::Affected { files, stdin, depth, filter, json, quiet } => cmds::affected(base, files, stdin, depth, filter, json, quiet),
        Cmd::Cycles { path, json } => cmds::cycles(path.or(base), json),
        Cmd::Version { verbose } => cmds::version(verbose),
        Cmd::Debug(DebugCmd::Dump) => cmds::debug_dump(base),
        Cmd::Debug(DebugCmd::Unresolved) => cmds::debug_unresolved(base),
    }
}
