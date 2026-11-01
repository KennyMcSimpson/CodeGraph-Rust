# Resolution quality investigation / 解析质量排查与修复

## 结论与版本

这次排查发现了真实的漏连、误连，也发现了图模型和上游错误带来的对比偏差。
主要缺口位于自研的 facts、模块解析、类型传播及框架层。四个仓库的非路由节点身份
在修复前后均与上游完全一致；问题并不是 Rust 语法提取器整体提取不出符号。

修复运行时：`34c0024`，引擎规则版本从 **2 升到 3**，存储格式仍为 **2**。
基线为 `9c610ba`，其运行时代码与之前性能复测的 `b827b1d` 相同。
上游仍固定为 `d2e0389b519be398f9bc616b6f7bea8a7edfbc6a`，package 版本 1.6.2。
未更换上游版本、仓库快照、包含范围或比较键。

[完整机器可读结果](benchmarks/2026-11-01-resolution-quality.json)包含二进制 SHA-256、
源代码版本、逐边种类统计、验证结果及独立 SQL 交集计数。
[此前的性能报告](BENCHMARKS.md)继续描述其明确标注的被测版本；本轮没有产生新的性能加速比。

## 已修复的根因

### 1. CommonJS 导出链缺失

`require()` 原先主要绑定到模块文件，没有完整追踪可调用的 `module.exports`、
转导出、导出别名与对象字面量导出。Express 的入口再导出 `lib/express.js`，
后者导出 `createApplication`；这条链断开，直接导致 **678 次 `express()` 调用漏连**。

现在在语法 facts 中记录这些导出，并沿原有、有深度上限的导出闭包解析。
调用 CommonJS 模块时使用其默认导出，成员访问则使用成员导出。
ES namespace import 不会因此被当作可调用的默认函数。
未导出的 JS 私有函数不再作为模块成员的后备答案，缺失的显式 JS 导入也不会继续
退化成全项目同名匹配。显式声明的 `module` / `exports` 局部变量会阻止相应的导出事实。

相关实现：`cg-extract/src/facts/js.rs`、`cg-resolve/src/engine.rs`、`cg-resolve/src/resolve.rs`。

### 2. 接收者约束过弱

旧的成员兜底把普通函数也纳入候选。例如，Express 的
`examples/content-negotiation/index.js:36` 调用 `res.format()`，却被连到同文件
第 33 行定义的普通包装函数 `format(path)`，形成错误的自调用。

现在只有真实成员声明，或有对象成员赋值语法证据的 JS 函数，才进入成员兜底。
成员的对外属性名与函数的内部名称分别记录，静态字符串下标可以识别，动态下标不会
被当作字面属性名。已知类型的 `this/self` 查找限制在其类型层次内；显式类型注解
解析失败时，也不会改连到其他类型的唯一同名成员。
成员元数据、返回类型元数据均排除在普通词法变量查找之外。

未知、未注解的动态接收者仍可能采用低置信度的唯一成员候选；这不是完整类型检查。

### 3. Rust 模块根与限定路径不完整

库、主程序、bench、test、example 原先可能共用空模块路径。
按路径排序靠前的辅助文件可能抢占库根；自定义 `[lib] path` 也没有用于确定模块目录。
简单样例中，全局同名兜底会掩盖这些错误。

现在优先使用实际存在的 Cargo 根文件，隔离辅助目标，按声明的库路径确定目录。
限定路径解析保留模块后面的声明、成员部分，能解析返回注解中的
`tokio::runtime::Runtime`，并处理模块与值同名时的导出查找，例如 `tokio_stream::once`。
回归检查使用重复声明及导入别名，并核对目标文件，避免只凭名称相同判定成功。

### 4. Rust 条件宏中的导出与返回类型丢失

Tokio 的部分 `pub use` 位于 `cfg_* ! { ... }` 中，tree-sitter 将宏体视为 token tree。
旧 facts 遍历看不到这些导入、重导出，`Builder`、`Runtime` 等名称容易变成歧义或漏连。
内核的兼容性行为还会简化返回注解，导致泛型返回信息不足。

现在为条件 item 包装宏建立保持字节长度、换行及原始位置的遮罩视图，补充 facts。
只有发现相应宏才增加一次解析，源文件限 **1 MiB**，补充解析有 **100 ms** 取消期限，
遍历有深度上限；重解析失败或产生语法错误则回退。普通宏、宏定义及 `cfg_if!` 保持不透明，
不会执行宏或项目代码，也不改变内核产生的节点与引用。

另外保存原始 Rust 返回注解，按声明类型逐段传播链式调用、`Self`、标准
`Result/Option`、解包操作与 `?` 的值类型。标准容器在解包前不会被视为内部对象；
自定义同名 `Result` 不套用标准容器规则。静态方法不再被一律假定返回其所属类型。
传播限制为 16 层及 4 KiB 表达式路径，不做无限递归或逐引用源文件扫描。

### 5. 框架检测漏掉框架自身，路由扫描误读注释

旧检测主要查依赖列表，Express/Gin 自身通常不会依赖自身，所以对应规则没有启用。
现在同时识别精确的项目包名、Go module 身份；Express/Gin 路由扫描先遮罩注释，
避免把文档中的示例注册成真实路由。

这补齐了功能，但也暴露出两套图模型的差异：cgr 的路由 ID 与上游格式不同，
cgr 还产生上游没有的文件到路由 `contains` 边。因此本轮保留全部边时，Gin 的
上游相对 P 会下降，不能把新增边全部判成语义错误。

### 6. Django 默认 Manager 的返回对象需要框架证据

移除“静态工厂都返回所属类型”的宽泛猜测后，部分原本猜对的 ORM 调用也会丢失。
现在仅在实际解析到 Django `Model` 基类、且目标位于其继承层次中时，识别默认
`.objects` 的 `create/get/first/last/earliest/latest` 返回模型实例的约定。
不会仅因为出现 `.objects.create` 就把任意类当成 Django Model，也不把返回 tuple
或 QuerySet 的其他操作当成模型实例。

这个规则要求图中存在可解析的 Django Model 基类；外部依赖未索引、自定义 manager、
动态用户模型和动态生成的类仍有缺口。

### 7. 新增语义 facts 必须参与增量失效

只修改成员的对外名称、保留内部函数名称时，内核声明可能完全不变。
如果只比较内核符号，消费者会保留旧连线。

现在成员事实与原始返回注解也参与差异检测，触发相关名称、导入消费者的重新解析。
42 文件的定向复现中，修改 1 个成员别名后只重新解析 **2 个文件**，结果与完整重建一致。
规则版本提升后，旧版本索引在下一次 `sync` 时自动完整重建；已经实测从版本 2 升级。

## 同口径复测

比较键始终为 `(source stable ID, target stable ID, kind, line, column)`。
NULL 位置保持 NULL，包含全部边、路由、包含关系和有提取错误的文件，不按结果删样本。
四个快照的索引文件数分别为 142、99、814、2,984，复测及编辑还原后逐文件源字节一致。
独立 SQLite `INTERSECT` 再次复核了下列交集。

这里的 **P = 共有边 / cgr 边**，**R = 共有边 / 上游边**，均是对指定上游的相对一致性，
不是人工语义标注下的准确率。上游自身不被定义为语义真值。

| 项目 | 共有边：修复前 → 后 | 相对 P：前 → 后 | 相对 R：前 → 后 |
|---|---:|---:|---:|
| Express | 1,384 → 2,063 | 54.77% → 58.96% | 59.30% → 88.39% |
| Gin | 7,626 → 7,678 | 96.28% → 93.45% | 94.46% → 95.11% |
| Tokio | 31,541 → 33,638 | 86.39% → 87.14% | 79.48% → 84.77% |
| Django | 173,752 → 174,016 | 93.66% → 94.07% | 89.51% → 89.65% |

四仓合计新增 **3,224 条**与上游相同的边，同时减少 **132 条**原有共有边，
净增 **3,092 条**。被移除的共有边不能全部称为误连：其中也有保守类型推断带来的真实漏连，
仍需继续处理。所有非路由节点的身份一致性保持 **100%**。

为避免大量 `contains` 掩盖调用解析，单独列出 `calls`，但不改变全图比较分母：

| 项目 | 共有调用：修复前 → 后 | 调用相对 P：前 → 后 | 调用相对 R：前 → 后 |
|---|---:|---:|---:|
| Express | 419 → 1,097 | 27.03% → 49.26% | 32.73% → 85.70% |
| Gin | 3,430 → 3,482 | 99.42% → 99.66% | 90.10% → 91.46% |
| Tokio | 8,345 → 10,058 | 90.20% → 91.35% | 53.48% → 64.45% |
| Django | 43,533 → 43,829 | 90.10% → 91.11% | 76.61% → 77.13% |

**仍未全面达到原设计 P ≥ 97%、R ≥ 95% 的门槛。** 不应据此声称已经完全兼容上游，
也不应把所有上游独有边当作 cgr 的错误。已核对的反例是 `express.urlencoded`：
上游将它连到 `createApplication`，而该版本源码实际把它赋给 `bodyParser.urlencoded`。
类似地，cgr 的某些真实 `app.use` 调用与路由包含边，在上游没有对应边。

## 验证与剩余工作

- 35 项仓库外定向语义检查：旧版通过 16 项，修复版通过 **35 项**，包含正例与拒绝误连的负例。
- 现有工作区测试：**137 通过，1 项原有 ignored**；随机编辑按 6 组 × 100 次执行。
- 四个真实仓库共 **24 项**检查：初始完整比较、编辑后同步与完整比较、还原后同步与完整比较、严格索引校验。
- 成员别名变更的增量复现、旧规则版本索引升级均通过。
- Rust 1.99 Clippy `-D warnings`、格式检查、锁定依赖的 release 构建通过。
- 新增的一次性脚本、测试项目、SQLite 数据库与逐边日志保存在仓库外；仓库只提交运行时代码和报告数据。

后续优先补齐 Rust 泛型/trait/Deref 与更复杂宏的静态建模，Python 跨方法字段与动态工厂，
以及 JS 接收者、动态导出和 mixin 的类型证据。路由协议的 ID、边种类和包含关系需要另行明确
兼容性约定，再决定是否迁移；不在比较脚本里悄悄归一化。
下一轮性能复测必须针对补齐能力后的运行时，继续采用相同的受控配对方法。
本次增加了解析工作量，没有把旧 README 的性能结果改标为新版本结果。

## English summary

The low agreement exposed both real resolver defects and differences between graph models.
Non-route symbol identities already matched upstream completely; most gaps were in the
Rust implementation's facts, export/module resolution, type propagation and framework rules.

Runtime commit **`34c0024`** fixes callable CommonJS exports and re-exports, enforces JS
export boundaries, distinguishes assigned property names from internal function names,
and constrains member lookup using receiver evidence. Rust fixes select the declared Cargo
root, isolate auxiliary targets, resolve qualified type paths and value/module name collisions,
recover conditional-wrapper export facts, and preserve raw return annotations for bounded
call-chain and standard Result/Option inference. Framework detection recognizes a framework's
own package/module; Express/Gin route scans ignore comments. Django default-manager inference
requires the resolved Model hierarchy. New semantic facts participate in incremental invalidation.

The tables above use the **unchanged strict edge key and complete edge sets** against the
same pinned upstream snapshot. Shared edges increase by **3,092 net**: 3,224 additions and
132 removals. The remaining shared-edge removals include genuine coverage gaps as well as
heuristic differences; they are not all claimed as correctness improvements. All non-route
node identities still match. P/R describe agreement with this upstream version, which is not
semantic ground truth. Gin's relative P declines partly because cgr now emits route containment
edges absent from upstream. The original global 97%/95% agreement gate is **not fully met**.

Validation: **35/35** external focused checks, **137** existing workspace tests passed with
one existing ignored test, **600** random edit steps, **24** checks across four real repositories,
an independently checked SQL intersection for every graph, and the targeted incremental alias
and old-engine upgrade checks. Release build, strict Clippy and formatting pass. Engine version
is **3**; format version stays **2**, and `sync` rebuilds older indexes automatically.

Remaining work includes richer generics/trait/Deref inference, dynamic factories and field
flow, dynamic exports/mixins, auxiliary Rust target module trees, and an explicit route
compatibility contract. The README's previously pinned performance measurements are retained
as historical measurements. This quality run makes **no new performance claim**.
