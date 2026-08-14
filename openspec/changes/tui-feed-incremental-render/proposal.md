# TUI feed 增量渲染优化

## Why

daemon 高活动时每个快照帧都会触发 TUI 全量重渲染整个 feed：所有历史块重新走
`theway-markdown`（含 syntect 高亮），再克隆 ~3000 行 `Line` 交给 `Paragraph` 滚动绘制。
两个客户端并发时 CPU 会被打满（#33 收尾烟测实测）。终端侧的 ratatui buffer diff 已经是
增量的，瓶颈全在 widget 渲染输入侧。

## What changes

- **块级渲染缓存**（`theway-tui/src/feed_cache.rs`）：按 feed block 指纹（kind + 内容 fnv 哈希）
  缓存每个块的渲染结果与其行区间；`update()` 用指纹前缀匹配找到第一个脏块，截断并只重渲染
  后缀（重算块间分隔符）；宽度、渲染选项（thinking 三态 / 工具展开）或容量变化时整体失效。
- **可视窗口直绘**（`feed_render::render_lines_window`）：替代 `Paragraph::new(lines).scroll(…)`，
  仅把可视行经 `Buffer::set_line` 写入；选择高亮改为绘制期叠加（只构造选中行），不再每帧
  克隆全部行。首部裁剪改为超容量 + 余量时的低频 `drain`。
- **URL 下划线扫描移入块渲染**：每块渲染一次、随缓存复用，去掉每帧全量正则扫描。
- **流式尾块暂缓**：v1 后每帧成本 = O(viewport) + 最后一个脏块的一次 markdown 渲染；
  长流式输出的 O(n²) 尾部与 daemon 快照 diff（只发尾部）记为后续工作（代码中 TODO）。

## Impact

- 交互与视觉不变（滚动锚定、选择坐标、滚动条、mermaid/thinking/工具折叠全部保持现有行为）。
- `feed_render::lines` 保留（测试用），`App` 渲染路径切换到 `FeedRenderCache`。
