# Design: crate-restructure-web-isolation

## Context

workspace 当前四个成员: `crates/llm-provider`、`crates/core` (theway-core)、`crates/harness` (包名 `theway`)、`crates/mcp`。核心结构问题:

1. **命名歧义**: `theway-core` 内部有 `harness` 模块 (引擎的运行时层: AgentHarness/会话/压缩/技能/触发/图编排/子代理, ~12.5k 行),而 `crates/harness` 是包名 `theway` 的应用层 crate (CLI/tools/TUI/transport)。两者无任何从属关系,纯属历史撞名 (目录名来自 fork 遗留)。依赖方向为 `theway` → `theway-core` → `theway-llm-provider`。
2. **web 栈混杂**: gRPC/HTTP/WS 服务器 (`transport/`, ~2.3k 行) 通过 `impl App { run_web/run_grpc }` 直接驱动 `App` 的私有事件循环 (ui/mod.rs 3545 行, TUI 与 web 共用, 靠 `pub(crate)` 字段互捅)。`web_index.html` (2112 行单文件前端, 纯 `EventSource`/`fetch` 协议消费者) 用 `include_str!` 内嵌进服务器。
3. **无 health 端点**: 没有 `/healthz`, 没有 gRPC health service;`/` 的唯一作用是返回内嵌 HTML。

约束: 包名 `theway` 是公开 API (workmate 等外部项目 `use theway::...`),不可破坏;`AgentHarness` 是行业术语,保留;`--web`/`--grpc` 协议端点语义不变,现有客户端无需改动。

## Goals / Non-Goals

**Goals:**

- 消除 "harness" 撞名: 仓库中该术语仅作为 `AgentHarness` 类型名存在。
- 建立三层干净架构: 引擎 (core) / 宿主 (app) / 协议 (server),命令行入口独立 (cli)。
- 服务器纯协议化: 零 HTML、零前端资源,只有协议端点 + 标准 health。
- 服务器只通过公开 `AppHandle` 接口访问运行时,与 App 内部实现解耦。
- 每一步可独立验证、可提交,全程 `cargo test --workspace` 全绿。

**Non-Goals:**

- 不实现 session 资源模型 (ListSessions/CreateSession/SwitchSession 等) — 那是独立的后续变更,本变更只保证其落地时有干净的 crate 边界。
- 不拆 `theway-runtime` 独立 crate (把 `core::runtime` 提为 `crates/runtime`) — 列为后续阶段,理由见 Decision D1。
- 不做 per-session automations (triggers/cron 保持进程级) — 与 session 模型变更绑定,不混入本次。
- 不创建/迁移浏览器前端 (theway-console 独立仓库) — 仅删除内嵌页,前端另行立项。
- 不改动 `AgentHarness` 类型名、不改动 wire 协议端点语义、不改动包名 `theway`。

## Decisions

### Decision: core 模块 `harness` → `runtime`

`theway_core::harness` 改名为 `theway_core::runtime`。该模块装的就是 agent 的运行时 (会话持久化、压缩、技能加载、触发器、图编排、子代理注册表),`theway_core::runtime::AgentHarness` 语义通顺且零歧义。lib.rs re-export 与全部引用机械替换,`AgentHarness` 类型名保留。

替代方案: (a) 拆出 `crates/runtime` 独立 crate — 结构最纯,但 `runtime` 与裸 agent 共享 `types.rs`/`node.rs`/`proxy.rs`,拆 crate 被迫先拆共享类型,且 feature 重组 (`harness` feature 迁移)、依赖搬家 (tokio/chrono/serde_yaml/sha2…) 与全量引用替换一次做完,风险高;当前无 wasm 等"只要裸内核"的实际消费者,收益为结构纯度,不匹配本次变更的风险预算。列为后续独立阶段。(b) 保持模块名、靠文档说明 — 妥协,不采用。

### Decision: `crates/harness` 目录 → `crates/app`

目录改名 `crates/app`,包名 `theway` 不变。内容本来就是应用层 (tools/TUI/AgentSession/session 操作),叫 `app` 名副其实。改动仅: `git mv` + 根 `Cargo.toml` members + workspace 内 `path = "../harness"` 引用 (3 处)。公开 API `use theway::...` 不受影响。

替代方案: 保留目录名 + 文档澄清 — 已否决 (用户明确不接受"文档写清"式妥协);目录改名 `crates/agent` — 与 core 的 `agent` 模块概念重叠,`app` 更准。

### Decision: 新增 `theway-server` 与 `theway-cli` 两个 crate

- `crates/server` (theway-server): 纯协议层 — `transport/` (grpc/http/ws/proto/types) + `build.rs` (protox) + 服务器依赖 (axum/tonic/prost/tokio-tungstenite/tokio-stream) + `http/tests/`。只对 `AppHandle` 编程。
- `crates/cli` (theway-cli): `main.rs` ([[bin]]) + CLI 依赖 (clap 等)。组装 `theway` + `theway-server`,提供 `--tui`/`--web`/`--grpc` 三模式。

**为什么 bin 必须移出 theway 包**: cargo 的包级依赖环检测 — theway 包的 bin 若使用 theway-server,而 theway-server 依赖 theway 的 lib,则 theway 包 ↔ theway-server 包构成环,被拒绝。把 bin 放独立 crate 是标准做法 (lib/bin 分离)。

替代方案: 把 bin 塞进 theway-server 包 — 依赖无环但职责混乱 (协议包里长着 CLI);bin 留在 theway 包且 server 只依赖 core — 不可行,server 需要 App 层的能力 (kernel/feed/snapshot)。

### Decision: wire 模型留 `theway`,proto 编解码进 `theway-server`

`WebStatus`/`WebCommand` 定义在 theway (App::web_snapshot() 返回它们),`proto.rs` 转换、proto 构建与传输实现进 server。依赖方向 server → app → core 严格单向。

### Decision: 抽公开 `AppHandle` 接口

theway 暴露 `AppHandle` (snapshot / command / events 流 / dag 快照),server 只对 handle 编程。事件循环 (kernel 轮询) 留在 App 内部 — 它是"turn 语义"的归属,不是协议翻译。

替代方案: 继续靠 `pub(crate)` 字段跨 crate — 不可能 (pub(crate) 仅限 crate 内);定义 `Frontend` trait 让 App 反向依赖 — 过度设计,struct handle 已足够。

### Decision: 删除 `web_index.html`,`/` 404,加标准 health

- 删 `index` 路由 + `web_index.html` → `/` 404,服务器零 HTML。
- HTTP `GET /healthz` → 固定短文本 200,不碰业务状态 (liveness 语义)。
- gRPC 实现标准 `grpc.health.v1.Health` (Check/Watch)。
- CLI 启动日志补一行: 监听地址 + 端点清单 + UI 指引 (独立前端)。

**为什么不要"极简占位页"**: health 是机器接口 (K8s probe / 监控),"人打开浏览器看到什么"由 CLI 启动日志解决 (它已经打印 listening 行,补端点/UI 说明即可)。占位页本质是"内嵌 UI"原则没走到底的残留;`/` 404 是最诚实的"这里只有协议"。

### Decision: relay 留在 theway

`ui/relay.rs` (web-connect 远程查看器) 与 App 的通道字段 (relay_prompt_rx/abort_rx/resolve_rx…) 耦合深,且 TUI/web/headless 三模式共用。不进 server crate;若 relay 后续产品化 (独立查看器前端),再评估迁移。

## Risks / Trade-offs

- [全量引用替换遗漏 (runtime 改名)] → 机械替换 + `grep -rn "theway_core::harness"` 应为空 + `cargo build/test/clippy/fmt` 全绿作为验收。
- [proto 构建迁移 (build.rs 换 crate) 致 include_proto! 路径错] → P1.1 独立一步单独验证构建;proto 文件本身不移动。
- [AppHandle 抽取改变 SSE/WS 时序行为] → P1.2 保持 run_web/run_grpc 语义不变,现有 transport 测试 (endpoints/grpc/ws) 作回归网。
- [cargo 包级依赖环漏判] → 依赖方向硬约束: cli → server → app → core,lib 层无环,`cargo metadata` 校验。
- [目录改名破坏外部引用 (crates/harness 路径)] → workspace 内 `path` 引用全部更新;包名 `theway` 不变,外部 Cargo 依赖按包名引用,不受影响。
- [一次搬移过多导致 diff 不可审] → 严格分步 (P1.0-P1.4),每步独立提交,`git mv` 保留历史。

## Migration Plan

1. **P1.0 改名** (零行为变化): `core::harness` → `core::runtime` (lib.rs + 引用替换,测试全绿);`crates/harness` → `crates/app` (git mv + path 引用)。验收: `cargo test --workspace` 全绿,`grep harness` 只剩 AgentHarness。
2. **P1.1 建 theway-server**: 迁移 transport/ + build.rs + 服务器依赖;theway 移除 axum/tonic 依赖。验收: 构建全绿,`--grpc`/`--web` 可编译。
3. **P1.2 抽 AppHandle**: theway 暴露公开接口,server 改对 handle 编程。验收: 现有 transport 测试全过。
4. **P1.3 建 theway-cli**: main.rs 移入,依赖 theway + theway-server。验收: `--tui`/`--web`/`--grpc` 三模式冒烟。
5. **P1.4 删 UI + 加 health**: 删 web_index.html + index 路由;`/healthz` + gRPC health;CLI 引导日志。验收: `curl /healthz` 200,`curl /` 404,health 单测。

回滚策略: 每步独立 commit,任一步失败可 revert 该 commit;P1.0-P1.3 无行为变化,回滚零成本。

## Open Questions

- `web_index.html` 是直接删除还是先归档到 `web/` 目录 (git 历史已保留全部内容)?倾向直接删除,前端另行立项。
- `crates/harness` 改 `crates/app` 后,`docs/issues/00-master.md` 中的分层描述 (theway-core ↔ theway) 是否需要同步更新引用路径 — 是,纳入 P1.0。
