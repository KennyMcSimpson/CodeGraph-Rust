# CodeGraph Rust (`cgr`)

**A local code knowledge graph for AI coding agents, built in Rust.**

[English](README.md) · [简体中文](README.zh-CN.md) · [Architecture](docs/ARCHITECTURE.md) · [Benchmarks](docs/BENCHMARKS.md) · [Changelog](CHANGELOG.md)

CodeGraph Rust indexes a repository into a persistent graph of symbols, references and
relationships. It gives developers and coding agents a way to retrieve relevant source,
follow calls across files, inspect dependencies and estimate the impact of a change through
one command-line executable or an MCP connection.

The engine runs locally and builds its graph from source syntax and deterministic analysis.
Indexing and querying require no hosted service, embedding API, model credentials, Node.js
runtime or database server. The normal index is stored in the project's `.cgr/` directory;
query results include source windows from the working tree.

This is an independent Rust re-implementation of the engine behind
[colbymchenry/codegraph](https://github.com/colbymchenry/codegraph). It reuses the upstream
Tree-sitter extraction kernel and adapts the agent-facing tool schemas and guidance, with
`codegraph-rust_` as the MCP tool prefix. Storage, resolution, incremental synchronization,
queries and service integration are implemented in this Rust workspace. Current coverage
differs from upstream; see [current limitations](#current-limitations).

## Contents

- [Capabilities](#capabilities)
- [Measured performance](#measured-performance)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Language and framework support](#language-and-framework-support)
- [Command-line reference](#command-line-reference)
- [Agent and MCP integration](#agent-and-mcp-integration)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Index lifecycle](#index-lifecycle)
- [Development and verification](#development-and-verification)
- [Current limitations](#current-limitations)
- [Contributing](#contributing)
- [Acknowledgements and license](#acknowledgements-and-license)

## Capabilities

- **Source-backed exploration.** `explore` combines symbol search, call paths, related source
  and change impact in one bounded response. Queries can contain natural-language questions,
  symbol names, qualified names or file names; specific names help identify a flow.
- **Cross-file code navigation.** Inspect callers, callees, implementations, imports and
  dependencies, including supported framework routes and heuristic dynamic-dispatch links.
- **Incremental indexing.** Re-extract changed files and re-resolve affected references.
  Larger changes and configuration changes can trigger a full rebuild.
- **Efficient persistent storage.** Immutable memory-mapped segments, dense integer IDs,
  string pools, bidirectional CSR edges, and prebuilt FST, trigram and BM25 search indexes.
- **CLI and MCP access.** Shared query handlers serve terminal workflows and agent clients.
  Built-in installers support Claude Code, Cursor and Codex.
- **Updates during agent sessions.** The MCP server can watch indexed projects, debounce
  file changes and publish new snapshots. Run `cgr sync` manually when no watcher is running.
- **Inspectable results.** Index health and checksums, unresolved references, incremental/full
  graph comparisons and optional SQLite exports are available from the CLI.

## Measured performance

In a local comparison on **2026-11-01**, `cgr` achieved a **6.40× geometric-mean full-index
speedup** across four repositories, with **86.2%–91.1% lower peak indexing RSS** than upstream
CodeGraph. Both versions indexed the same source files, with upstream's native Rust extraction
kernel enabled.

| Repository | Files | Upstream full index | `cgr` full index | Speedup | Peak RSS: upstream → `cgr` |
|---|---:|---:|---:|---:|---:|
| Express | 142 | 560 ms | 106 ms | **5.30×** | 278 → 35 MiB |
| Gin | 99 | 652 ms | 112 ms | **5.81×** | 284 → 39 MiB |
| Tokio | 814 | 2,409 ms | 321 ms | **7.51×** | 899 → 80 MiB |
| Django | 2,984 | 10,184 ms | 1,404 ms | **7.25×** | 1,999 → 260 MiB |

Warm `explore` P50 latency improved by **3.72×–9.21×**; new-process `explore` CLI calls by
**12.48×–17.42×**. Sync after a single-file comment edit improved by **7.54×–8.37×**, and
single-file symbol renames by **2.99×–9.58×**.

Measured on an Apple M5 (10 cores, 16 GiB RAM), macOS 26.5.2, on AC power, using release builds
and four parse/resolve workers per implementation. Full-index numbers are medians of three
timed CLI runs after warmup, with warm OS file caches; speedup is upstream time divided by
`cgr` time. Tested revisions: `cgr` **`fcc59af`**, upstream **`087de4c`** (Node 24.18.0).

Route coverage and query output lengths differ between the implementations, including missing
Express/Gin route nodes in `cgr` for these samples. These differences affect the work performed
and must be considered alongside the timings. See [benchmark methodology and coverage](docs/BENCHMARKS.md)
for exact revisions, query and sync results, and validation details.

### Query latency in the same local comparison

| Repository | Warm `explore` P50: upstream → `cgr` | Warm speedup | New-process CLI speedup |
|---|---:|---:|---:|
| Express | 3.79 → 1.02 ms | 3.72× | 12.48× |
| Gin | 8.82 → 0.96 ms | 9.21× | 15.70× |
| Tokio | 15.93 → 3.28 ms | 4.86× | 15.60× |
| Django | 32.28 → 4.60 ms | 7.02× | 17.42× |

Warm measurements cover the in-process explore handler, excluding MCP transport and queueing.
New-process CLI measurements include startup. The warm-query P50 speedup geometric mean is
**5.85×**. These measurements cover repository analysis and retrieval; no LLM task-completion
or token-savings benchmark was run.

### Performance engineering update

The next engine update adds event-path synchronization, source-free snapshot compaction after
a quiet period, extraction-buffer spill to temporary mappings, preallocated resolution outcomes,
fused hook scanning, selective/parallel event synthesis, compressed BM25 postings and adaptive
radix edge sorting. Query source windows use one bounded text buffer with lazy line offsets.
MCP now has bounded admission, cooperative cancellation and bounded project/watcher resources;
prompt-hook discovery and the smoke helper's stdin/timer handling have been fixed.

These changes use **snapshot format 2**. Run `cgr index /path/to/project` to rebuild an old index.
The historical speedups above still refer to `fcc59af`. A new guarded benchmark was refused
because available memory and swap did not meet the design's prerequisites, so this update has
no additional measured speedup claim. See the bilingual
[implementation and validation record](docs/PERFORMANCE-IMPLEMENTATION.md) for exact changes,
resource readings and remaining work.

## Installation

### Build and install from source

Source installation is the currently available path. Requirements are **Rust 1.89 or newer**,
Cargo, Git and a C compiler toolchain for the Tree-sitter grammars.

```sh
git clone https://github.com/NianJiuZst/CodeGrap-Rust.git
cd CodeGrap-Rust
cargo install --path crates/cgr --locked
cgr version --verbose
```

Cargo installs `cgr` into its binary directory, normally `~/.cargo/bin`. Make sure that directory
is on your `PATH`. Rust and the C compiler are build-time requirements; the resulting executable
runs without a Rust toolchain or Node.js.

For a local build without installing:

```sh
cargo build --release --locked -p cgr
./target/release/cgr --help
```

On Windows, the executable is `target\release\cgr.exe`.

### Release installers

The repository includes release packaging and checksum-verifying download scripts. **As of
2026-11-01, no GitHub Release has been published.** The following commands require published
archives and checksum files on the [Releases page](https://github.com/NianJiuZst/CodeGrap-Rust/releases).
Until those assets are available, use the source installation above.

```sh
# Linux / macOS — requires a published release
curl -fsSL https://raw.githubusercontent.com/NianJiuZst/CodeGrap-Rust/main/scripts/install.sh | sh
```

```powershell
# Windows, PowerShell 7+ — requires a published release
irm https://raw.githubusercontent.com/NianJiuZst/CodeGrap-Rust/main/scripts/install.ps1 | iex
```

Release automation defines these targets:

| Platform | Architectures | Packaging |
|---|---|---|
| Linux | x86_64, ARM64 | Static musl executable, `.tar.gz` |
| macOS | Intel x86_64, Apple Silicon ARM64 | Executable, `.tar.gz` |
| Windows | x86_64, ARM64 | MSVC executable, `.zip` |

The scripts accept `CGR_VERSION` to select a version and `CGR_INSTALL_DIR` to choose the install
location. Defaults are `~/.local/bin` on Linux/macOS and `%LOCALAPPDATA%\Programs\cgr` on Windows.

## Quick start

Run these commands in the repository you want to analyze:

```sh
cd /path/to/your-project
cgr init
cgr status --verify
```

Explore an area using names from that project's source:

```sh
cgr explore "AuthService loginUser SessionStore"
cgr query loginUser --json
cgr callers loginUser
cgr callees loginUser
cgr impact loginUser --depth 2
```

`AuthService`, `loginUser` and `SessionStore` are illustrative names; replace them with your
own symbols. You can also ask a question such as `cgr explore "how does login reach the database"`.
The response uses the indexed graph and relevant source, with line numbers and an output budget.

Register the MCP server with supported agents:

```sh
cgr install --dry-run
cgr install
```

The installer connects the agent; `cgr init` builds each project's index. When you use only
CLI commands, update the graph after edits with `cgr sync`. While an MCP server is running
with watching enabled, it performs that synchronization automatically after a debounce period.

## Language and framework support

### Source languages

| Language | Recognized supported extensions |
|---|---|
| TypeScript / TSX | `.ts`, `.mts`, `.cts`, `.tsx` |
| JavaScript / JSX | `.js`, `.mjs`, `.cjs`, `.jsx`, `.xsjs`, `.xsjslib` |
| Python | `.py`, `.pyw` |
| Go | `.go` |
| Java | `.java` |
| Rust | `.rs` |
| Kotlin | `.kt`, `.kts` |

Extraction records source structure, symbols and references. Resolution uses available scope,
import, receiver-type, module and package information. Coverage depends on language constructs
and project configuration; this is a static code graph rather than a full compiler type checker.

### Framework and dynamic-dispatch rules

Route hooks are implemented for **Express, Flask, FastAPI, Django, Gin, Spring, Axum and Next.js**.
Framework detection uses project manifests, so route coverage can differ when dependencies are
absent or when indexing a framework's own repository. The benchmark documents observed gaps.

Supported synthesis passes include interface/implementation dispatch, Go structural interfaces,
event-emitter handlers, React re-render links and JSX parent/child links. These are bounded
heuristics with recorded provenance; they cover specific patterns rather than arbitrary
runtime behavior.

C/C++, C#, PHP, Ruby, Swift and the other upstream language families are outside this version's
supported extraction set. Vue/Svelte-specific extraction is also outside the current scope.

## Command-line reference

Use `cgr --help` or `cgr <command> --help` for the full option list. `--path` / `-p` selects a
project explicitly, for example `cgr --path /path/to/project explore "UserService save"`.
Query commands can locate an index in the current directory or an ancestor.

| Command | Purpose |
|---|---|
| `cgr init [path]` | Create `.cgr/` and build a full index |
| `cgr index [path] --timings` | Rebuild the full graph and report phase timings |
| `cgr sync [path]` | Reconcile additions, edits and deletions with the index |
| `cgr status --json --verify` | Report counts, changes and snapshot checksum verification |
| `cgr status --strict` | Return a nonzero status when the index has pending changes |
| `cgr explore <query…>` | Retrieve related source, call paths and impact |
| `cgr query <text> --kind function --json` | Search symbols, optionally filtering by kind |
| `cgr node <symbol-or-file> --code` | Inspect a symbol or file and its source |
| `cgr files` | List indexed files and structure |
| `cgr callers <symbol>` / `cgr callees <symbol>` | Follow incoming or outgoing calls |
| `cgr impact <symbol> --depth 2` | Inspect the graph neighborhood affected by a change |
| `cgr implementations <symbol>` | Find implementations or subtypes |
| `cgr affected <files…> --json` | Find test files connected to changed files |
| `cgr cycles --json` | Report import cycles; exit 1 when cycles are found |
| `cgr export --sqlite graph.sqlite` | Export a queryable SQLite graph |
| `cgr serve --mcp` / `cgr serve --http ADDR` | Start an MCP service |
| `cgr install` / `cgr uninstall` | Add or remove agent integration |
| `cgr uninit [path] --yes` | Remove the project's index |
| `cgr version --verbose` | Show build, format and kernel information |

Normal success returns 0; operational failures and conditions such as detected cycles return 1;
invalid command-line usage returns 2. Source windows are read from the current working tree,
so keeping the graph synchronized matters after edits.

## Agent and MCP integration

### Automatic configuration

`cgr install` detects supported agents and edits their existing configuration while preserving
comments and unrelated entries. Existing user-pinned commands and settings are retained;
missing integration keys are added. `uninstall` removes the owned registration and marked
instruction blocks.

```sh
cgr install --agent claude-code
cgr install --agent cursor
cgr install --agent codex
cgr install --local --agent claude-code
cgr install --no-hook
cgr uninstall --agent cursor
```

| Agent | Global registration | Project-local behavior |
|---|---|---|
| Claude Code | MCP registration, instructions, optional prompt hook and explore permission | `.mcp.json`, `CLAUDE.md`, `.claude/settings.json` |
| Cursor | `~/.cursor/mcp.json`, with workspace path passed to the server | `.cursor/mcp.json` with the project path |
| Codex | MCP registration in `~/.codex/config.toml` and instructions | `--local` edits project `AGENTS.md`; register the server globally separately |

Claude Code honors `CLAUDE_CONFIG_DIR`; Codex honors `CODEX_HOME`. Use `--dry-run` to inspect
changes and `--bin` to specify a different executable path. `--no-hook` disables registration
of the optional Claude Code prompt hook; `--no-permissions` skips the explore permission entry.

### Manual stdio configuration

For an MCP client with an `mcpServers` JSON configuration, use absolute paths appropriate to
that machine:

```json
{
  "mcpServers": {
    "codegraph-rust": {
      "command": "/absolute/path/to/cgr",
      "args": ["serve", "--mcp", "--path", "/absolute/path/to/project"]
    }
  }
}
```

Initialize that project with `cgr init` first. Clients can supply MCP roots; tools also accept
`projectPath` for selecting another indexed project. Use `--path` for clients that do not
provide a project root. Add `--no-watch` when the server should serve snapshots without
watching source changes.

By default, the tool list exposes **`codegraph-rust_explore`**. Set `CGR_MCP_TOOLS` in the server's
environment to enable additional tools:

```sh
CGR_MCP_TOOLS=explore,node,callers,callees,impact,files,search,status cgr serve --mcp
```

| Tool | Purpose |
|---|---|
| `codegraph-rust_explore` | Relevant source, named-symbol flows and impact |
| `codegraph-rust_node` | Symbol or file details and source |
| `codegraph-rust_callers` / `codegraph-rust_callees` | Call relationships |
| `codegraph-rust_impact` | Change impact neighborhood |
| `codegraph-rust_files` | Indexed file tree |
| `codegraph-rust_search` | Symbol locations |
| `codegraph-rust_status` | Index health and pending changes |

### HTTP transport

HTTP-capable clients can connect to `/mcp`:

```sh
export CGR_HTTP_TOKEN="replace-with-your-token"
cgr serve --http 127.0.0.1:7878 --path /absolute/path/to/project
```

This listens at `http://127.0.0.1:7878/mcp` and requires `Authorization: Bearer <token>` when a
token is configured. The token can also be supplied with `--token`. Keep the binding and token
appropriate for the clients that should access the project's source.

## Configuration

### Project settings

Place `cgr.json` at the project root. The upstream filename `codegraph.json` is also read;
`cgr.json` takes precedence for fields it supplies.

```json
{
  "exclude": ["**/generated/**", "**/fixtures/**"],
  "include": ["packages/shared/src/**"],
  "deprioritize": ["third_party/**"],
  "includeIgnored": false,
  "extensions": {
    ".customts": "typescript"
  }
}
```

| Field | Meaning |
|---|---|
| `exclude` | Gitignore-style patterns excluded from indexing, including tracked files |
| `include` | Re-include user-ignored paths; this is not a source-file whitelist |
| `deprioritize` | Keep matching paths indexed but rank them below first-party code |
| `includeIgnored` | Include embedded Git repositories during discovery |
| `extensions` | Map extra extensions to existing language parsers |

Explicit `exclude` rules take precedence over `include`; built-in directory skips still apply
when re-including paths. Invalid configuration produces warnings. Extension mappings select
an existing parser and do not add support for a new language.

Discovery applies ignore rules and skips common dependency/build directories. Source files
larger than 1 MiB are excluded from extraction. Generated files and test paths are recorded as
flags for downstream ranking and analysis.

### Environment variables

| Variable | Behavior |
|---|---|
| `CGR_THREADS` | Indexing worker count; the detected memory budget can reduce it |
| `CGR_MEMORY_MB` | Tuning budget for workers, batches, kernel spill and resolver caches; not an RSS cap |
| `CGR_MCP_TOOLS` | Comma-separated list of exposed MCP tools; `CODEGRAPH_MCP_TOOLS` is a fallback |
| `CGR_EXPLORE_DEDUP=1` | Skip source already returned in the same durable MCP conversation |
| `CGR_NO_PROMPT_HOOK=1` | Disable the optional Claude Code prompt hook at runtime |
| `CGR_HTTP_TOKEN` | Require a bearer token on HTTP requests |
| `CGR_LOG` | Service log filter, default `warn`; logs go to stderr |
| `CGR_TIMINGS` | Enable CLI indexing phase timing output |
| `CGR_FSYNC` | Default platform syncing; `full` selects strongest syncing, `off` skips syncing |

Defaults derive indexing resources from the host's available memory and CPU parallelism.
For example, `CGR_THREADS=4 CGR_MEMORY_MB=2048 cgr index --timings` requests a constrained
indexing configuration. Actual memory use also depends on graph size and intermediate data.

## Architecture

```text
source files → scan → parallel extraction + facts → merge → resolve + synthesize
                                                                    ↓
CLI / MCP ← query handlers ← base + delta view ← mmap segments ← publish snapshot
```

The snapshot format stores hot/cold node records, string pools, bidirectional CSR edges,
reference outcomes and search indexes. Publishing writes new immutable files and switches
the manifest; readers retain a stable snapshot, with shared locks protecting files from GC.

Incremental updates compare timestamps/sizes and content hashes, extract changed files, then
re-resolve references affected by changed definitions, imports, exports or hierarchy. One delta
is computed against the base; deltas do not accumulate as an unbounded stack. Quiet-period
compaction rewrites the live snapshot without re-extraction; broad reference changes or config
changes can still rebuild the graph. The correctness target is that the
incremental view equals a fresh full build, checked by canonical graph comparisons.

| Crate | Responsibility |
|---|---|
| `cg-core` | IDs, node/edge kinds, configuration, resource tuning and shared errors |
| `cg-kernel` | Vendored upstream extraction kernel and adaptation patches |
| `cg-scan` | File discovery, language detection and ignore rules |
| `cg-extract` | Parallel extraction, facts, error isolation and hooks |
| `cg-store` | Segment format, indexes, snapshots, publishing and garbage collection |
| `cg-resolve` | Module/export tables, reference resolution, route hooks and synthesis |
| `cg-query` | Retrieval, traversal, source windows and shared tool handlers |
| `cg-sync` | Change detection, incremental updates and watcher coordination |
| `cg-mcp` | JSON-RPC, stdio and HTTP service transports |
| `cg-install` | Agent configuration and instruction-block editing |
| `cgr` | CLI, prompt hook, SQLite export and benchmark commands |

See [Architecture](docs/ARCHITECTURE.md) for storage and algorithm details.

## Index lifecycle

- `cgr init` creates `.cgr/` and indexes the project. Its internal `.gitignore` keeps index
  contents out of version control. `init --local-only` additionally uses `.git/info/exclude`
  when available.
- CLI queries open existing snapshots directly. The MCP process stays alive for its client
  and can watch source changes; it does not require a separate database service.
- `cgr sync` updates the graph after edits. `cgr index` performs a full rebuild, including after
  changes that require refreshed engine analysis. `status` reports index version and changes.
- `cgr export --sqlite graph.sqlite` writes a separate SQLite file for analysis; it does not
  replace the native snapshot backend. Add `--force` to replace an existing export.
- `cgr uninstall` removes agent integration. `cgr uninit --yes` separately removes the project's
  index. The source tree is retained.

The index and exported graphs contain code-derived information. Keep them with the access and
retention policy appropriate to the repository.

## Development and verification

```sh
cargo test --workspace --locked
cargo clippy --workspace --exclude cg-kernel --all-targets -- -D warnings
cargo build --release --locked -p cgr
```

For faster optimized development builds, use `cargo build --profile fastrel -p cgr`. The
release profile uses fat LTO; `fastrel` reduces link work and is a separate performance profile.

Focused correctness and measurement commands include:

```sh
cgr status --verify --strict
cgr debug unresolved
cgr debug diff
cgr bench index /path/to/project --runs 3 --json
cgr bench sync /path/to/project --edits 30 --seed 1 --verify --json
cgr bench explore /path/to/project --count 100 --runs 3 --json
scripts/bench-local.sh --help
```

`bench sync` edits a temporary copy by default; `--in-place` changes that behavior. `debug diff`
compares the current graph with a fresh full build. The benchmark wrapper adds memory, disk
and swap preflight checks and process-tree monitoring; its prerequisites may require closing
other applications before a run.

The performance implementation update passed **137 tests, with one ignored**, on macOS,
plus release compilation, Clippy, formatting and the CLI/MCP/hook smoke test. All four real
projects passed full/edit/restore graph equivalence and matched the prior engine's graph dumps.
The historical benchmark revision passed 116 tests. Current verification details and the benchmark preflight refusal are in
[the implementation record](docs/PERFORMANCE-IMPLEMENTATION.md). Linux/Windows runtime checks
and hosted CI results are separate from this local validation.

The CI configuration includes formatting, Clippy, kernel reproducibility, multi-platform tests
and release-build checks. The smoke helper covers the CLI, MCP handshake and prompt hook.
To refresh the kernel, use `scripts/sync-kernel.sh <upstream-checkout> <commit>` and verify the
resulting patches and extraction tests. Vendored formatting is kept separate from workspace
formatting checks.

## Current limitations

- **Coverage parity:** supported language families, framework routes, synthesized edges and
  explore responses differ from upstream. The benchmark has a per-repository coverage table.
- **Static-analysis limits:** ambiguity can leave references unresolved; dynamic behavior is
  covered only by implemented patterns. Inspect important call paths before relying on them.
- **Memory tuning:** kernel-buffer spill and resolver cache limits release heap memory, but
  facts, graph builders and shared tables still scale with repository size. The tuning budget
  does not enforce a process RSS cap.
- **Incremental scope:** filesystem event probing is local, but inventory assembly and the
  base-relative delta still do broader in-memory work. Structural changes can trigger wider
  resolution/synthesis; background compaction with delta rebasing remains M6 work.
- **Execution deadlines:** query/hook cancellation is cooperative; a blocking filesystem call
  cannot be forcibly interrupted by the worker deadline.
- **New performance targets:** the latest optimizations have correctness coverage; their
  throughput, P95/P99 latency and large-repository RSS still need a new guarded measurement.

## Contributing

Start with [Architecture](docs/ARCHITECTURE.md) and the owning crate. Keep graph changes focused,
include a regression case for changed resolution or synchronization behavior, and compare
incremental output with a full rebuild. Document supported constructs and observed coverage
when extending a language or framework rule. Run the relevant tests and formatting/Clippy
checks before submitting changes; kernel updates should remain reproducible from their pinned
upstream commit and patches.

## Acknowledgements and license

CodeGraph Rust builds on [Colby Mchenry's CodeGraph](https://github.com/colbymchenry/codegraph)
and its Tree-sitter extraction work. The vendored upstream kernel is MIT-licensed; its pinned
revision is recorded in `crates/cg-kernel/UPSTREAM_COMMIT`.

This project is distributed under the [MIT License](LICENSE). See
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) for upstream attribution and vendored components.
