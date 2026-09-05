# 实施任务

> 对应 GitHub issue：#73。每项完成一个 commit（Conventional Commits + issue 引用）。
> 编排：节点按 DAG 声明依赖；文件不相交的节点并行（theway-tui / theway-transport / theway-daemon 各归各的 crate）。
> agent 映射：executor = executor-coder，verify = checker。

## 1-wire-mcp（theway-transport）

- [x] 1.1 `wire/commands.rs`：新增 `WireProvisionedMcpServer`（name/kind/command/args/endpoint/auth{kind,token_keychain_ref}/request_timeout_ms/sse_idle_timeout_ms/body_cap_bytes/reconnect{initial_ms,max_ms,max_attempts}/inject_summary/inject_and_run）与 `WireProvisionedMcpAuth` / `WireProvisionedMcpReconnect`；`WireDaemonConfig.mcp_servers` 字段；`FIELDS` 加 `"mcp_servers"`（unknown_clear_fields 随之生效）；`merge_from` / clear 分支同步。
- [x] 1.2 `settings.proto`：`ProvisionedMcpServer` / `ProvisionedMcpAuth` / `ProvisionedMcpReconnect` message，`DaemonConfig.mcp_servers = 15`；`proto/resources.rs` 双向转换；TS SDK 经 sdk-sync 镜像（pre-commit 自动）。
- [x] 1.3 测试：wire merge/clear 含 mcp_servers；proto round-trip 含全字段与空 optional 默认表达。

## 2-mcp-scan（theway-tui）

- [x] 2.1 新增 `crates/theway-tui/src/mcp_scan.rs`：deserialize-only 镜像结构（字段与 daemon `mcp_loader::ServerConfig` 一致）；扫描 user `~/.theway/mcp.toml` + project `<cwd>/.theway/mcp.toml`，同名 project 覆盖 user（镜像 loader 合并规则）；解析失败产出 `(file_label, error)` 进 notes；结果映射为 `WireProvisionedMcpServer`。
- [x] 2.2 `config_payload.rs`：`assemble_config_from` 填 `payload.mcp_servers`；`reconcile` 增加列表差量（相等跳过、变化整表推送；空列表不推，清除用 clear_fields）。
- [x] 2.3 测试：合并覆盖、字段映射对照 loader fixture（与 daemon 测试共享一份 toml fixture 文本）、解析失败进 notes、reconcile 差量/相等/clear。
- [x] [depends: 1-wire-mcp]

## 3-daemon-provision（theway-daemon）

- [x] 3.1 `mcp_loader.rs`：`connect_all` 提为 `pub(crate) fn connect_servers(configs, cwd, auth_path) -> (tools, hooks, diagnostics, client_count, server_names)`；`ServerConfig` 增加 name 唯一性校验 helper。
- [x] 3.2 新增 `McpProvisionState`（configs/tools/hooks/inject 两集合/server_names/tool_names/errors/registered_labels）+ `DaemonConfig.mcp_provision: Arc<RwLock<McpProvisionState>>`；startup 构造空 slot 并注入。
- [x] 3.3 core `AgentHarness::replace_mcp_tools(&self, old, new)`（`Arc::ptr_eq` 移除 old 实例 + append new，见 design 决策 5）+ core 测试。
- [x] 3.4 `turn/daemon/commands.rs` `handle_configure` 新增 mcp_servers 分支：wire→ServerConfig → 校验（name 非空唯一）→ `connect_servers`（`paths.base/auth.json`）→ 写 slot → `replace_mcp_tools` → 更新 `RuntimeCapabilities`（servers/tools/names/errors）→ applied 回显；`clear_fields: ["mcp_servers"]` 断开全部并归零；配置视图（GetConfig）回显 mcp_servers。
- [x] 3.5 `register_notification_hooks` 提为 `pub(crate)` 共用 helper（合并 startup.rs / session.rs 两份）；Configure 成功后把 `registered_labels` 之外的新钩子注册进 live 会话 TriggerExecutor。
- [x] 3.6 测试：fixture 服务器走完整 Configure——工具进 harness/重复应用幂等/改列表摘旧挂新/clear 清空/capabilities 与快照 errors 更新/name 重复拒绝；clear 校验未知字段仍拒绝。
- [x] [depends: 1-wire-mcp]

## 4-session-slot-reload（theway-daemon）

- [x] 4.1 `SessionMcpResources` 持 slot 句柄（`Option<Arc<RwLock<McpProvisionState>>>`）；session build 在 controller 模式（slot 存在）从 slot 读 tools/hooks/inject 集合，替代 startup 空快照；独立模式路径不变。
- [x] 4.2 `reload_everything`：slot 存在时用 `configs` 重连 MCP（先摘旧工具/钩子，再 `connect_servers`，更新 slot + capabilities）；独立模式保持扫盘。
- [x] 4.3 测试：controller 模式新 session 拿 slot 工具与钩子；reload 重连生效；standalone 回归（本地 mcp.toml 照常加载）。
- [x] [depends: 3-daemon-provision]

## 5-e2e-docs-closeout（验证与文档）

- [x] 5.1 E2E（tmux PTY）：`~/.theway/mcp.toml` 配 devops-mcp(:10443) → 面板 MCP 区块出现 servers/tools 计数与名称；改坏 endpoint → 3s banner + 面板红色 `[x] devops-mcp`；修回 + `/reload` → 恢复。
- [x] 5.2 `docs/architecture.md` 同步 controller 供给边界（skills/templates/mcp 三目录均由 controller 扫描供给；MCP 凭据只传引用名）。
- [x] 5.3 `openspec validate --strict`；workspace 相关 crate 测试 + clippy + fmt 全绿；按 design.md 验收标准逐条核对；归档 change。
- [x] [depends: 2-mcp-scan, 4-session-slot-reload]
