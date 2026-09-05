# Proposal: provision-mcp-servers

## Why

#95/#96 把 skills 与 templates 的供给改为 controller 全责（TUI 扫描 → `WireDaemonConfig` → daemon `Configure` 应用 + provision slot），controller 模式下 daemon 零文件 IO。但 MCP 服务器还留在 `load_local_sources` seam 里：TUI spawn 的 daemon 带 `--storage-service-addr` → `load_local_sources = false` → `mcp_loader::load_all` 被整体跳过 → 默认 TUI 流程中 `~/.theway/mcp.toml` 与项目 `mcp.toml` 完全没有加载方（issue #73 的 TODO 就写在此处）。后果：devops-mcp 等服务器在默认流程里连工具列表都不可见，连接失败也只有 daemon 日志、用户零提示。

本 change 把 MCP 服务器纳入 controller 供给面：TUI 扫描两个 mcp.toml → settings RPC 供给 → daemon 连接并把工具/通知钩子/诊断注入运行时与快照（错误可见性复用刚落地的 `McpSnapshot.errors` 机制）。

## What Changes

- **wire 契约**：`WireDaemonConfig` 新增 `mcp_servers: Vec<WireProvisionedMcpServer>`（含全部连接参数，见下），FIELDS/merge_from/unknown_clear_fields 同步；`WireProvisionedMcpServer` 形状对应 daemon `mcp_loader::ServerConfig`（name/kind/command/args/endpoint/auth{kind,token_keychain_ref}/request_timeout_ms/sse_idle_timeout_ms/body_cap_bytes/reconnect{initial_ms,max_ms,max_attempts}/inject_summary/inject_and_run）。**凭据不出 wire**：`token_keychain_ref` 只传引用名，token 由 daemon 从 `paths.base/auth.json` 解析（与现状一致）。
- **proto twin**：`settings.proto` `DaemonConfig` 增加 `repeated ProvisionedMcpServer mcp_servers = 15`（message 与 wire 一一对应），`resources.rs` 双向转换 + round-trip 测试，TS SDK 重新生成。
- **TUI 扫描**：新增 `mcp_scan` 模块——解析 `~/.theway/mcp.toml`（user）与 `<cwd>/.theway/mcp.toml`（project，同名覆盖 user，镜像 daemon `mcp_loader::load_all` 的合并规则）；deserialize-only 镜像结构（daemon 的 `ServerConfig` 不跨 crate）；解析失败进 provision notes 而非静默。`config_payload` 组装 + reconcile 差量（空列表 ≠ clear 语义，与 skills/templates 相同）。
- **daemon Configure 应用**：`handle_configure` 新增 `mcp_servers` 分支——wire→`ServerConfig` 转换（name 唯一性校验）→ 通过 `mcp_loader` 公开的连接路径 `connect_servers(configs, cwd, auth_path)` 连接（stdio spawn / streamable_http + auth.json）→ 结果写入新 `McpProvisionSlot`（configs/tools/hooks/inject 集合/server_names/tool_names/errors）→ 替换 live harness 的 MCP 工具（base ∪ new-mcp，旧服务器工具随客户端 drop 一并回收）→ 更新 `RuntimeCapabilities`（servers/tools/server_names/tool_names/errors）→ 下一次快照即触发 3s banner + 面板红色 `[x]` 行；`clear_fields: ["mcp_servers"]` 断开全部服务器并清空 slot。
- **session 构建走 slot**：controller 模式下 `SessionMcpResources` 持 `McpProvisionSlot` 句柄，session build 从 slot 读 tools/hooks/inject 集合（替代 startup 时的空 `LoadedMcp`），新 session 直接获得供给的 MCP 工具与推送钩子。
- **live session 钩子注册**：Configure 成功后把新连接的 MCP notification hooks 注册进**当前会话**的 trigger executor（抽取 startup 的 `register_notification_hooks` 为可复用 helper，不重复注册同名 hook）。
- **`/reload` 重连**：controller 模式下 `reload_everything` 额外用 slot 里的 configs 重连 MCP（幂等：连接替换 + 快照刷新），独立 thewayd 的 reload 保持扫盘语义。

## Capabilities

### Modified Capabilities

- `session-resource-model`: 增加「controller-provisioned MCP 服务器」需求——MCP 工具/hooks/inject 集合的来源在 controller 模式下改为供给 slot、连接替换与清除语义、错误进入会话快照的 `McpSnapshot.errors`。
- `external-protocol-service`: 增加「设置契约的 MCP 供给字段跨协议一致」需求——settings twin 携带 `mcp_servers`，gRPC 与 JSON-RPC 行为一致。

### New Capabilities

- 无。

## Impact

- `crates/theway-transport`: wire `WireProvisionedMcpServer` + `WireDaemonConfig.mcp_servers`；`settings.proto` message + `resources.rs` 转换；serde/proto 两侧测试；TS SDK 重新生成。
- `crates/theway-tui`: 新增 `mcp_scan`；`config_payload` 组装/reconcile；测试。
- `crates/theway-daemon`: `mcp_loader` 公开连接入口；`McpProvisionSlot` + `DaemonConfig` 注入；`handle_configure` 分支；`SessionMcpResources` slot 句柄；session build 与 live hook 注册；`reload_everything` 重连；capabilities 快照更新；测试。
- `docs/architecture.md`：controller 供给边界补充 MCP。

## Non-Goals

- 不改 `config.toml` schema；LSP/hook/ts_extensions 的供给（仍走 `load_local_sources` seam）不在本 change。
- 独立 `thewayd`（无 controller）保持本地扫盘行为，不受影响。
- 不传凭据内容过 wire；`token_keychain_ref` 语义不变。
- 不实现 MCP 服务器的热插拔管理命令（enable/disable 单个服务器）；供给即整体替换。
- 参考 GitHub issues：#73（本 change 主体）、#95/#96（供给范式）。
