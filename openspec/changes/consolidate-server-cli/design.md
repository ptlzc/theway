# Design: consolidate-server-cli

## Context

合并前:`crates/app` (theway, 核心: App 事件循环/会话/wire)、`crates/server` (theway-server: axum/tonic/ws + proto 编解码 + health)、`crates/cli` (theway-cli: main.rs bin)。查证: theway lib 唯一 Rust 消费方是 theway-cli bin;workmate-local 走协议 (gRPC/HTTP/WS 客户端),不嵌入。因此拆分的"可嵌入性"收益无真实消费者,合并由 feature 门控替代,不损失。

关键约束: 包名 `theway` 公开 API 不变;协议端点语义不变;`server → app` 依赖逻辑保留为 feature 内模块边界;`cargo build --workspace` 默认 headless (与合并前 theway-cli 之前的设定一致: default=[] + bin required-features)。

## Goals / Non-Goals

**Goals:**

- 6 → 4 crates;server/cli 职责收进 theway 包的 feature + bin。
- feature 矩阵: default=headless, `tui`, `server`;bin required-features=["tui","server"]。
- transport 模块回迁后保持"只通过公开面与核心交互"的边界 (spec)。
- 全量测试/clippy/fmt 绿;冒烟 (--web/--grpc + healthz + sessions) 通过。

**Non-Goals:**

- 不改 wire 模型、不改协议端点、不改核心架构 (事件循环仍留 app,transport 仍走 TransportEndpoints)。
- 不改变 Makefile/CI 的语义 (只补 feature 名)。
- 不合并 tui 与 server 的依赖 (各自 feature 独立门控)。

## Decisions

### Decision: server 回迁为 `transport` 模块 (server feature)

`crates/server/src/{grpc,http,ws,proto}.rs` + `http/tests/` → `crates/app/src/transport/`;`build.rs` (protox) → `crates/app/build.rs`。lib.rs 加 `#[cfg(feature = "server")] pub mod transport;`。模块内引用 `theway::wire` → `crate::wire`、`theway::ui::feed` → `crate::ui::feed`、`theway::session_ops` → `crate::session_ops` (同 crate 相对路径)。可见性: transport 用到的核心项 (TransportEndpoints 字段、wire 类型) 保持 pub 或降为 pub(crate) (同 crate 可见即可 — 优先降级,减少公开面)。

理由: 回迁后 pub(crate) 够用 (bin 通过包名访问 lib 公开面,transport 在 lib 内),公开面只保留 SDK 需要的 (wire/App/AgentSession/SessionOps 等)。

### Decision: bin 收回 theway 包

`crates/cli/src/main.rs` → `crates/app/src/main.rs`;`tests/cli_help.rs` → `crates/app/tests/`。Cargo.toml 加 `[[bin]] name="theway" path="src/main.rs" required-features=["tui","server"]`。main.rs 引用: 现为 `theway::xxx` (cli 时期),保持;`theway_server::...` → `theway::transport::...` (仅 server feature 下引用,main.rs 需 `#[cfg(feature="server")]` 处理 --web/--grpc 分支,或 bin 始终在 required-features 下编译 — 由于 bin required-features=["tui","server"],bin 编译时 server 恒开,无需 cfg)。

clap 等 bin-only 依赖回 app 的 [dependencies] (bin 与 lib 共享依赖表;clap 是轻依赖,可接受)。

### Decision: workspace 收尾

根 Cargo.toml members 移除 crates/server、crates/cli;Makefile 与 .github/workflows/ci.yml 的 `--features tui` → `--features tui,server`;docs 分层描述同步。

## Risks / Trade-offs

- [合并后 app 依赖表变重 (axum/tonic 在 feature 下)] → feature 门控保证默认/嵌入不携带;文档注明 `default-features = false`。
- [引用适配遗漏 (theway:: vs crate:: 混合)] → 编译错误定位 + grep 复核 (`theway_server` 应为空)。
- [可见性降级误伤 SDK 面] → 只降 transport 内部需要的项;wire/App 等公开面不动;cargo doc 校验。
- [Makefile/CI feature 名遗漏] → grep `--features` 复核。

## Migration Plan

1. **N1 server 回迁**: 文件移动 + feature 门控 + 引用适配 + build.rs。验收: `cargo build --workspace --features tui,server` + `cargo test -p theway --features server transport::`。
2. **N2 cli 回迁**: main.rs/tests 移动 + [[bin]] + clap 依赖 + `theway_server` 引用改 `theway::transport`。验收: `cargo build --workspace --features tui,server` (bin 产出)。
3. **N3 收尾**: members/Makefile/CI/docs。验收: `cargo build --workspace` (headless) 与 `--features tui,server` 均通过。
4. **N4 验证**: 全量 test/clippy/fmt + 冒烟 (--web healthz/state/sessions)。

回滚: 每步独立 commit;合并后协议面不变,回滚仅 revert。

## Open Questions

- bin-only 依赖 (clap 等) 放 lib 依赖表后,headless 嵌入者也编译 clap — 可接受 (clap 轻);若在意可用 `bin-dependencies` 特性 (Cargo 1.8x 无此能力),维持现状。
