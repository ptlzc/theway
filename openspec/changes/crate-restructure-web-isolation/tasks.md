# Tasks: crate-restructure-web-isolation

## 1. 改名 (P1.0, 零行为变化)

- [ ] 1.1 `theway-core` 模块改名: `crates/core/src/harness/` → `crates/core/src/runtime/`,更新 `lib.rs` 的 `pub mod harness` → `pub mod runtime` 与全部 re-export。
- [ ] 1.2 替换所有 `theway_core::harness::` 引用为 `theway_core::runtime::`(core 内部 + app 层 + 测试/示例),`grep -rn "theway_core::harness"` 结果为空。
- [ ] 1.3 `git mv crates/harness crates/app`,更新根 `Cargo.toml` members 与 workspace 内 `path = "../harness"` 依赖声明 (3 处)。
- [ ] 1.4 更新 `docs/issues/00-master.md` 等文档中的分层描述与路径引用。
- [ ] 1.5 验收: `cargo test --workspace` 全绿;`cargo clippy --workspace --all-targets -- -D warnings` 通过;`cargo fmt --all --check` 通过;`grep harness` 仅剩 `AgentHarness` 类型。

## 2. 建 theway-server (P1.1)

- [ ] 2.1 创建 `crates/server` crate (`theway-server`),Cargo.toml 引入服务器依赖 (axum/tonic/tonic-prost/prost/tokio-tungstenite/tokio-stream) 与 `theway`、`theway-core` path 依赖。
- [ ] 2.2 迁移 `transport/` 模块: `grpc.rs` `http.rs` `http/tests/` `ws.rs` `proto.rs` `types.rs` `mod.rs` 从 `crates/app/src/transport/` 移入 server crate。
- [ ] 2.3 迁移 proto 构建: `crates/app/build.rs` (protox) → server crate,`include_proto!`/proto 路径适配,根 `proto/` 文件不移动。
- [ ] 2.4 从 `crates/app` 移除服务器依赖 (axum/tonic/prost/tokio-tungstenite/tokio-stream) 与 `build.rs`。
- [ ] 2.5 验收: `cargo build --workspace` 全绿;`cargo test -p theway-server` 通过 (transport 现有测试随迁)。

## 3. 抽 AppHandle 接口 (P1.2)

- [ ] 3.1 在 `crates/app` 定义公开 `AppHandle`: `snapshot() -> WebStatus`、`command(WebCommand) -> Result`、`events() -> broadcast::Receiver<WebStatus>`、dag/subagent 快照访问;`App` 提供构造入口。
- [ ] 3.2 重构 `run_web`/`run_grpc`: 事件循环 (kernel 轮询) 留在 App 内部;HTTP/gRPC/WS handler 改为只对 `AppHandle` 编程,不再访问 App 私有/`pub(crate)` 字段。
- [ ] 3.3 调整可见性: 原 `pub(crate)` 字段中不再被 transport 触碰的收回私有,AppHandle 需要的提升为公开只读方法。
- [ ] 3.4 验收: transport 测试 (http endpoints / grpc / ws) 全过;`--web`/`--grpc` 冒烟行为与重构前一致。

## 4. 建 theway-cli (P1.4)

- [ ] 4.1 创建 `crates/cli` crate (`theway-cli`),含 `[[bin]]`,依赖 `theway` + `theway-server` + clap 等 CLI 依赖。
- [ ] 4.2 迁移 `crates/app/src/main.rs` → `crates/cli/src/main.rs`,组装逻辑 (AppConfig 构建、`--tui`/`--web`/`--grpc` 分发) 适配新 crate 边界。
- [ ] 4.3 从 `crates/app` 移除 `[[bin]]` 与 bin-only 依赖 (clap 等),theway 变纯库。
- [ ] 4.4 验收: `cargo run -p theway-cli -- --web` / `--grpc` / TUI 三模式冒烟;`cargo test --workspace` 全绿。

## 5. 删 UI + 加 health (P1.4)

- [ ] 5.1 删除 `web_index.html` 与 `/` index 路由,`GET /` 返回 404;确认 server crate 无任何 HTML/JS/CSS 前端资源。
- [ ] 5.2 HTTP 新增 `GET /healthz`: 固定短文本 200,不依赖业务状态。
- [ ] 5.3 gRPC 实现标准 `grpc.health.v1.Health` (Check/Watch,健康返回 SERVING),与 ThewayGrpc 同端口共存。
- [ ] 5.4 CLI 启动日志补引导行: 监听地址 + 端点清单 (`/state` `/events` `/ws` `/healthz`) + UI 指引 (独立前端)。
- [ ] 5.5 新增测试: `/healthz` 200 且不携带业务负载;`/` 404;gRPC health Check 返回 SERVING。
- [ ] 5.6 最终验收: `cargo test --workspace` + clippy + fmt 全绿;`curl /healthz` 200;`curl /` 404;`curl /state` 正常;gRPC health 探测 SERVING。
