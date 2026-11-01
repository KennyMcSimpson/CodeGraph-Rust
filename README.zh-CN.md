# codegraph-rust（`cgr`）

给编码 agent 用的代码知识图谱，交付物是**一个静态二进制**：不需要 Node.js，没有常驻进程，也没有数据库服务。
`cgr` 把仓库索引成不可变、可 mmap 的快照，agent 通过 MCP（或命令行）查询 `explore`、`callers`、`impact` 等。

它是 [colbymchenry/codegraph](https://github.com/colbymchenry/codegraph) 引擎的 Rust 重写：复用上游的 tree-sitter 提取内核
（已 vendoring，见 [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES)），MCP 工具名、描述和服务说明保持同样的形态（工具名前缀为 `codegraph-rust_`），
因此上游调出来的 agent 行为可以直接继承。

[English](README.md) · [架构](docs/ARCHITECTURE.md) · [实测结果](docs/BENCHMARKS.md) · [对照设计文档的完成情况](docs/STATUS.md)

## 安装

```sh
curl -fsSL https://raw.githubusercontent.com/NianJiuZst/codegrap-rust/main/scripts/install.sh | sh   # Linux / macOS
irm https://raw.githubusercontent.com/NianJiuZst/codegrap-rust/main/scripts/install.ps1 | iex        # Windows
cargo install --path crates/cgr --locked                                                              # 源码安装
```

## 使用

```sh
cd 你的项目
cgr init                       # 在 ./.cgr 建索引（目录自带 .gitignore，不会进 git）
cgr explore "登录是怎么走到数据库的"
cgr install                    # 把 MCP 服务注册到 Claude Code、Cursor、Codex
```

`cgr install` 就地修改各 agent 自己的配置，保留注释和无关条目，不覆盖你固定的键，可用 `cgr uninstall` 完全还原；
`--local` 只配置当前项目，`--dry-run` 只显示改动。agent 通过 `cgr serve --mcp`（stdio）连接，容器里的 agent 用
`cgr serve --http 127.0.0.1:7878 --token …`。默认只列出 `codegraph-rust_explore`，其余工具用 `CGR_MCP_TOOLS` 打开。

索引会跟随编辑：MCP 服务会监听文件变化，也可以手动 `cgr sync`。增量同步只重新解析一次编辑可能影响到的引用，
结果与全量重建逐项一致（由性质测试和 `cgr debug diff` 保证）。

## 语言与框架

TypeScript / JavaScript（含 TSX/JSX）、Python、Go、Java、Rust、Kotlin。路由提取：Express、Flask、FastAPI、Django、Gin、Spring、Axum、Next.js；
动态分派边：接口与实现、事件发射器、React 渲染与 JSX 子组件。C/C++、C#、PHP、Ruby、Swift 等本版本暂不支持。

更多命令、环境变量和构建方式见 [README.md](README.md)。
