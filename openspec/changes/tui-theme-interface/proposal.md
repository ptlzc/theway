# tui-theme-interface

Issue: #43

## Problem

feed 块渲染的颜色全部硬编码在 `crates/theway-tui/src/feed_render.rs` 的
const 里（`THINKING_STYLE` / `TOOL_NAME_STYLE` / `USER_STYLE` 等），且工具
调用、thinking 块没有背景色。用户要求：

1. tools 调用与 thinking 的方块支持定义背景颜色；
2. TUI 像 pi 一样暴露出 theme 接口。

## pi 参考（已调研 ~/pi-src/config/）

- `themes/dark-theway.json`：`vars`（命名色值变量，hex）+ `colors`（角色 →
  变量名或 hex：`toolPendingBg` / `toolSuccessBg` / `toolErrorBg` /
  `toolTitle` / `toolOutput`、`userMsgBg`、thinking 五级、markdown、
  syntax 等）+ `export`（page/card/info 背景）。
- `settings.json`：`"theme": "dark-theway"` 选择生效主题。

## What changes（v1）

**新 theme 层**（theway 惯用 TOML，pi 的 JSON 形态仅参考）：

- `crates/theway-tui/src/theme.rs`：`Theme` 结构——命名颜色角色 +
  `Theme::default()` = 现状硬编码色（完全向后兼容）；`Theme::load(path)`
  解析 `~/.theway/theme.toml` 的 `[colors]` 表 `role = "#rrggbb"`；
  未知角色忽略、非法 hex warn + 回落默认、空文件/无文件 → 默认。
- 角色清单 v1（默认值 = feed_render 现有 const）：
  `user_text` / `user_bg`（现有 BG_HIGHLIGHT user band）/ `assistant_text` /
  `assistant_prefix` / `tool_title` / `tool_args` / `tool_result` /
  `tool_error` / `tool_running_bg` / `tool_success_bg` / `tool_error_bg` /
  `thinking_text` / `thinking_bg`。
  v1 可见变化：工具调用块（标题行 + args + result 展开/预览）与
  thinking 块（Full/Peek）获得背景色，且背景铺满块行宽（真"方块"；
  块内各行宽度归一后补 bg span）。
- 加载与传递：启动时加载一次（TUI 入口），存 `App.theme`；
  `FeedRenderOptions` 增加已解析的块颜色字段（`#[derive(PartialEq)]`
  已有，feed_cache 指纹随之覆盖主题——启动一次性加载，运行期不变）。
- `feed_render.rs`：const 迁移为 theme 角色默认值；块渲染路径改读
  opts 里的主题颜色；`Block::Tool`（含 result 展开/preview）、
  `Block::Thinking`（stats 行 + 段落行）设背景。

## Out of scope

- prompt chrome / dag band / picker / 状态带颜色不纳入 v1（后续加角色）。
- syntax 高亮配色覆盖、pi 式多主题目录（themes/）+ settings 选择、
  运行期热切换（/reload 不重读主题）。

## Impact

- 仅 theway-tui；无 wire/daemon 改动。
- 默认（无 theme.toml）视觉与现状完全一致。
- `~/.theway/theme.toml` 为新配置文件，文档在 README/help 里加一节。

## Acceptance

- 无 theme.toml 时视觉不变；写 theme.toml 后 tool/thinking 块背景色
  生效且铺满块宽。
- `make check` / `make test` / `make lint` / `make fmt-check` 通过；
  tmux e2e 截图验证（自定义背景 + 默认回归）。
