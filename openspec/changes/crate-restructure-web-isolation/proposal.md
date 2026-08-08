## Why

workspace 存在两层结构问题:(1) `theway-core` 内部的 `harness` 模块与 `crates/harness` crate 撞名,两个"harness"语义完全不同 (引擎运行时层 vs 应用层),目录结构无法反映真实分层;(2) web 栈混杂在应用 crate 里 — gRPC/HTTP/WS 服务器靠 `pub(crate)` 字段直接捅 `App` 的私有事件循环,2112 行的内嵌前端 `web_index.html` 把"协议消费者"绑死在"协议服务器"中,且服务器没有任何 health 端点。结构问题不解决,后续 session 资源模型等协议演进都将在混乱的边界上叠加。

## What Changes

- **BREAKING (路径)**: `theway_core::harness` 模块改名为 `theway_core::runtime`(模块 = 引擎的运行时层: AgentHarness/会话/压缩/技能/触发/图编排/子代理)。`AgentHarness` 类型名保留 (行业术语)。
- **BREAKING (路径)**: `crates/harness/` 目录改名为 `crates/app/`(包名 `theway` 不变,公开 API 不破坏;仅 workspace 内 `path` 引用更新)。
- 新增 crate `crates/server` (`theway-server`): 纯协议层 — 从 `theway` 迁出 `transport/` (gRPC/HTTP/WS + proto 编解码)、`build.rs` (protox) 与服务器依赖 (axum/tonic/prost/tokio-tungstenite/tokio-stream)。
- 新增 crate `crates/cli` (`theway-cli`): `main.rs` ([[bin]]) 移入,组装 `theway` + `theway-server`(cargo 包级依赖环的硬约束)。
- 抽取公开 `AppHandle` 接口: `theway` 暴露 snapshot/command/events 公开 API,`theway-server` 只对 `AppHandle` 编程,不再访问 App 私有字段。
- wire 模型 (`WebStatus`/`WebCommand`) 留在 `theway`(依赖方向: server → app → core);proto 文件与编解码进 `theway-server`。
- 删除内嵌前端 `web_index.html` 与 `/` index 路由 → `/` 返回 404,服务器零 HTML。
- 新增 health 端点: HTTP `/healthz`(轻量 liveness)+ gRPC 标准 `grpc.health.v1.Health` service。
- CLI 启动日志补充端点清单与 UI 引导行 (`listening on http://… · endpoints: … · UI: workmate (独立)`)。

## Capabilities

### New Capabilities

- `crate-layout`: 规范 workspace 的 canonical crate 布局 (llm-provider/core/app/mcp/server/cli)、core 内模块分层 (agent/agent_loop/runtime)、`harness` 术语边界 (仅 `AgentHarness` 类型名) 与单向依赖约束。
- `protocol-server-health`: 规范 `theway-server` 的健康检查行为 (HTTP `/healthz` + gRPC health)、`/` 不提供 UI (404) 与"服务器零 HTML/前端资源"约束。

### Modified Capabilities

- 无 (openspec/specs/ 尚无既有 spec)。

## Impact

- **Cargo**: 根 `Cargo.toml` workspace members 增加 `crates/server`、`crates/cli`,`crates/harness` → `crates/app`;相关 crate 的 `path = "../harness"` 依赖声明更新。
- **Rust 路径**: 所有 `theway_core::harness::` 引用 (lib.rs re-export、core 内部、app 层 use) 替换为 `theway_core::runtime::`。
- **代码移动**: `crates/harness/src/transport/{grpc,http,ws,proto,types}.rs` + `http/tests/` → `crates/server`;`crates/harness/build.rs` → `crates/server`;`crates/harness/src/main.rs` → `crates/cli`;`crates/harness/src/ui/web_index.html` 删除。
- **行为变化**: `/` 从返回内嵌 UI 变为 404;新增 `/healthz` 与 gRPC health;启动日志新增端点/UI 引导。
- **测试**: transport 现有测试 (http/tests/endpoints.rs、grpc.rs 内嵌测试、ws.rs) 随代码迁移并保持全绿;新增 health 端点测试。
- **文档**: `docs/issues/00-master.md` 等引用 "theway-core ↔ theway" 分层描述的文档同步更新。
- **不改变**: 包名 `theway` 的公开 API;`AgentHarness` 类型名;gRPC/HTTP/WS 协议端点语义 (`--web`/`--grpc` 客户端无需改动)。
