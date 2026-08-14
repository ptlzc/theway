# TUI grok 对标打磨 + daemon thinking 摘要

## Why

theway 的 TUI 会话视图与 grok 的终端体验还有差距：输入框没有光标、信息行右下角没有上下文用量、
用户消息与工具调用全部平铺渲染、assistant 消息里的 mermaid 图没有宽度适配、thinking 输出只能全量显示、
用户上滚时新输出会强制跳回底部、feed 里无法选择文字。同时 daemon 侧缺少把冗长 thinking 输出压缩成
结构化摘要的配置（每次 thinking 结束交给 subagent 总结后回填，避免长思维链占据会话视野）。

## What changes

- **theway-tui**: 输入框聚焦时渲染光标；prompt chrome 信息行右侧显示上下文用量（token / context window 百分比）；
  feed 渲染对标 grok（用户消息 `❯` accent 前缀 + 提升色带；工具调用 `⏵` 前缀 + 淡色参数；工具结果默认折叠为
  一行摘要，可全局展开）；mermaid fence 以 feed 宽度渲染成图；thinking 渲染三态（隐藏 / 查探窗口 / 完整）由
  Ctrl+O 循环；feed 上滚后保持用户滚动位置（apply_snapshot 不再强制 follow）；feed 文字选择高亮（暂不复制）。
- **theway-transport**: `WireStatus` 增加 `usage`（token 用量 + context window），proto `SessionState`
  增加 `ContextUsage`，gRPC/HTTP 快照往返携带；`[orchestrator] thinking_summary` 配置解析。
- **theway-daemon**: 快照发布 token 用量与当前模型 context window；thinking 突发结束后按配置把内容交给
  subagent 生成结构化摘要，回填替换 feed 中的 thinking 块（thinking summarization / 思考压缩）。
- **theway-markdown**: 公开带最大宽度的 full 渲染入口（把 `max_table_width` 传给 mermaid 渲染器）。

## Impact

- 协议：`SessionState` 增加可选 `context_usage` 字段（向后兼容）。
- 配置：`~/.theway/config.toml` 新增可选 `[orchestrator]` 段：`thinking_summary`（bool）与
  `thinking_summary_min_chars`（usize，默认 2000）。
- 交互：Ctrl+O（thinking 三态）、Ctrl+T（工具结果展开/折叠）、Ctrl+Space / Shift+↑↓（选择）。
