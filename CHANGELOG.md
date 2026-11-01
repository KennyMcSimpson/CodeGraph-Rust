# Changelog

## 0.1.0 — first release (milestones M0–M4)

**Engine**
- Static single binary `cgr`; vendored upstream tree-sitter kernel (TypeScript/JavaScript, Python,
  Go, Java, Rust, Kotlin) with three small patches, re-vendorable with `scripts/sync-kernel.sh`.
- Immutable, memory-mapped snapshot format (header, checksummed partitions, CSR edges, FST name
  indexes, trigram and BM25 indexes), published atomically; readers never block the writer.
- Reference resolution in rounds (module tables, export closure, scope/import/receiver/qualified
  strategies, "better unresolved than wrong"); route extraction for Express, Flask, FastAPI,
  Django, Gin, Spring, Axum, Next.js; synthesised dynamic-dispatch edges for interfaces and
  implementations, Go structural interfaces, event emitters, React render and JSX children.
- Incremental sync: delta layer over a base segment, re-resolution of only the affected
  references, thresholds that fall back to a full rebuild, automatic merge; the result equals a
  full rebuild (property test with random edit sequences, plus sabotage-checked rule coverage).
- File watcher with a lock-based election (one watcher per project across sessions).

**Agent surface**
- MCP server over stdio and HTTP: upstream tool schemas/descriptions/instructions with the
  `codegraph-rust_` prefix, explore-only default surface, `roots`/`--path`/`projectPath`
  precedence, success-shaped errors, 30 s call budget, in-flight calls answered on EOF.
- `cgr install` / `uninstall` for Claude Code, Cursor and Codex (comment-preserving JSONC/TOML
  edits, marker blocks, idempotent and reversible), Claude Code `UserPromptSubmit` hook.
- CLI: init, index, sync, status, query, explore, node, files, callers, callees, impact,
  implementations, affected, cycles, serve, install, uninstall, uninit, export --sqlite, version.
- Development tools: `debug dump|unresolved|diff`, `bench index|sync|explore|compare`,
  `scripts/bench-local.sh` (memory-guarded), `scripts/smoke.sh`.

**Release engineering**
- CI: rustfmt, clippy `-D warnings`, kernel re-vendoring check, tests on Linux/macOS/Windows,
  smoke test, static musl build. Release workflow for six targets with checksums; install scripts.

Not in this release: C/C++, C#, PHP, Ruby, Swift and the other upstream languages; Vue/Svelte;
code signing and notarisation; npm/PyPI wrappers; Homebrew tap. See docs/STATUS.md.
