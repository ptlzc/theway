# 实施任务

> 单一 `config.toml` 承载全部配置；theme/ui-state/mcp 各自带旧文件 fallback。
> 编排：节点按 DAG 声明依赖；文件不相交的节点并行（theme / ui-state / mcp 都在 theway-tui，但文件边界清晰）。

## 1-theme-config（theway-tui）

- [x] 1.1 `theme/core.rs`：抽 `parse_sections(&TomlTable)`（现有 section 遍历逻辑）；`parse(text)` 调它；新增 `parse_namespaced(text)`（提取 `theme` 子表后调 `parse_sections`）。`color.rs` 的 `theme_toml_path()` 保留（fallback 用）。
- [x] 1.2 `theme/core.rs` `load()`：读 `config.toml`（`config_payload::config_path(None)`），有 `[theme]` → `parse_namespaced`；否则 `load_from(&theme_toml_path())`（旧行为）。新增 `load_from_config(text)` 供测试与 reload 复用；`load_from(path)`（旧文件 fallback）语义不变。
- [x] 1.3 测试：`[theme]` 子表解析 == 顶层 section 解析；config.toml 无 `[theme]` → fallback theme.toml；config.toml 混入 `[model]`/`[ui]`/`[[server]]` 不影响 theme 解析。

## 2-ui-state-config（theway-tui）

- [x] 2.1 `ui_state.rs`：`load()` 读 `config.toml`（`config_payload::config_path(None)`），有 `[ui]` → 从 `ui.feed.thinking_mode` / `ui.panel` / `ui.graph` 解析；否则 `load_from(&state_path())`（旧 ui-state.toml）。`load_from(path)`（旧文件 fallback）语义不变。
- [x] 2.2 测试：`[ui.*]` 解析；无 `[ui]` fallback ui-state.toml；字段缺省；save 仍写旧 `ui-state.toml`（本次不改持久化目标，见任务 4）。
- [x] [depends: 1-theme-config]（共用 config_path 解析）

## 3-mcp-config（theway-tui）

- [x] 3.1 `mcp_scan.rs`：新增 `scan_mcp_servers_from_config(config_path) -> (Vec<…>, Vec<String>)`（读 config.toml 的 `[[server]]`，复用 `McpConfigToml`）；`scan_mcp_servers(user, project)` 改为「config.toml 有 `[[server]]` 直接用它，否则读 user+project 旧文件」。config.toml 读取用 `config_payload::config_path(None)`。
- [x] 3.2 `config_payload.rs`：`assemble_config_from` 里 mcp 扫描改走 config.toml 优先（`scan_mcp_servers_from_config` → fallback 旧路径）。
- [x] 3.3 测试：config.toml 有 `[[server]]` → 用 config（不读旧文件）；无 → 读 user+project 旧文件（保持 merge/覆盖/诊断）；config 解析失败进 diagnostics。
- [x] [depends: 1-theme-config]

## 4-save-location（theway-tui，可选收尾）

- [x] 4.1 决定 ui-state 持久化目标：保持写 `ui-state.toml`（最小改动）还是也迁到 `config.toml` 的 `[ui.*]`（真单一文件）。**默认保持写 `ui-state.toml`**——ui-state 是运行时高频自写状态，混进手写的 config.toml 会污染注释；本任务只读合并，写侧不动。config.toml 里的 `[ui.*]` 即「钉住」开关，会遮蔽运行时切换，直到用户移除该表。
- [x] 4.2 若无代码改动则勾选说明，不做实现。
- [x] [depends: 2-ui-state-config]

## 5-migrate-verify（迁移与验证）

- [x] 5.1 合并 `~/.theway/config.toml`：`[theme.*]`（theme.toml 内容命名空间化）、`[ui.*]`（ui-state.toml 内容）、`[[server]]`（mcp.toml 内容），保留 `[model]`。
- [x] 5.2 删除 `~/.theway/theme.toml`、`~/.theway/ui-state.toml`、`~/.theway/mcp.toml`；重启 TUI 验证：主题样式、面板位置、MCP 服务器（devops-mcp/crg 连上、devin-search 红行）与迁移前一致。
- [x] 5.3 `docs/architecture.md` 配置文件清单与 `config.toml` 结构同步；`openspec validate --strict`；workspace 测试 + clippy + fmt 全绿；归档 change。
- [x] [depends: 1-theme-config, 2-ui-state-config, 3-mcp-config]
