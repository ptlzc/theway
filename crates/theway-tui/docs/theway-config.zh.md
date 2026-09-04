# theway 配置指南

[English](theway-config.md) | 中文

面向在 theway 内部工作的 agent 的配置参考：每个配置文件的位置、各 section 控制什么、优先级，以及改动何时生效。本文档打包在 `theway` 二进制内，启动时安装到 `~/.theway/docs/tui.md`，因此任何安装方式都自带这份文档；`tui-docs` 扩展会把你的提示词指向这个路径。

## 基础目录与分层

- `$THEWAY_DIR`（默认 `~/.theway`）是用户根目录。项目层 `<cwd>/.theway/` 按工作目录叠加覆盖。
- 配置文件：`<base>/config.toml`、`<base>/theme.toml`、`<base>/mcp.toml`。项目层额外提供 `<cwd>/.theway/mcp.toml`、`<cwd>/.theway/skills/`、`<cwd>/.theway/templates/` 和 `<cwd>/.theway/extensions/`。
- 运行时状态也在 `<base>` 下：`sessions/`（按 cwd 哈希分桶）、`memory/`、`history`、`exports/`、`logs/`、`auth.json`、`models.json`、`skill-overrides.json`、`extensions/trust.json`、`extensions/audit.jsonl`。

## config.toml

由客户端在启动时读取，并作为 settings payload 提供给 daemon；daemon 自己不读这个文件。优先级为 CLI 参数 > config.toml > 内置默认值。

| Section | 键 | 含义 |
|---|---|---|
| `[model]` | `provider`、`model`、`thinking` | 启动默认模型对与思考等级（`off`、`minimal`、`low`、`medium`、`high`、`xhigh`）。TUI 把最近一次 `/model` 的选择写到这里。 |
| `[builtin_skills]` | `enabled` | 启用的内置 skill 名称；与 `--builtin-skill` 参数取并集。 |
| `[triggers]` | `poll_interval_secs` | 本地动态 trigger 轮询间隔（默认 600）。 |
| `[tui]` | `max_feed_lines` | TUI 对话流回看上限。 |
| `[relay]` | `base_url` | Relay base URL 回退值。 |
| `[orchestrator]` | thinking-summary 设置 | 编排器思考摘要调优。 |

非法值软失败：客户端报告诊断并保留默认值。运行中的 daemon 保持它被 provision 时的值；修改文件需要重启客户端（或走 settings RPC）才会生效。

## theme.toml

带版本的 theme 文件（v2）。文件缺失或未知 section/键会回退到内置默认值并在 stderr 告警。

| Section | 控制内容 |
|---|---|
| `[palette]` | 供其他位置引用的命名颜色。 |
| `[colors]` | 颜色角色（user/assistant/tool/thinking 的文本与背景、状态、picker、sidebar、dag band）。 |
| `[screen]` | 视口内缩：`margin`（四边统一）加单边 `margin_top` / `margin_right` / `margin_bottom` / `margin_left`；默认左边距为 2。 |
| `[blocks.<kind>]` | `user` / `assistant` / `tool` / `thinking` 的块布局：`margin_top`、`margin_bottom`、`border_bottom`、背景。 |
| `[composer]` | 输入框边框颜色：`border_focused`、`border_unfocused`、`prefix`、`text`、`bg`、`info_text`、`placeholder`、`hint`、`cursor`。 |
| `[feed]` | 对话流节奏：块间 `gap`、`separate_all`、分隔样式。 |
| `[statusbar]` / `[picker]` / `[sidebar]` / `[dag_band]` | 组件样式表（前景/背景颜色槽，部分接受 `transparent`）。 |

theme 改动热重载：daemon 侧 reload 使 runtime revision 递增后，已连接客户端无需重启就会重读文件。

## mcp.toml

`[[server]]` 条目定义 MCP stdio/HTTP 服务：`name`、`kind`、`command`、`args`、`endpoint`、`auth`、超时、`reconnect`，以及通知选项 `inject_summary` / `inject_and_run`。从 `<base>/mcp.toml` 与 `<cwd>/.theway/mcp.toml` 读取；启动失败的服务会被跳过并给出诊断。

## Skills、templates 与 extensions

- Skills 从 `~/.theway/skills/`、`<cwd>/.theway/skills/` 和内置 skill 中解析；更近的层按名称覆盖。`/reload` 重新扫描 skills 与 file commands。
- Templates（带 frontmatter 的 `.md`）从 `~/.theway/templates/` 和 `<cwd>/.theway/templates/` 解析；用 `/template <name>` 运行。
- Extension package 从 `<base>/extensions-managed/`、`<base>/extensions/` 和 `<cwd>/.theway/extensions/` 解析（project > user > managed）。官方 package（`tui-docs`）内嵌在 daemon 二进制中，启动时自动装配到 managed 层。Project package 需要在 `<base>/extensions/trust.json` 中有信任记录；用 `/extension-trust` 管理，用 `/extension-reload` 重载。

## 给 agent 的指引

- 配置变更优先使用斜杠命令：`/model`、`/thinking`、`/skills`、`/extension-trust`、`/extension-reload`、`/reload`。它们在运行中的 daemon 里即时生效。
- 只在用户明确要求时直接编辑文件；`config.toml` 需要重启客户端才能让改动到达 daemon。
- 排查"为什么 X 被配置成这样"时，先查优先级顺序，再查上面的分层位置。
