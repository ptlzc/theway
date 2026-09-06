# Design: unify-config-toml

## Context

`~/.theway/` 的 4 个 toml 文件由不同组件读：

- `config.toml` — TUI `config_payload::assemble_config`（读全文，解析 `[model]` 等 daemon 运行时设置）。
- `theme.toml` — TUI `theme::Theme::load()`（`theme_toml_path()` = `base_dir()/theme.toml`；`parse(text)` 遍历 11 个顶层 section，unknown section 会 `warn`）。
- `mcp.toml` + 项目 `mcp.toml` — TUI `mcp_scan::scan_mcp_servers(user, project)`（deserialize-only `McpConfigToml { server: Vec<…> }`）；独立 thewayd 另有 `mcp_loader::load_all` 读同一批文件。
- `ui-state.toml` — TUI `ui_state::load_from`（读 `[feed].thinking_mode` / `[panel]` / `[graph]`）。

三者（theme/ui-state/mcp_scan）都在 theway-tui，读取时机各不相同：theme 在 `apply_snapshot` 的 reload 重读，ui-state 在启动读，mcp 在 `assemble_config` 读。

## Goals / Non-Goals

**Goals:** 单一 `config.toml` 承载全部配置；旧文件作为 fallback 继续工作（可渐进删除）。

**Non-Goals:** 不动独立 thewayd 的 mcp.toml 扫描；不改 daemon 运行时 section 语义。

## Decisions

### 1. `config.toml` 命名空间化 section（无冲突）

theme 的 11 个 section 原先是顶层（`[screen]`/`[feed]`/…），ui-state 用 `[feed]/[panel]/[graph]`。直接平铺会让 theme 的 `[feed]` 与 ui-state 的 `[feed]` 混同、并让 theme parser 对 `[model]`/`[panel]`/`[graph]`/`[[server]]` 产生 unknown-section 噪音。**决策**：主题迁到 `[theme.<section>]`、ui-state 迁到 `[ui.<section>]`、mcp 保持顶层 `[[server]]`（数组与 table 天然不冲突，且与 `mcp.toml` 同形便于迁移）。

```toml
[model]                    # 已有
provider = "deepseek"

[theme.screen]
margin_left = 2

[theme.feed]
gap = 1
separate_all = true

[ui.feed]
thinking_mode = "full"

[ui.panel]
mode = "shown"
position = "left"

[[server]]
name = "devops-mcp"
kind = "streamable_http"
# …
```

### 2. 每域「config.toml 优先，旧文件 fallback」

每个读取器先看 `config.toml` 里有没有本域的 section：
- theme：`config.toml` 有 `[theme]` 子表 → 用它；否则读 `theme.toml`（旧行为）。
- ui-state：有 `[ui]` 子表 → 用它；否则读 `ui-state.toml`。
- mcp：有 `[[server]]` → 用它；否则读 user `mcp.toml` + 项目 `mcp.toml`（旧 merge 行为）。

判定「有/无」以 section 是否存在为准（空表算「有」）。单文件内字段级缺省仍走各解析器的既有 default 语义。

### 3. 解析器改造：提取子表后复用现有 section 逻辑

- **theme**：把 `Theme::parse(text)` 的 section 遍历抽成 `parse_sections(table: &TomlTable)`（纯函数）；`parse` = 解析全文后调 `parse_sections`；新增 `parse_namespaced(text)` = 解析全文 → 取 `theme` 子表 → `parse_sections`。`load()` 改为：读 `config.toml` → 有 `[theme]` 用 `parse_namespaced`，否则读 `theme.toml` 走旧路径。`load_from(path)`（测试用）语义不变。
- **ui-state**：`parse` 已经按 section 读；新增「从 `[ui]` 子表解析」的路径（`ui.feed.thinking_mode` / `ui.panel` / `ui.graph`）。`load()` 读 `config.toml` → 有 `[ui]` 用它，否则 `ui-state.toml`。
- **mcp_scan**：`McpConfigToml` 加 `server` 字段即可直接从 config.toml 读 `[[server]]`（同形）；`scan_mcp_servers(user, project)` 签名不变，但内部改为「先读 config.toml，有 `[[server]]` 直接返回，否则读 user+project 旧文件」。新入口 `scan_mcp_servers_from_config(config_path)` 或扩展现有入口。

### 4. 路径解析共享

`config.toml` 路径已由 `config_payload::config_path(home)` 解析（`$THEWAY_DIR` > `--home` > `$HOME/.theway`）。theme/ui-state 目前用 `base_dir()`（只认 `$THEWAY_DIR`/`$HOME`，不认 `--home`）。**决策**：theme 与 ui-state 的 config 读取复用 `config_base_dir(home)` 语义（接受 `--home`），但为最小改动，先让它们读 `config_payload::config_path` 同源路径；`--home` 支持通过 `App` 持有已解析的 config 路径传入（theme 的 reload 重读用同一路径）。fallback 旧文件路径保持 `base_dir()` 语义不变。

### 5. 测试策略

- theme：`[theme]` 子表解析 == 顶层 section 解析（同一张表两种路径结果一致）；无 `[theme]` 时 fallback `theme.toml`；`[model]`/`[ui]`/`[[server]]` 存在于 config.toml 时不影响 theme 解析。
- ui-state：`[ui.*]` 解析；无 `[ui]` fallback；字段缺省。
- mcp_scan：config.toml 有 `[[server]]` → 用它（不读旧文件）；无 → 读 user+project 旧文件（保持现有 merge/覆盖/诊断行为）。
- 迁移脚本（或手工步骤）验证：合并后 `config.toml` 解析出的 theme/ui-state/mcp 与旧三文件一致。

## Risks / Trade-offs

1. **命名空间迁移是破坏性的（对现有 theme.toml 用户）**：`[screen]` → `[theme.screen]`。但 fallback 兼容旧文件，且本仓库一次性迁移用户目录，风险可控。
2. **theme reload 的 `--home` 支持**：若 `App` 不传 home，reload 用 `base_dir()`（`$THEWAY_DIR`/`$HOME`）读 config.toml，与启动时 `--home` 路径可能不一致。用 `App` 持有启动时解析的 config 路径解决；未持有则退回 base_dir 语义（与现状一致）。
3. **双读 IO**：theme/ui-state/mcp/config_payload 各自读一次 config.toml（启动时数次文件读），可接受；后续可统一为「启动读一次、传文本」。
