# codegraph-rust (`cgr`)

A code knowledge graph for coding agents, delivered as **one static binary**: no Node.js, no
daemon, no database server. `cgr` indexes a repository into an immutable, memory-mapped snapshot
and answers an agent's questions about it (`explore`, `callers`, `impact`, …) over MCP or from
the command line.

It is a Rust re-implementation of the engine behind
[colbymchenry/codegraph](https://github.com/colbymchenry/codegraph): it reuses upstream's
tree-sitter extraction kernel (vendored, see [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES)) and keeps
the MCP tool names, descriptions and server instructions in the same shape (with the
`codegraph-rust_` prefix), so the agent behaviour that was tuned upstream carries over.

[中文说明](README.zh-CN.md) · [Architecture](docs/ARCHITECTURE.md) · [Measured results](docs/BENCHMARKS.md) · [Status against the design](docs/STATUS.md)

## Install

```sh
# Linux / macOS (downloads a release archive and verifies its checksum)
curl -fsSL https://raw.githubusercontent.com/NianJiuZst/codegrap-rust/main/scripts/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/NianJiuZst/codegrap-rust/main/scripts/install.ps1 | iex

# from source
cargo install --path crates/cgr --locked
```

## Use

```sh
cd your-project
cgr init                       # build the index into ./.cgr (it ignores itself in git)
cgr explore "how does login reach the database"
cgr install                    # register the MCP server with Claude Code, Cursor and Codex
```

`cgr install` edits the agents' own configuration (`~/.claude.json`, `~/.cursor/mcp.json`,
`~/.codex/config.toml`, plus the instruction block in `CLAUDE.md` / `AGENTS.md`) in place, keeps
comments and unrelated entries, never overwrites an entry you pinned, and is exactly reversible
with `cgr uninstall`. `--local` configures only the current project, `--dry-run` shows the changes.

Agents talk to `cgr serve --mcp` (stdio) — or `cgr serve --http 127.0.0.1:7878 --token …` for
agents in containers. By default one tool is listed, `codegraph-rust_explore`; open the others
with `CGR_MCP_TOOLS=explore,node,callers,callees,impact,files,search,status`.

The index follows your edits: the MCP server watches the tree and `cgr sync` does the same by
hand. A sync re-resolves only the references an edit can change; the result is byte-for-byte what
a full rebuild would produce (checked by a property test and by `cgr debug diff`).

## Commands

| command | what it does |
|---|---|
| `cgr init [path] [--local-only]` | create `.cgr/` and index the project |
| `cgr index [--timings]` / `cgr sync` | full rebuild / incremental update |
| `cgr status [--json] [--strict] [--verify]` | counts, pending changes, checksum verification |
| `cgr explore <query…>` | the MCP `explore` output, verbatim |
| `cgr query <text> [--kind K] [--json]` | symbol search |
| `cgr node`, `files`, `callers`, `callees`, `impact`, `implementations` | the other tools on the command line |
| `cgr affected [files…] [--stdin]` | test files affected by a change |
| `cgr cycles` | import cycles (exit 1 if any) |
| `cgr serve --mcp` / `--http ADDR` | MCP server |
| `cgr install` / `uninstall` / `uninit` | agent integration and cleanup |
| `cgr export --sqlite out.db` | the graph as a SQLite file for ad-hoc SQL (schema in `crates/cgr/src/export.rs`) |
| `cgr debug dump\|unresolved\|diff`, `cgr bench index\|sync\|explore\|compare` | development tools |

Exit codes: 0 success, 1 failure (`cycles` found cycles, `status --strict` found pending changes),
2 usage error.

## Languages and frameworks

TypeScript / JavaScript (incl. TSX/JSX), Python, Go, Java, Rust and Kotlin. Route extraction for
Express, Flask, FastAPI, Django, Gin, Spring, Axum and Next.js; dynamic-dispatch edges for
interfaces and implementations, event emitters, and React render / JSX children.
C/C++, C#, PHP, Ruby, Swift and the other upstream languages are not in this release.

## Configuration

| variable | meaning |
|---|---|
| `CGR_THREADS`, `CGR_MEMORY_MB` | worker threads and the memory budget for indexing (default: cores and a share of RAM) |
| `CGR_MCP_TOOLS` (`CODEGRAPH_MCP_TOOLS`) | tools to list, comma separated |
| `CGR_EXPLORE_DEDUP=1` | skip source already sent in the same conversation (only for hosts with one durable context per connection) |
| `CGR_NO_PROMPT_HOOK=1` | disable the Claude Code prompt hook |
| `CGR_LOG` | log filter for `serve` (stderr), default `warn` |
| `CGR_FSYNC` | `full` (strongest, F_FULLFSYNC on macOS) or `off` for index publishing; default is a plain fsync |
| `CGR_HTTP_TOKEN` | bearer token for `serve --http` |

Project settings live in `cgr.json` or the upstream-compatible `codegraph.json` at the project root
(`cgr.json` wins field by field): `exclude`, `include`, `deprioritize` (gitignore-style patterns),
`includeIgnored`, and `extensions` (extra extension → language mappings). A malformed file only
produces a warning.

## Build and test

```sh
cargo test --workspace
cargo clippy --workspace --exclude cg-kernel --all-targets -- -D warnings
cargo build --release -p cgr          # fat LTO; `--profile fastrel` links in a fraction of the time
scripts/smoke.sh target/release/cgr   # init → explore → sync → MCP → hook → export on a tiny project
scripts/bench-local.sh --help         # memory-guarded benchmarks for a laptop
```

The vendored kernel in `crates/cg-kernel` is upstream plus three small patches;
`scripts/sync-kernel.sh <upstream checkout>` regenerates it and CI verifies that it reproduces.

## License

MIT. The extraction kernel is © Colby Mchenry (MIT), see [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
