# Design: provision-mcp-servers

## Context

现状链路（2026-09-06）：

- **TUI spawn daemon（controller 模式）**：`startup.rs` 里 `storage_service_addr.is_some() → load_local_sources = false` → `mcp = LoadedMcp::empty()` → `SessionMcpResources::default()`（零工具零钩子）。skills/templates 已由 #95/#96 走「TUI 扫描 → `WireDaemonConfig` → `Configure` 应用 + provision slot」路径，MCP 是 seam 里最后一块没供给通道的资源。
- **加载器**：`mcp_loader::load_all(paths)` 读 user `paths.base/mcp.toml` + project `paths.work_dir/.theway/mcp.toml`（同名 project 覆盖），`connect_all(configs, cwd, auth_path)` 逐服务器连接（stdio spawn / streamable_http + `auth.json` 的 `token_keychain_ref` 解析），产出 tools + `McpNotificationHook`（label=`mcp:<name>`，one-shot receiver）+ diagnostics（`mcp server '<name>' failed: …` / `mcp config (<label>, …): …`）。`connect_all` 与 `connect_one` 目前是私有 fn。
- **session build**：`tools.extend(ctx.mcp.tools)` 进 harness；`register_notification_hooks(&trigger_executor, &ctx.mcp.notification_hooks, …)` 注册进每 session 的 TriggerExecutor（`register` 接口在 `NotificationHookSink` trait，startup.rs / session.rs 各有一份私有实现，label 唯一性契约）。`ctx.mcp` 是 startup 一次性快照，Configure 改不动它。
- **Configure 应用**（`turn/daemon/commands.rs::handle_configure`）：串行事件循环里按字段分支应用到 **live session** 的 harness（`self.session.kernel.harness()`）+ 写 provision slot（`Arc<RwLock>`，挂在 `DaemonConfig` 上，`/reload` 与 session build 读它）。
- **错误可见性**：`RuntimeCapabilities.mcp_server_errors` → `WireMcpSnapshot.errors` → TUI 3s banner + 面板红行（df17d55 已落地，本 change 只填充数据）。

## Goals / Non-Goals

**Goals:**

- controller 模式下 MCP 服务器由 TUI 供给、daemon 连接，工具与推送钩子进入 live session 与新 session。
- 供给集合替换/清除语义完整；失败进入快照 `errors`。
- gRPC 与 JSON-RPC 供给行为一致（proto twin 完整）。

**Non-Goals:**

- 不做单服务器热插拔开关（enable/disable 单个）；供给 = 整体替换。
- 不把 `ServerConfig` 下沉共享 crate（见决策 1）。
- 不改 `load_local_sources` seam 本身；独立 thewayd 行为不动。
- live session 里**移除**服务器的钩子不反注册（见风险 1）。

## Decisions

### 1. TUI 侧 deserialize-only 镜像结构，不建共享 crate

`ServerConfig` 在 `theway-daemon/src/mcp_loader.rs`，theway-tui 不依赖 theway-daemon。选项：下沉 `theway-contract`（该 crate 是依赖无关的 API 类型，塞 toml loader 形状违背 layering）vs TUI 自写镜像。**决策**：TUI 新增 `mcp_scan.rs`，只声明 `Deserialize` 的镜像结构（字段与 `mcp.toml` `[[server]]` 表一一对应，未知字段忽略——与 daemon loader 的 serde 行为一致），扫描后映射为 wire `WireProvisionedMcpServer`。合并规则照抄 loader：user 先入，project 同名覆盖。风险点：两个 serde 结构可能漂移——用共享 fixture toml 的对照测试兜底（`tests/mcp_scan/` 断言字段映射与 loader 的 `load_all` 结果一致）。

### 2. 凭据只有引用名过 wire

`WireProvisionedMcpServer.auth` 只带 `kind` + `token_keychain_ref`；token 由 daemon 在 `connect_servers(configs, cwd, paths.base/auth.json)` 时解析（与 `load_all` 完全同一条路径）。`GetConfig` 回显同理不回 token。好处：settings RPC 与配置视图零秘密；坏处：auth.json 的 token 失效时错误只出现在 daemon 侧（这正是 `McpSnapshot.errors` 要呈现的）。

### 3. daemon 复用 `connect_all`，不复制连接逻辑

`mcp_loader` 把 `connect_all` 提为 `pub(crate) fn connect_servers(configs, cwd, auth_path) -> (tools, hooks, diagnostics, client_count, server_names)`（连接失败诊断格式不变 → `parse_mcp_diagnostic` 直接产出 `errors`）。Configure 分支里顺序 await（与 startup 的 `load_all` 时序一致），单服务器超时由各自的 timeout 配置兜住。

### 4. `McpProvisionSlot`：供给状态的唯一事实源

`DaemonConfig` 加 `mcp_provision: Arc<RwLock<McpProvisionState>>`，其中：

```
configs: Vec<ServerConfig>          // /reload 重连用
tools: Vec<Arc<dyn AgentTool>>      // 当前连接的 MCP 工具
hooks: Vec<Arc<McpNotificationHook>>
inject_summary / inject_and_run: HashSet<String>
server_names / tool_names / errors: … // 直接映射 RuntimeCapabilities
registered_labels: HashSet<String>    // 已注册进 live executor 的 mcp:<name>
```

三处消费：① `handle_configure` 写并应用到 live harness；② `SessionMcpResources` 持 slot 句柄，controller 模式下 session build 从 slot 读 tools/hooks/inject 集合（替代 startup 空快照；`ctx.mcp` 保持原样供独立模式）；③ `reload_everything` 用 `configs` 重连。

### 5. live harness 工具替换：core 加 `replace_mcp_tools`

`AgentHarness` 目前没有工具替换 API（build 时 `agent.state().tools` 一次性赋值）。**决策**：core `catalog.rs` 旁新增 `replace_mcp_tools(&self, old: &[Arc<dyn AgentTool>], new: Vec<Arc<dyn AgentTool>>)`——用 `Arc::ptr_eq` 从 `agent.state().tools` 移除 old 实例再 append new（old 就是 slot 里上一次的实例，同一 Arc 必命中；不按名字匹配，避免同名工具误删非 MCP 工具）。备选：按 tool name 增删（名字可撞，被否）；重建 harness（丢运行态，被否）。移除即 drop → `McpClient` drop → stdio 子进程回收。

### 6. live session 钩子：注册增量，不反注册

`register_notification_hooks` 改成 `pub(crate)`（startup.rs 与 session.rs 两份合并成一份，`NotificationHookSink` 泛型不变）。Configure 成功后对 live 会话的 TriggerExecutor 注册 `new_labels = mcp:<name> 不在 registered_labels` 的钩子并记录 label。`McpNotificationHook` 是 one-shot receiver，新 hook 从未 run，注册进已存活的 executor 是安全的。移除服务器的钩子**不**从 live executor 反注册（TriggerExecutor 无 remove API）；其 receiver 随客户端 drop 关闭，hook 不再产出事件——功能等价，只残留一个死注册。

### 7. Configure 分支语义与 clear

`handle_configure` 新增分支（放 skills/templates 之后）：`!config.mcp_servers.is_empty() || config.clears("mcp_servers")` 时生效。校验：name 非空且唯一（违反 → error_line，整体不应用）。应用顺序：转换 ServerConfig → 校验 → `connect_servers` → 写 slot（含 diagnostics→errors）→ `replace_mcp_tools` → 更新 `RuntimeCapabilities`（servers/tools/names/errors）→ `applied.mcp_servers` 或 `clear_fields` 回显。空列表 + 无 clear = 不动作（与 skills/templates 一致）。清除时 slot 全清、工具全摘、capabilities 归零。快照在应用后自然携带新 errors → TUI banner/面板红行无需新代码。

### 8. proto twin

`settings.proto`：`DaemonConfig` 加 `repeated ProvisionedMcpServer mcp_servers = 15`；`ProvisionedMcpServer`（name/kind/command/args/endpoint + `ProvisionedMcpAuth{kind,token_keychain_ref}` + 三个超时/上限 + `ProvisionedMcpReconnect{initial_ms,max_ms,max_attempts}` + inject 两标志）。wire `WireDaemonConfig.FIELDS` 加 `"mcp_servers"`（unknown_clear_fields 校验随之生效）；`WireProvisionedMcpServer` 放 `wire/commands.rs` 现有供给类型旁。转换与 round-trip 测试放 `proto/resources.rs` 既有位置。TS SDK 由 pre-commit sdk-sync 自动镜像。

### 9. TUI reconcile

`config_payload::reconcile`：`desired.mcp_servers != current.mcp_servers → patch.mcp_servers`；`desired` 空列表且不 clear → 不推（daemon 无「清空」语义，用 clear_fields 表达）。TUI 扫描结果始终是合并后的完整列表（权威整体替换）。扫描解析失败 → `notes`（fresh spawn 时 notes 不打印——连接失败会经 daemon 快照呈现；纯解析失败在 fresh spawn 下的呈现留待后续 provision 回执机制，见风险 3）。

### 10. 测试策略

- wire/proto：round-trip 含全部字段（含空 optional 的默认表达）。
- `mcp_scan`：merge（project 覆盖）、字段映射对照 loader fixture、解析失败进 notes。
- `reconcile`：列表差量、clear 语义、空列表不推。
- daemon Configure：复用 `tests/mcp_server/` 的 fixture 服务器（stdio/streamable_http）走完整 `handle_configure`——应用后 harness 工具出现/消失、capabilities 与快照 errors 更新、clear 清空、重复应用幂等（同配置不重连——由 reconcile 层保证，daemon 侧测试直接验证两次应用结果一致）。
- session build：controller 模式下新 session 从 slot 拿工具与钩子。
- reload：改 configs 后 `/reload` 重连生效。
- E2E：tmux TUI + `~/.theway/mcp.toml`（devops-mcp:10443 已实测通）→ 面板出现 MCP 区块与工具数；改坏 endpoint → 3s banner + 红行。

## Risks / Trade-offs

1. **live executor 死注册**：移除服务器的钩子不反注册，TriggerExecutor 内残留 label。影响面小（receiver 关闭后不产事件），后续若加 remove-hook API 再清理。
2. **Configure 阻塞**：连接在 daemon 事件循环里顺序 await，慢服务器延迟设置应用与快照。与 startup `load_all` 行为一致；后续可改为连接后台化 + 完成后补快照。
3. **TUI 解析失败在 fresh spawn 无通道**：notes 只在 attach 打印。连接类错误走快照可见，纯 toml 解析错误暂只在 daemon 日志/tracing。后续 provision 回执（ack 带 notes）可补齐。
4. **双 serde 结构漂移**：TUI 镜像与 daemon `ServerConfig` 各自演化。对照测试 + 字段缺失时 serde 默认值兜底；新增字段需同步两处（wire 消息是中间层，漂移会最先在对照测试暴露）。
5. **stdin/stdio 子进程生命周期**：工具替换 drop 客户端即回收子进程；`replace_mcp_tools` 按 Arc 实例移除，若未来有路径重建工具实例（同名重连），ptr_eq 会漏删——用「slot 只存当前实例」不变量约束，重连先摘旧再挂新。
