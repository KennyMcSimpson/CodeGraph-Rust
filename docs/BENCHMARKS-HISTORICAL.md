# Local performance comparison

Archived measurements of `fcc59af`, using four workers. For the subsequent merged-main retest
and strict upstream edge oracle, see [the current report](BENCHMARKS.md).

**Revision scope:** these measurements describe `fcc59af`, before the subsequent performance
implementation update. New compaction, event sync, spill, compression and service changes are
documented in the bilingual [implementation record](PERFORMANCE-IMPLEMENTATION.md). A new
guarded measurement was refused at preflight; a final repeat still had only 5,889 MiB available
memory against the required 6,144 MiB (swap used 977 MiB).
The historical results below have not been relabeled as measurements of the new revision.

Measured on **2026-11-01** against the upstream CodeGraph source. The tables below use only
the final measurement round, with AC power checked before and after every command. Earlier
exploratory rounds, during which the machine changed from battery to AC power, are excluded.

## Versions and environment

| Component | Version / setting |
|---|---|
| `cgr` | `fcc59af8853f1ff799df541309edfb19deb31eeb` |
| Upstream CodeGraph | [`087de4ca34b72bd64b59b49fd6edda2636c526c6`](https://github.com/colbymchenry/codegraph/tree/087de4ca34b72bd64b59b49fd6edda2636c526c6), package.json version 1.6.2 |
| Vendored extraction kernel | `9d3abb843a84d7610bf987bda7a1db4a6fe3eee3` |
| Machine | Apple M5, 10 cores, 16 GiB RAM, arm64 |
| OS | macOS 26.5.2 |
| Toolchains | Rust 1.94.1; Node 24.18.0 |
| Builds | `cgr`: release, fat LTO, opt-level 3; upstream: production JS build and release native kernel |
| Worker settings | `CGR_THREADS=4`; upstream parse and resolve workers each set to 4 |
| Memory settings | `CGR_MEMORY_MB=2048`; external process-tree RSS guard at 3,072 MiB |
| CLI runtime | Upstream used its production launcher flags: `--liftoff-only --disable-warning=ExperimentalWarning` |

Upstream's native Rust kernel was loaded and native routing was enabled for the measured
languages. The relevant extraction walkers, IDs and buffers did not change between the
vendored kernel revision and the upstream revision under test. This comparison measures the
full pipeline, including coordination, resolution, storage and queries.

Both implementations operated on independent copies of the same repositories. Identical
`codegraph.json` exclusions removed YAML/XML files that only upstream indexed. The final
indexed file sets and their source bytes were verified equal using SHA-256. Worker parameters
were matched; processes were not CPU-pinned or restricted to four physical cores.

| Repository | Commit | Matched files |
|---|---|---:|
| [Express](https://github.com/expressjs/express) | `1c6f1b0fe4000977b9a7b4f34097e4d527dd9824` | 142 |
| [Gin](https://github.com/gin-gonic/gin) | `0f09c3a9b4626d9fe9979cf4d0521d6ae32ab646` | 99 |
| [Tokio](https://github.com/tokio-rs/tokio) | `b499257ef62bd631b83a988d371f4330f545670c` | 814 |
| [Django](https://github.com/django/django) | `a05e16a7230fe33d3f43f4a3aaed103cebb42efc` | 2,984 |

## Full indexing

Each implementation ran one untimed warmup followed by three timed full CLI rebuilds, with
execution order alternated and one measured process running at a time. Compilation, downloads
and copying are excluded. OS file caches were retained. Elapsed time covers the complete
command; peak RSS comes from macOS `/usr/bin/time -l`.

| Repository | Upstream median | `cgr` median | Speedup | Peak RSS: upstream → `cgr` | RSS reduction |
|---|---:|---:|---:|---:|---:|
| Express | 560 ms | 106 ms | 5.30× | 278 → 35 MiB | 87.5% |
| Gin | 652 ms | 112 ms | 5.81× | 284 → 39 MiB | 86.2% |
| Tokio | 2,409 ms | 321 ms | 7.51× | 899 → 80 MiB | 91.1% |
| Django | 10,184 ms | 1,404 ms | 7.25× | 1,999 → 260 MiB | 87.0% |

Speedup is upstream elapsed time divided by `cgr` elapsed time, calculated before rounding.
Peak RSS values are also medians of the three runs. The equal-weight geometric mean of the
four indexing speedups is **6.40×**.

| Repository | Upstream min–max (ms) | `cgr` min–max (ms) |
|---|---:|---:|
| Express | 559.1–565.0 | 102.1–106.0 |
| Gin | 649.6–658.3 | 111.7–113.7 |
| Tokio | 2,368.6–2,453.4 | 319.2–338.2 |
| Django | 10,155.5–10,195.7 | 1,345.4–1,498.1 |

## Incremental sync

The following are CLI elapsed-time medians. No-change sync ran three times per implementation;
comment edits and symbol renames each ran four times. Both sides received the same edit
sequence, and original contents were restored and synced after each category.

| Repository | No changes: upstream → `cgr` | Comment edit: upstream → `cgr` | Symbol rename: upstream → `cgr` |
|---|---:|---:|---:|
| Express | 179.1 → 26.4 ms (6.79×) | 319.5 → 39.7 ms (8.05×) | 318.8 → 37.5 ms (8.50×) |
| Gin | 174.2 → 25.3 ms (6.90×) | 328.4 → 43.6 ms (7.54×) | 335.9 → 112.2 ms (2.99×) |
| Tokio | 204.1 → 36.6 ms (5.58×) | 487.3 → 58.2 ms (8.37×) | 511.7 → 53.4 ms (9.58×) |
| Django | 517.6 → 235.7 ms (2.20×) | 2,029.0 → 268.0 ms (7.57×) | 2,048.0 → 263.4 ms (7.77×) |

Edited files were `lib/application.js`, `gin.go`, `tokio/src/runtime/builder.rs`, and
`django/core/handlers/base.py`, respectively. The comment workload appended a comment; the rename
workload alternated an existing symbol's name and its references within that file. It does not
measure repository-wide refactoring. No-change sync still includes repository scanning.

## Explore queries

Each repository used 20 fixed-seed queries drawn from shared production symbols, including
single names, pairs and natural-language questions containing names. Eight queries warmed the
tool before 20 queries × 3 passes were timed. Warm measurements call the production explore
handler in-process: `cgr bench explore` and upstream `ToolHandler.executeReadTool`. They exclude
MCP transport, queueing and network latency.

The new-process CLI measurement used the same first four queries twice, for eight commands per
implementation, including startup but retaining warm OS file caches.

| Repository | Warm P50: upstream → `cgr` | P50 speedup | Warm P95: upstream → `cgr` | New-process CLI median: upstream → `cgr` | CLI speedup |
|---|---:|---:|---:|---:|---:|
| Express | 3.79 → 1.02 ms | 3.72× | 10.14 → 2.82 ms | 106.2 → 8.5 ms | 12.48× |
| Gin | 8.82 → 0.96 ms | 9.21× | 11.98 → 2.61 ms | 111.7 → 7.1 ms | 15.70× |
| Tokio | 15.93 → 3.28 ms | 4.86× | 22.09 → 6.23 ms | 152.5 → 9.8 ms | 15.60× |
| Django | 32.28 → 4.60 ms | 7.02× | 46.23 → 9.47 ms | 304.6 → 17.5 ms | 17.42× |

The geometric mean of warm-query P50 speedups is **5.85×**. Output lengths were not equalized:

| Repository | Upstream average output length | `cgr` average output length |
|---|---:|---:|
| Express | 12,028 | 6,371 |
| Gin | 12,919 | 11,393 |
| Tokio | 24,809 | 16,710 |
| Django | 24,434 | 23,544 |

Upstream reports JS string length; `cgr` reports UTF-8 bytes. These mostly ASCII outputs still
represent different returned content. Shorter output can contribute to lower query latency.

## Coverage and interpretation

The non-route nodes matched completely by the tuple of relative file, kind, name, start line
and start column. Route extraction and relationship resolution differed:

| Repository | Non-route nodes: upstream / `cgr` | Route nodes: upstream / `cgr` | Edges: upstream / `cgr` |
|---|---:|---:|---:|
| Express | 866 / 866 | 277 / 0 | 2,334 / 2,527 |
| Gin | 2,436 / 2,436 | 231 / 0 | 8,073 / 7,921 |
| Tokio | 14,460 / 14,460 | 0 / 0 | 39,682 / 36,511 |
| Django | 61,915 / 61,915 | 840 / 745 | 194,069 / 185,506 |

The missing Express/Gin routes in these samples are associated with framework detection being
gated by dependency manifests. Node-key agreement alone does not establish equal signatures,
metadata, resolved call graphs or answer quality. These workload timings should be considered
alongside coverage and returned content; no LLM, token-savings or agent-task benchmark was run.

After editing and restoring the real repositories, `cgr debug diff` matched a fresh full rebuild
in all four cases: 14,336, 24,824, 122,075 and 541,259 canonical lines, respectively.
`status --verify --strict` passed for each repository. The workspace test run passed 116 tests
with one ignored. The original smoke script failed because its background command lost MCP
stdin; an independent direct-stdin MCP probe received all three expected replies. A separate
unindexed-root prompt-hook probe was stopped after 88 seconds. The smoke run is not counted as
passed. Linux, Windows and hosted CI were not verified in this local run.

This is one machine with other applications running and existing swap use. The final round
recorded 416 AC-power checks around 208 external commands, including 184 timed commands.
Three full-index samples support the reported medians and ranges, rather than statistical
confidence intervals or a universal speedup guarantee.

Raw measurements, query lists, exclusions, source digests, logs and the comparison harness were
retained outside this source checkout. The project-provided `cgr bench index`, `bench sync`,
`bench explore`, `debug diff` and `scripts/bench-local.sh` support local measurement and
correctness checks; they do not by themselves reproduce the upstream comparison above.
