# 流式渲染与快照减负

## Why

#34 把 TUI 每帧成本降到 O(viewport) + 脏块重渲染，但三个 O(history) 路径仍在：

- 流式输出时最后一个 assistant/thinking 块每帧全量重走 markdown/syntect，总量 O(n²)。
- daemon 每帧快照重新生成并序列化整个 feed 的 100 列纯文本行（`feed_lines`），wire 字节 O(history)/帧。
- 每个 token chunk 都触发一次快照发布，无合并。

## What changes

- **流式尾块增量渲染**（`theway-tui`）：`FeedRenderCache` 检测最后一个块为 assistant/thinking
  且纯追加时进入流式模式。assistant 用 `StreamingMarkdownRenderer`（冻结行只处理一次，每帧
  O(delta+tail)）；thinking 用增量折行器（复用 `wrap_str` 语义，每帧 O(delta)）。非追加变更
  （如 thinking 摘要回填）自动回退一次性渲染。
- **daemon 快照合并**：事件驱动的 `publish_snapshot` 改为 50ms 脏标记 + 定时 flush。
- **feed_lines 尾部化**（`theway-transport` + proto + daemon）：新增 `PlainLinesCache`（块指纹 +
  行起始偏移，只重渲染脏后缀）；`SessionState` 增加 `feed_lines_base`，快照只带新增行；
  headless 客户端按 base 合并。

## Impact

- 协议：`SessionState` 增加 `uint64 feed_lines_base = 17`（默认 0，向后兼容）。
- 帧率：token 洪峰期间快照帧率降至 ~20fps；交互命令延迟 ≤50ms。
- 视觉与滚动语义不变。
