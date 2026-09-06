# Proposal: unify-config-toml

## Why

`~/.theway/` 现在散着 4 个 toml 配置：`config.toml`（`[model]`）、`theme.toml`（11 个主题 section）、`mcp.toml`（`[[server]]`）、`ui-state.toml`（`[feed]/[panel]/[graph]`），外加项目级 `<repo>/.theway/mcp.toml`。用户要改一个主题色、加一个 MCP 服务器、调一下面板位置，得记得每个东西住在哪个文件。单一 `config.toml` 更符合直觉：一个文件承载全部配置。

## What Changes

- **`config.toml` 成为唯一配置源**，采用命名空间化的 section 结构（无 section 冲突）：
  - `[model]`（已有，daemon 运行时配置）
  - `[theme.<section>]`（theme 迁移：`screen` / `feed` / `composer` / `blocks.*` / `thinking` / `statusbar` / `picker` / `sidebar` / `dag_band` / `palette` / `colors`）
  - `[ui.feed]` / `[ui.panel]` / `[ui.graph]`（ui-state 迁移）
  - `[[server]]`（MCP 迁移，顶层数组，与 `mcp.toml` 的 `[[server]]` 同形）
- **向后兼容 fallback**：每个域优先读 `config.toml`；`config.toml` 缺少该域时回退到旧的独立文件（`theme.toml` / `ui-state.toml` / `mcp.toml` + 项目 `mcp.toml`），旧文件继续工作。删除旧文件是渐进式的，删除后 `config.toml` 接管。
- **迁移**：把当前 `~/.theway/` 的 theme.toml / ui-state.toml / mcp.toml 内容合并进 `config.toml`（保留注释语义），并删除这三个独立文件。
- 独立 `thewayd`（无 controller）的本地 `mcp.toml` 读取路径**不动**——它不读 `config.toml`（#73），本次只统一 controller（TUI）侧的配置面。

## Capabilities

纯配置文件组织变更（无跨协议服务契约变化），`skip_specs: true`。

## Impact

- `crates/theway-tui`：`theme`（从 `config.toml` 提取 `[theme]` 子表）、`ui_state`（提取 `[ui]` 子表）、`mcp_scan`（读 `config.toml` 的 `[[server]]`），三者各自带旧文件 fallback。
- `docs/architecture.md`：配置文件清单与 `config.toml` 结构说明同步。
- 用户目录：`~/.theway/config.toml` 合并、三个旧文件删除。

## Non-Goals

- 不改独立 `thewayd` 的本地 `mcp.toml` 扫描（仍读旧文件）。
- 不改 `config.toml` 的 daemon 运行时 section（`[model]` 等）语义。
- 不做配置热迁移工具（一次性手工/脚本迁移即可）。
