# 快照线上瘦身与重同步协议

## Why

#34/#35 把渲染侧的每帧成本降到了 O(viewport)+O(脏块)，但快照线上仍有 O(history) 路径：

- `feed_blocks` 每帧全量上 wire：daemon `wire_blocks()` 克隆全部块 + proto 编码全部块；
  TUI `apply_snapshot` 的前缀 zip 比较对每个历史块做逐字节 memcmp，流式期间最后一个块
  每帧变化还会触发 `Feed::replace_blocks` 全量重建。
- #35 把 `feed_lines` 尾部化做在 `WireStatus` 层：`get_state` / HTTP SSE / JSON-RPC 与
  任何中途接入的消费者拿到 tail-only（base>0）数据——web viewer 与 headless 重连会缺行。
- gRPC 流在 `Lagged` 时静默丢帧（`filter_map` 返回 `None`）：尾帧协议下丢帧即视图损坏。
- 块级 diff 若每帧重算全部指纹，会把优化从 O(history) 换装成 20fps × O(history) 哈希。

## What changes

**A. 语义分离**：`WireStatus` 永远全量——`feed_blocks` / `feed_lines` 完整（
`PlainLinesCache` 使全量行构建保持增量成本），`get_state`、HTTP JSON/SSE 对任何时刻
接入的消费者都正确。`WireStatus` 另携带自上次发布以来的块补丁
（`feed_block_patches` + `feed_block_base`），由 daemon 的版本指纹 + 事件驱动 dirty
集合增量生产。

**B. 增量只在 gRPC 流层**：每个流持游标（已发行数 / 块数）。首帧全量（base=0）；
后续帧只带尾部与补丁。`Lagged` 或补丁间隙（`feed_blocks_base` 不等于消费端块数）
→ 下一帧转为全量重同步帧。`feed_lines_base` 的游标同样移入流层（每流独立）。

**C. proto**：`SessionState` 增加 `uint64 feed_blocks_base = 18` 与
`message FeedBlockPatch { uint64 index = 1; FeedBlock block = 2; }`（
`repeated FeedBlockPatch feed_block_patches = 19`）。`feed_blocks` 仅在全量帧携带。
补丁语义：`index == 消费端块数` → 追加；`index < 块数` → 原地替换；`index > 块数`
→ 间隙 → 消费端必须重同步（get_state）。

**D. TUI**：`apply_snapshot` 支持全量 / 补丁两种形态，删除前缀 memcmp；
transport 新增 `Feed::replace_block(index, WireFeedBlock)` 单块替换（feed 渲染缓存已有
指纹失配回退，自动处理中段修改）；headless 打印器重连时若 `printed > base + len`
（daemon 重启 / clear）则重置 `printed` 并全量重放。

## Impact

- 协议向后兼容：旧客户端无 `feed_blocks_base`（默认 0）→ 走全量路径。
- `feed_lines_base` 的流层语义不变，但 `WireStatus.feed_lines_base` 恒为 0
  （JSON 消费者语义修正为「永远全量」）。
- 每帧线上成本：无变更帧 = 空补丁 + 尾部空行（O(1)）；流式帧 = 尾部行 + 尾块补丁
  （O(delta)）；中段回填（thinking 摘要）= 单个 replace 补丁。
- 不变量：任何时刻、任何消费者，`get_state` 与 HTTP 状态面永远完整。
