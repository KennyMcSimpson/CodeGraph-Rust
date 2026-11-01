# Resolution quality v4 / 解析质量与评测口径改进

本轮接续已合并的 [PR #2](https://github.com/NianJiuZst/CodeGraph-Rust/pull/2)。
运行时代码为 `0d48a07caa67791e64dccf015922a877ab3f9fee`，基线为合并后的
`4090af1`（规则版本 3）。引擎规则升到 **4**，存储格式保持 **2**；旧索引在下次同步时重建。

同一批四仓库源码、同一个上游快照下，全图共有边净增 **877**：新增 980，移除 103。
80 项定向断言由 **59/80 提升到 80/80**。这分别是上游一致性和定向语义检查，
不能把任一个分数解释为所有真实项目的语义准确率。

[机器可读证据](benchmarks/2026-11-01-resolution-quality-v4.json)记录逐种类计数、两种指标的
完整前后结果、二进制哈希、源码哈希、断言明细及独立 SQL 复核。
[v3 报告](RESOLUTION-QUALITY.md)和此前的性能结果继续保留原来的版本归属。

## 修复内容

### 同名声明与外部接收者

- JS、Python、Rust 的嵌套函数绑定保存实际声明节点。两个函数内部各有一个 `helper()` 时，
  不再都落到文件中出现的第一个同名声明。
- Rust 成员查找区分同一 crate 内不同模块的同名类型；跨模块 impl 需要能确认所属类型的导入。
  定向检查同时覆盖拒绝串用成员和保留合法的跨模块 impl。
- 由明确的外部导入构造出的未知对象，不再用本仓库唯一的同名成员填空。例如
  `reflect.ValueOf(x).Type()` 的局部变量不应连到测试代码中的 `Local.Type`。

### Rust 调用链、泛型与标准指针

兼容内核会把部分链式调用简化成裸方法名，丢掉接收者。facts 层现在记录有界的完整调用路径，
在相同引用位置补回名称；内核代码、节点身份和调用位置不变。有歧义的同位置映射不覆盖原名称。

这补齐了 `make().unwrap().execute()`、`make()?.execute()` 等形式；链式返回也统一使用
保存的原始返回信息，避免备用路径绕过容器约束。带生命周期的引用返回会先去掉引用与生命周期，
再解析目标类型。

Rust 类型 facts 保留泛型参数，可识别标准 `Box`、`Rc`、`Arc` 的内层类型和有界的单参数构造，
支持标准导入的别名。指针自身的已建模操作（例如 `Arc::clone`）不会被连到内层类型的 `clone`。
自定义同名类型仍按自己的声明解析。字段缓存只保存单一类型节点，因此不会把指针包装直接擦除后
用于字段方法查找；这部分保留保守行为。这里没有实现通用 `Deref` 或任意泛型替换。

### Python 工厂返回与增量失效

Python 的简单、带引号和受支持的可选返回注解参与解析。没有注解时，仅记录满足以下条件的返回：
末尾存在显式 `return`，所有显式返回都是同一个可追踪调用，且不属于生成器、异步函数或被装饰的函数。
混合返回不会被任选一个分支当作确定类型，嵌套函数的返回也不参与外层函数推断。

因此 `make()` 明确返回 `Worker()` 时，局部变量以及 `self.worker = make()` 这样的字段可获得类型。
返回表达式还参与增量语义签名：仅把 `return A()` 改成 `return B()`，消费者也会重新解析。
63 文件的定向检查中只重新解析 **2 个文件**，前后目标正确，且与完整重建一致。

### 条件宏补充解析不能静默丢事实

本轮完整索引与重建对比曾发现 `tokio-stream/src/stream_ext.rs` 丢失 7 个导入和 2 个重导出。
条件宏补充解析原有独立的 **100ms** 限时，会在负载下回退，改变同源代码的 facts。

补充解析现在遵守配置中的解析限时（默认每次解析 **5 秒**），仍有 1 MiB 源码大小限制和遍历深度限制。
若耗尽限时，会标记文件 `TIMED_OUT` 并记录补充解析错误。不会继续把回退结果呈现为完整提取。
修复后，四仓库的编辑、同步、还原和重建检查均通过；Tokio 在 1、2、4 个 worker 下的规范图相同。

## 指标：把目标一致性与参考覆盖范围分开

旧指标把上游没有的每一条边都计入相对 P 的分母。它适合检查严格兼容，
但上游并非完整语义真值。例如 v3 已能解析的许多 Express `app.use()` 调用，上游没有对应边。
两套路由表示的差异也继续影响全图分数。

本轮保留旧指标，另外报告**上游已标注调用点的目标一致性**：

```text
C = cgr 的完整 calls 边集合
O = 固定上游的完整 calls 边集合
site = (source stable ID, kind=calls, line, column)
E = C 中 site 在 O 里出现过的边

已标注点目标一致性 P* = |C ∩ O| / |E|
上游调用召回率 R       = |C ∩ O| / |O|
额外未标注边数         = |C − E|
```

边的目标仍须精确相同；位置的 NULL 与 0 仍有区别。R 的分母没有缩小。
`C − E` 单独列出，未获语义核验前既不算命中，也不直接判错。
P* 是条件一致性，不是全体输出的语义精确率，也不能替代严格兼容门槛。

**改指标带来的分数变化与改代码带来的提升分别比较。** Express 基线在新口径下已经是 99.28%，
本轮代码没有再提高其调用点分数，不能把旧口径的 49.26% 到新口径的 99.28% 写成算法提升。
上游在已标注点也可能错连，额外边也可能误连，仍需正负语义用例和后续人工抽样核验。

| 项目 | P*：v3 → v4 | 调用 R：v3 → v4 | v4 已标注点边数 / 全部调用边 | v4 额外未标注边 |
|---|---:|---:|---:|---:|
| Express | 99.28% → 99.28% | 85.70% → 85.70% | 1,105 / 2,227 | 1,122 |
| Gin | 99.86% → 99.86% | 91.46% → 91.46% | 3,487 / 3,491 | 4 |
| Tokio | 95.64% → 95.61% | 64.45% → 67.90% | 11,083 / 11,545 | 462 |
| Django | 97.42% → 97.80% | 77.13% → 77.44% | 44,998 / 48,128 | 3,130 |

## 原口径全图结果

仍使用 `(source stable ID, target stable ID, kind, line, column)`，包含全部边、路由、包含关系
和原有提取错误文件。索引源码文件的集合及字节与基线、上游完全一致。四仓文件数为
142、99、814、2,984；上游固定为 `d2e0389b519be398f9bc616b6f7bea8a7edfbc6a`。

| 项目 | 共有边：v3 → v4 | 全图相对 P：v3 → v4 | 全图相对 R：v3 → v4 |
|---|---:|---:|---:|
| Express | 2,063 → 2,062 | 58.96% → 58.93% | 88.39% → 88.35% |
| Gin | 7,678 → 7,678 | 93.45% → 93.49% | 95.11% → 95.11% |
| Tokio | 33,638 → 34,176 | 87.14% → 87.38% | 84.77% → 86.12% |
| Django | 174,016 → 174,356 | 94.07% → 94.23% | 89.65% → 89.83% |

移除的 103 条原有共有边包括同名声明重新绑定、外部对象误连的修正，也包括保守建模留下的覆盖缺口，
没有将其全部归为旧版错误。Express 减少的是一条回调函数引用；其共有调用数不变。
所有非路由节点身份仍与上游 **100%** 一致。

原设计的全图 **P ≥ 97%、R ≥ 95% 仍未全面达到**。Tokio 的 P* 也略有下降，
其新增调用和剩余接收者歧义需要继续核验。后续优先处理 Rust trait、泛型与字段包装传播，
以及 Python 动态类、跨方法字段流和有分支的工厂返回。

## 验证范围

以下均为本地验证，针对上述运行时；本轮没有产生新的性能加速比。

- 80 项定向断言：正例由 **30/46 → 46/46**，拒绝误连的负例由 **28/33 → 33/33**；另有 1 项路由检查。
  保留了上一轮 40 项检查，并加入同名干扰声明、错误目标拒绝和合法跨模块 impl 等检查。
  这是定向回归结果，不是随机真实项目样本的精确率估计。
- 工作区 **137 通过、1 项原有 ignored**；随机编辑按 **6 × 100** 次运行。
- 四仓共 **24 项**完整图、编辑同步、还原同步和严格结构校验；索引源码字节已还原。
- 63 文件工厂返回变更和规则版本 3 升级的定向增量检查通过。
- 独立 SQLite 复核每仓严格共有边、已标注调用点边数和共有调用数；没有超时文件。
- Tokio 在 1、2、4 个 worker 下重复比较规范图，全部相同；补充解析正常预算与零预算的 2 项定向检查通过。
- 锁定依赖的 release 构建、Rust 1.99 严格 Clippy、CI 包列表格式检查和现有 CLI/MCP 冒烟检查通过。

一次性检查程序、测试项目、数据库和日志保留在仓库外。仓库只包含运行时代码、已有 facts 测试中
泛型保留契约的更新，以及这份报告和结果数据。两个 README 和既有性能数字未改动。

## English summary

Engine 4 fixes scoped nested-function bindings, cross-module Rust type-name collisions,
external-initializer member fallback, lost Rust call paths, lifetime returns, bounded standard
pointer inference, Python factory return facts, and return-expression invalidation. Supplementary
Rust parsing now honors the configured timeout and exposes deadline exhaustion instead of
silently returning incomplete facts. Format version remains 2; engine-3 indexes rebuild on sync.

The unchanged strict oracle comparison gains **877 shared edges net** (980 added, 103 removed).
Not every removed shared edge is claimed to be a correctness improvement. The original global
97%/95% gate remains unmet. A separate, explicitly conditional metric evaluates target agreement
at upstream-labeled call sites and reports all additional unlabeled edges. Its higher denominator-
adjusted score is not presented as an algorithmic improvement or semantic ground truth.

Validation: **80/80** focused assertions versus 59/80, **137** workspace tests with one existing
ignored test, 600 random edits, 24 real-repository checks, independent SQL counts, targeted
incremental/upgrade checks, repeated Tokio graph equality with 1/2/4 workers, release build,
strict Clippy, formatting and CLI/MCP smoke. No new performance claim is made.
