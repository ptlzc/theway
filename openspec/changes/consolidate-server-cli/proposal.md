## Why

查证显示 theway lib 没有独立嵌入消费方 (workmate-local 是纯 gRPC/HTTP/WS 客户端,不依赖 Rust lib;唯一消费方是 theway-cli bin)。"app 可独立嵌入"的拆分理由是纸面价值,当前由依赖/编译隔离 + 关注点边界撑起的两个 crate (theway-server / theway-cli) 实际收益不足以支撑 6 crate 结构。合并回单一 `theway` 包 (feature 门控) 更简单且不损失可嵌入性。

## What Changes

- **theway-server → `theway` 的 `server` feature**:`transport/` 模块 (grpc/http/ws/proto + http/tests) 与 `build.rs` (protox) 回迁 `crates/app`;axum/tonic/prost/tonic-prost/tokio-stream 依赖挂到 `server` feature 下;`#[cfg(feature = "server")] pub mod transport;`。
- **theway-cli → `theway` 的 `[[bin]]`**:`main.rs` + `tests/cli_help.rs` 回迁;`[[bin]] name = "theway" required-features = ["tui", "server"]`;clap 等 bin-only 依赖回 app。
- **workspace**:members 移除 `crates/server`、`crates/cli` (6 → 4 crates: llm-provider/core/app/mcp)。
- **引用适配**:server 内 `theway::wire` → `crate::wire`、`theway::ui` → `crate::ui`;cli 的 `theway_server::` → `theway::transport::`;可见性放宽 (同 crate 用 pub(crate))。
- **构建入口**:Makefile / CI 的 `--features tui` → `--features tui,server`。

## Capabilities

### New Capabilities

- `consolidated-package`: 规范合并后 theway 包的 feature 矩阵 (default=headless, tui=TUI, server=axum/tonic transport)、bin 组装 (required-features tui+server) 与 workspace 成员集合。

### Modified Capabilities

- 无。

## Impact

- **Cargo**: 根 `Cargo.toml` members;`crates/app/Cargo.toml` (features/deps/bin/build-deps);`crates/server`、`crates/cli` 删除。
- **代码**: `crates/app/src/transport/**` 重建 (server 回迁);`crates/app/src/main.rs` + `tests/cli_help.rs` 回迁;`build.rs` 回迁;引用适配 (crate:: vs theway::)。
- **行为**: 不变 (同一进程, 同一协议面);`cargo build --workspace` 默认 headless (不编 bin),`--features tui,server` 出完整 CLI。
- **文档**: `docs/issues/00-master.md` 分层描述、Makefile、CI、AGENTS.md 结构描述同步。
- **不改变**: 包名 `theway` 公开 API;wire 模型;协议端点语义;`server → app` 依赖逻辑 (变成 feature 内模块边界)。
