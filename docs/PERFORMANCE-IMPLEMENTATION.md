# Performance implementation / 性能优化实现记录

[English](#english) · [简体中文](#简体中文)

## English

This update implements the next engine performance work from the technical design. It follows
the design's correctness rule: an incremental graph must match a fresh full build. The previous
upstream comparison in [the historical report](BENCHMARKS-HISTORICAL.md) measured `fcc59af`;
it does **not** measure the changes described here. The subsequent
[main retest and oracle report](BENCHMARKS.md) measures `b827b1d`: **4.95×** geometric-mean
full-index speedup and **5.73×** warm-query P50 speedup over the pinned upstream revision.
The strict graph-agreement gate remains unmet in all four samples.

### Implemented changes

| Design area | Implementation and effect on work performed |
|---|---|
| §3.3 resource control | macOS memory detection uses Mach statistics; Windows uses `GlobalMemoryStatusEx`. Explicit thread counts remain capped by the memory-derived worker limit. Project/module table construction now uses the configured pool, rather than the global Rayon pool. Index workers use Linux nice, macOS utility QoS or Windows below-normal priority. |
| §4.6 extraction spill | Accumulated kernel buffers reaching half the extraction share are written as an immutable temporary chunk in `.cgr/tmp/` and mapped back. One mapping/file backs a chunk of source files. All five ABI buffers remain byte-identical; owned facts remain resident. I/O failure stops the pipeline before the next batch and leaves its existing buffers intact. |
| §7.2 fused scanning | Hook trigger literals are compiled once into a shared Aho-Corasick automaton. Source is scanned once to select applicable hooks, including overlapping/shared literals. Hook order and output numbering remain deterministic. |
| §6.4 resolution | Full-build workers write outcomes directly into disjoint preallocated file ranges. The former per-file vectors, flattened round vectors and subsequent copies are removed. Optional field and file-context caches have budget-derived byte limits; cache eviction recomputes answers rather than discarding candidates. |
| §7.2 synthesis | Event keys can link in parallel, with budget-derived chunks of at most 256 keys and deterministic merging. Incremental event links reconnect old/new changed keys; React/JSX links reconnect changed files. Definition, export, import-target or hierarchy changes use a conservative full synthesis fallback. |
| §5.4 BM25 storage | Posting lists store node-ID differences as varints plus field masks and term frequencies. A separate document-frequency partition preserves BM25 statistics. Total document length is persisted, avoiding an all-node sum on query startup. Decoding is allocation-free and rejects truncated/overflowing entries. |
| §5.5 edge assembly | Large edge sets can use stable integer radix sorting of `(src, kind, dst, line, col)`, skipping constant bytes. Duplicate groups retain the original class/pass/metadata precedence. Small sets or insufficient scratch memory use comparison sorting. Both dense and sparse CSR builders use the same ordering. |
| §5.7 compaction | Compaction repacks `View(base + delta)` into a new base without source access, extraction or resolution. It preserves live facts, reference outcomes, provenance, positions and metadata, and rebuilds dense IDs/search indexes. Readers retain their previous complete generation. |
| §5.7 maintenance | Saves publish a delta first. After at least 30 seconds without a published change, maintenance can compact at >5% affected files (bases of at least 40 files), 64 MiB, 200 syncs, or 10 minutes of idle time. The watcher checks maintenance every 30 seconds and yields immediately to an existing writer. |
| §9.1–9.3 event sync | Ordinary file events update the file inventory by path without walking/statting every source file. Configuration/directory events, >500 dirty paths, watcher errors, rescan flags and queue overflow fall back to a full scan. A 1,024-event queue and 500-path set are bounded; sustained events flush within five seconds. Periodic scans recover missing events. |
| §4.6 / §8.3 source windows | `SourceView` keeps one bounded owned UTF-8 text buffer and lazy newline offsets instead of allocating a string per line. Files are reread for each call. Mutable editor files are not mapped; immutable snapshots/spills are. CRLF and unterminated final lines retain the previous line semantics. |
| §10 service execution | Four query workers have a 16-job admission queue. Saturation returns a readable busy result while control requests remain responsive. Thirty-second deadlines include queue time; cancellation notifications and hot-path checkpoints stop cooperative work. Queued cancelled work does not open a project. One background rebuild is admitted at a time; LRU eviction also releases its watcher resource. |
| §10.5 / §11.2 lifecycle | EOF gives final short calls 200 ms, then requests cancellation and stops watchers against one shared 600 ms deadline. Hook discovery skips an unindexed filesystem/home root, avoids directory symlinks, and caps depth/directories/entries/results. Discovery has a 200 ms deadline and the compute path a cooperative 450 ms deadline. The smoke helper preserves MCP stdin on macOS Bash 3.2 and cleans up its timer. |

### Compatibility

The snapshot **format is now version 2** because token posting records changed. Rebuild an old
index with `cgr index /path/to/project`. A format-1 segment is rejected by the format guard;
it is not reinterpreted as compressed data. Source files and agent configuration are unaffected.
The MCP background rebuild path coalesces repeated requests for an unreadable/old-format index.
The engine revision is also 2: package/module representatives are ordered by source path rather
than appended delta IDs. Package and declaration imports persist their file dependencies, so
adding/removing a package's representative invalidates its consumers correctly. Compaction
also handles advisory dependencies that still name the last removed base file.

### Validation and measurement status

Local regression checks cover byte-identical 1-thread/4-thread snapshots, resident/spilled ABI
buffers and segments, radix/comparison ordering and CSR equivalence, malformed posting lists,
native compaction without a source root and after source removal, old-reader survival, future
incremental sync, exact watcher paths, directory/config events, dropped-event recovery, queue
saturation, queued/running cancellation, rebuild admission and project LRU cleanup. Random and
targeted multi-language edit sequences compare canonical graphs after every edit.
The final randomized check uses the CI depth of six sequences of 100 edits, including package
membership changes; targeted cases explicitly add and remove a lexically earlier Go file.

The final macOS run passed **137 tests, with one ignored**, plus the locked release build,
Clippy with `-D warnings`, formatting and the complete smoke script under macOS Bash 3.2.
Four independent real-project copies passed partition checksum verification and canonical
full-build equivalence after indexing, adding a function, syncing and restoring the source:

| Project | Files | Nodes | Edges | Initial / edited / restored graph equals full build |
|---|---:|---:|---:|---|
| Express | 142 | 866 | 2,527 | Pass / Pass / Pass |
| Gin | 99 | 2,436 | 7,921 | Pass / Pass / Pass |
| Tokio | 814 | 14,460 | 36,511 | Pass / Pass / Pass |
| Django | 2,984 | 62,660 | 185,506 | Pass / Pass / Pass |

For all four projects, file/node/edge counts, sorted non-containment edge dumps and unresolved
reference dumps also match the retained pre-update binary. This checks regression against our
own prior engine; it does not establish upstream oracle parity. A separate 64-file fixture
verified automatic watcher delta publication, MCP refresh to the added function, full-build
equivalence and exit within a two-second functional deadline after stdin closed. An actual
format-1 index was rejected by the new reader and rebuilt into a verified format-2 index.
These are correctness checks, not performance samples. Code revision `f82506e` also passed
all nine [hosted CI jobs](https://github.com/NianJiuZst/CodeGraph-Rust/actions/runs/41286327676):
Linux/macOS/Windows unit/integration tests and end-to-end smoke, lint, kernel reproducibility
and a static musl build. Windows passed 137 tests with one ignored. Local/hosted checks use
Rust 1.99; the retained before/after release binaries use the same local Rust 1.94.1 toolchain.

At implementation completion, the guarded performance run was **refused before starting**.
The initial preflight reported
4,233 MiB available memory, 1,559 MiB swap and 40,800 MiB free disk. A repeat after the final
release build at that stage reported 5,889 MiB available memory, 977 MiB swap and 39,027 MiB free disk;
available memory still fell below the required 6,144 MiB (swap must be below 1,024 MiB and free
disk at least 20,480 MiB).
Protection thresholds were not relaxed, and no new speedup was published at that stage.
After the branch was merged and memory was released, the subsequent restricted-tier main
retest completed **128 timed commands** and full upstream edge comparisons; both implementations'
restored edge sets matched their initial sets, and all four `cgr` checksum/full-build checks passed.
See [the new report](BENCHMARKS.md) for measured medians, raw samples and oracle failures.
Standard-tier, concurrent-reader and large-repository RSS acceptance remain outside that run.
The earlier **6.40×** result remains historical and uses a different protocol.

To isolate before/after effects of these optimizations, use separate corpus/output directories for old and new binaries,
the same release profile and thread/memory settings, and the protected wrapper. For example:

```sh
scripts/bench-local.sh --tier restricted --cgr /path/to/cgr-before \
  --repos-dir /path/to/corpus-before --kinds index,sync,explore \
  --runs 3 --edits 200 --out /path/to/results-before --history /path/to/history-before \
  express gin tokio django
scripts/bench-local.sh --tier restricted --cgr /path/to/cgr-after \
  --repos-dir /path/to/corpus-after --kinds index,sync,explore \
  --runs 3 --edits 200 --out /path/to/results-after --history /path/to/history-after \
  express gin tokio django
```

Compare graph coverage and returned content alongside medians, P50/P95/P99, segment sizes and
process-tree RSS. Use the standard tier as a separate series. Keep raw one-off results outside
the source checkout.

### Remaining boundaries

- `CGR_MEMORY_MB` is a tuning budget. Kernel spill and cache limits do not impose an OS RSS
  limit; facts, graph builders and shared resolution tables still scale with repository size.
- Path-driven sync avoids filesystem-wide probing, but still assembles an in-memory inventory.
  Its delta is computed against the base, so earlier files in the current delta can be extracted
  again. Hierarchy changes still conservatively widen round-two resolution.
- Changed keys reduce linking work for implemented event/render rules. A persistent inverse
  index for every synthesis pass and finer definition/hierarchy invalidation remain future work.
  Missing callback/registry/Gin/Spring passes and framework coverage parity are separate work.
- Compaction uses the writer lock. Background compaction with delta rebasing is the design's M6
  work; it is not part of this update.
- Deadlines are cooperative. Blocking filesystem operations cannot be forcibly interrupted by
  a safe Rust thread cancellation mechanism. Source text has no cross-request LRU, preserving
  the requirement to read current disk contents on every call.

## 简体中文

本轮依据技术设计落地引擎性能优化，并继续以“增量结果等于全量重建”为正确性约束。
[历史 benchmark](BENCHMARKS-HISTORICAL.md) 测量的是 `fcc59af`，**不包含本轮新增优化**。
合并后的 `b827b1d` 已完成[main 性能复测与 oracle 对比](BENCHMARKS.md)：全量索引加速比几何平均
**4.95×**、热查询 P50 加速比几何平均 **5.73×**。四个样本的严格图谱一致率门槛仍未通过。

### 已落地的优化

| 设计章节 | 实现与减少的工作量 |
|---|---|
| §3.3 资源控制 | macOS 直接读取 Mach 内存统计，Windows 使用原生内存 API；显式线程数也受内存预算约束。模块/项目表进入配置的线程池。索引线程分别使用 Linux nice、macOS utility QoS、Windows 低于正常优先级。 |
| §4.6 提取降级 | 累计 kernel 缓冲达到提取份额一半后，按批写入 `.cgr/tmp/` 并通过不可变映射读取，释放堆缓冲。一批文件共享一个映射，五类 ABI 缓冲保持逐字节一致；facts 仍驻留内存。写入失败会在下一批开始前终止。 |
| §7.2 融合扫描 | 将所有 hook 的触发字面量一次编译为共享 Aho-Corasick 自动机，一次扫描选择需运行的 hook，保留重叠匹配、执行顺序与节点编号。 |
| §6.4 解析结果槽 | 全量解析直接写入互不重叠的预分配结果区，消除每文件结果、每轮展平数组及再次复制。字段缓存和文件上下文缓存受预算约束；淘汰只导致重算，不截断候选集。 |
| §7.2 合成连接 | 事件键按预算分批并行连接，每批最多 256 键，按固定顺序合并。普通增量只重连变化事件键和变化文件的 React/JSX 站点；定义、导出、导入目标、继承结构变化时保守回退。 |
| §5.4 BM25 存储 | 节点 ID 差值 + varint 压缩，保留字段掩码和词频；独立保存文档频率。总文档长度持久化，查询启动不再遍历所有节点求和。解码不分配数组，处理截断和溢出记录。 |
| §5.5 边组装 | 大规模边可按整数唯一键使用稳定基数排序，跳过不变化的字节；同键边继续遵守原有来源、pass 和元数据优先级。小数据或临时空间不足时使用比较排序。 |
| §5.7 纯数据合并 | 直接从 base + delta 重写基础段，保留存活 facts、引用结果、来源、位置与元数据，重建密集 ID 和搜索索引。合并不读取源码，不调用提取器或解析器，旧读者继续使用原快照。 |
| §5.7 静默维护 | 保存时先发布增量；至少静默 30 秒后，按受影响文件比例、64 MiB、200 次同步或空闲 10 分钟触发合并。文件比例阈值为 >5%，适用于基础段至少 40 文件的项目。维护遇到已有写者会立即让出。 |
| §9 事件同步 | 普通事件按具体路径更新文件清单；目录/配置变化、事件丢失、重扫标记或溢出时完整扫描。事件队列上限 1,024，脏路径上限 500，持续事件最多积累五秒；定期扫描兜底漏事件。 |
| §4.6 / §8.3 源码窗口 | 单份有界源码文本 + 延迟生成的行偏移，替代逐行字符串分配。每次调用重读磁盘；可被编辑器原地修改的源码使用自有缓冲，不采用 mmap，保留 CRLF 和末行语义。 |
| §10 MCP 执行 | 四个查询线程 + 16 个排队任务；满载立即返回可读的繁忙说明，控制请求继续响应。排队计入 30 秒期限；取消通知和检查点让运行任务协作退出。后台重建合并为单个任务，项目 LRU 淘汰也释放监听资源。 |
| §10.5 / §11.2 生命周期 | EOF 给短请求 200 ms 收尾，然后取消剩余查询，所有监听器共享 600 ms 停止期限。提示钩子限制发现范围、数量和时间，跳过未索引根目录及目录符号链接。Smoke 脚本兼容 macOS Bash 3.2，保留 MCP stdin 并清理计时器。 |

### 兼容与验证

倒排记录变更使快照格式升级为 **version 2**。升级后运行 `cgr index /path/to/project` 重建旧索引；
格式检查会拒绝 version 1，避免按新结构误读。源码和 Agent 配置不受格式升级影响。
引擎修订也升级为 2：包/模块代表文件按源码路径排序，不依赖 delta 追加的 ID；包与声明导入持久化
对应文件依赖，增删代表文件时会正确使消费者失效。合并也能处理仍指向最后一个已删除基础文件的辅助依赖。

最终 macOS 回归为 **137 项通过、1 项忽略**，覆盖线程确定性、临时映射前后字节一致、压缩边界、排序与
双向 CSR、无源码合并、旧读者和后续同步、事件路径、目录/配置变化、漏事件恢复、排队与执行取消、
重建去重及 LRU 资源释放。随机和定向编辑序列在每一步对照全量图谱。锁定依赖的 release 编译、
Clippy `-D warnings`、格式检查和 macOS Bash 3.2 下的完整 smoke 检查均已通过。
最终随机检查采用 CI 的 **6 组 × 100 步编辑**，定向用例还覆盖增删排序靠前的 Go 包成员。

Express（142 文件）、Gin（99）、Tokio（814）、Django（2,984）的独立副本均通过分区校验；
初始索引、添加函数并同步、恢复源码并同步这三个状态，都与全量重建的 canonical 图谱一致。
四个项目的文件/节点/边数量、排序后的非 contains 边输出、未解析引用输出，也与保留的优化前二进制一致。
这是对自身旧引擎的回归检查，不代表与上游 oracle 的覆盖度已完全对齐。

额外的 64 文件项目验证了监听器自动发布 delta、MCP 读到新增函数、结果等于全量，以及关闭输入后在
2 秒功能期限内退出。真实的旧格式索引已验证会被新读者拒绝，并能通过 `index` 重建为校验通过的格式 2。
这些是正确性检查，不作为性能样本；macOS 结果也不等于 Linux/Windows 运行验证或远程 CI 已通过。

代码版本 `f82506e` 的[最终远程 CI](https://github.com/NianJiuZst/CodeGraph-Rust/actions/runs/41286327676)
已全部通过，共 9 个任务：Linux/macOS/Windows 单元/集成测试与端到端 smoke、格式和 Clippy、
内核来源复现及静态 musl 构建。Windows 同样为 137 通过、1 忽略。最终本机/远程检查使用 Rust 1.99；
保留的新旧 release 二进制均使用本机 Rust 1.94.1 构建。

开发交付时，正式性能复测在启动前被保护脚本拒绝。首次读数为可用内存 **4,233 MiB**、Swap **1,559 MiB**、
空闲磁盘 **40,800 MiB**；当阶段 release 编译后重试，读数为 **5,889 MiB / 977 MiB / 39,027 MiB**。
重试仍未达到可用内存至少 **6,144 MiB** 的要求；Swap 必须低于 **1,024 MiB**，空闲磁盘至少 **20,480 MiB**。
当时保护阈值保持原值，因此没有发布新增性能数字。分支合并、释放内存后，已完成受限档的
**128 次计时命令**及完整边 oracle 对比：双方恢复后的严格边集合均与初始集合相同，四个 `cgr`
项目的校验和及增量/全量检查全部通过。实测中位数、原始采样与未通过的 oracle 门槛见[新报告](BENCHMARKS.md)。
标准档、并发读者与大仓库 RSS 验收不在本轮范围内。历史 **6.40×** 的协议不同，仍只属于原被测版本。
要单独量化此次优化前后的收益，可按上方命令在独立目录逐一测旧、新二进制，保留相同源码、release 配置与
线程/内存档位，同时检查覆盖度和返回内容，再比较中位数、分位数、段大小与 RSS。

### 仍需继续推进的边界

- 内存预算仍不是操作系统 RSS 硬限制；facts、图谱构建数组和共享解析表仍随项目规模增长。
- 路径事件避免全仓库文件系统探测，但仍构造内存清单；当前 delta 相对 base 计算，之前变化过的文件
  可能再次提取。继承关系变化仍可能扩大第二轮解析范围。
- 已实现规则的事件/渲染键可以局部重连；全部 pass 的持久化反向键索引、更细的定义/继承失效分析，以及
  缺失的 callback、registry、Gin、Spring 合成规则和框架覆盖度对齐仍需后续工作。
- 当前合并仍持有写锁；后台合并与 delta 变基属于设计 M6。
- 取消是协作式的，不能强制中断阻塞文件系统调用。源码不使用跨请求 LRU，以保证每次调用读取当前内容。
