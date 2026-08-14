# tui-feed-max-lines

## 背景

TUI 会话 feed 目前无行数上限：`App::render` 每次都通过 `feed_render::lines`
把全部 block 渲染成行并整体持有，会话越长内存增长越无界（issue #27）。

## 目标

feed 滚动回放默认最多保留 **3000 行**：渲染出的行超过上限时从头部裁掉最旧
的行，只保留最新 3000 行；向上滚动（非 follow）时视图不跳变。

## 设计

- `crates/theway-tui/src/ui/mod.rs` 增加
  `const DEFAULT_MAX_FEED_LINES: usize = 3_000;`
- 渲染路径（feed 行 Vec 构建之后）做 head-trim：
  - `trimmed = lines.len().saturating_sub(DEFAULT_MAX_FEED_LINES)`
  - `lines.drain(..trimmed)`
  - `self.scroll = self.scroll.saturating_sub(trimmed)` 同步偏移，
    使非 follow 滚动时可见内容保持不动。
- follow 语义不变：follow 时 scroll 直接设为裁剪后总行数对应的 max_scroll。

## 非目标

- daemon 侧 transcript / SQLite 历史裁剪不动；
- 不做 CLI / 配置可调（后续需要再加）。
