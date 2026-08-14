# 设计：全量状态面 + 增量流帧 + 重同步协议

## 1. 协议形态（proto）

```proto
message FeedBlockPatch {
  uint64 index = 1;    // 绝对块索引：index == 消费端块数 → 追加；< 块数 → 替换；> 块数 → 间隙
  FeedBlock block = 2;
}
message SessionState {
  // …既有字段…
  // 全量帧：feed_blocks_base == 0 且 patches 为空 → feed_blocks 是完整列表。
  uint64 feed_blocks_base = 18;             // 本帧补丁之前，消费端必须已拥有的块数
  repeated FeedBlockPatch feed_block_patches = 19;
}
```

`feed_lines_base = 17`（#35 引入）保留，语义收窄为「gRPC 流层游标」；
`WireStatus.feed_lines_base` 恒为 0（JSON 面永远全量）。

**帧形态**（gRPC `stream_events`）：

| 形态 | feed_blocks | feed_blocks_base | patches | feed_lines | feed_lines_base |
|---|---|---|---|---|---|
| 全量（首帧/重同步） | 完整列表 | 0 | 空 | 完整行 | 0 |
| 增量（正常） | 空 | 消费端应持有的块数 | 自上次发布以来的补丁 | 新增行 | 行游标 |

**消费端判定**：
- `feed_blocks_base == 0` → 全量替换本地状态。
- `feed_blocks_base > 0`：
  - `base == 本地块数` → 按序应用 patches（index==len 追加 / index<len 替换）。
  - `base != 本地块数`（间隙或本地落后/超前）→ 置 `stale`，下一轮 `get_state` 全量重同步。
- 行侧同理：`feed_lines_base + lines.len()` 与本地 `printed` 不一致时按重同步规则处理。

## 2. daemon：全量快照 + 补丁生产

### 2.1 全量状态面（get_state / JSON / SSE）

`wire_snapshot()` 产出全量 `WireStatus`：
- `feed_blocks = self.feed.wire_blocks()`（现状，保留）。
- `feed_lines = plain_lines_cache.rows()`（#35 的 `PlainLinesCache` 保留——它把全量行构建
  变成增量：clean 帧 O(1)，脏后缀 O(delta)）。
- `feed_lines_base = 0`（修正 #35 语义）。

### 2.2 补丁生产（发布时）

`TurnHost` 新增状态：

```rust
block_versions: Vec<u64>,          // 每个块的 fnv 指纹（transport::feed::block_fingerprint）
dirty_blocks: BTreeSet<usize>,     // 事件驱动，见 2.3
patches_out: Vec<WireFeedBlockPatch>, // wire_snapshot 消费后清空
blocks_before_patch: u64,          // 本批补丁之前的块数（feed_block_base）
```

`wire_snapshot()` 末尾（仅发布路径调用，get_state 读 `latest` 存量）：
1. 按序取出 `dirty_blocks`；对每个 index：
   - `index >= block_versions.len()` → append 补丁 `(index, block)`，扩展 versions。
   - 指纹 `h = block_fingerprint(block)` 与 `block_versions[index]` 不同 → replace 补丁，更新版本。
2. `blocks_before_patch = 处理前 versions.len()`；`patches_out = 补丁`；清空 dirty。
3. `WireStatus.feed_block_patches = patches_out`、`feed_block_base = blocks_before_patch`。

成本：每帧只重哈希 dirty 块（典型 = 流式尾块一个；字节数 O(delta 累计)，随 50ms 合并
后即 O(50ms 内增量)）。

### 2.3 事件驱动的 dirty 集合

`apply_feed_update()`（feed 更新的唯一入口）标记 dirty：

| FeedUpdate | dirty 标记 |
|---|---|
| `ThinkingDelta` / `TextDelta` | 最后一个块 index（流式增长） |
| `ToolStart` / `ToolEnd` / `ToolProgress` / `Plain` / `TurnStart` / `TurnEnd` | `feed.apply` 后新增的 index 区间 |
| `ThinkingSummary { block_index }` | 该 index（中段回填） |
| `TriggerPollStatus` / `SkillsReloaded` | 无（不进块流） |

`/clear`（feed.clear 的调用点）→ `dirty_blocks.clear()` + `versions.clear()` +
`patches_out.clear()`（下一次补丁批天然表现为全量收缩；流层会因 base 收缩检测到间隙
而重同步）。

## 3. gRPC 流层：每流游标 + 重同步

`grpc.rs::stream_events` 的 snapshot 分支改为有状态流：

```rust
struct StreamCursor {
    lines_emitted: usize,
    blocks_emitted: usize, // 本地视角：已告知消费端的块数
    first_frame: bool,     // 首帧全量
}
```

- **首帧 / Lagged 后**：发送全量帧（feed_blocks 完整、base=0、feed_lines 完整）。
  `BroadcastStreamRecvError::Lagged` 不再返回 `None`，而是置 `first_frame = true`
  （下一帧全量），同时保留事件帧。
- **正常帧**：
  - 块侧：`base = snapshot.feed_block_base`、`patches = snapshot.feed_block_patches`。
    流自身校验：若 `snapshot.feed_block_base != cursor.blocks_emitted`（广播与本地游标
    漂移，例如 daemon clear 收缩）→ 本帧转全量。
  - 行侧：`lines = snapshot.feed_lines[cursor.lines_emitted..]`、
    `base = cursor.lines_emitted`；若 `cursor.lines_emitted > snapshot.feed_lines.len()`
    （clear 收缩）→ 转全量并重置游标。
- 每帧后更新游标：`lines_emitted = base + lines.len()`；`blocks_emitted = base + 追加数
  （replace 不改变块数）`。

`get_state` 保持读 `latest`（全量），不变。

## 4. TUI 消费端

### 4.1 `apply_snapshot` 双形态

```
全量（feed_blocks_base == 0 && patches 空）：
    latest.feed_blocks = 新列表；self.feed.replace_blocks(新列表)
补丁（base == latest.feed_blocks.len()）：
    for patch: index == len → append_blocks(&[block])；index < len → replace_block(index, block)
    base != len → 标记 resync_pending，触发 get_state（复用现有重连路径）
```

删除 `old_blocks.clone()` 前缀 zip 比较（#33 引入的 tail-append 优化被补丁形态取代）。

### 4.2 `Feed::replace_block`

transport 新增：

```rust
pub fn replace_block(&mut self, index: usize, wire: &WireFeedBlock) -> bool
```

仅当 `blocks[index]` 与 wire 同 kind 时替换（异 kind 拒绝，防错位）；保留时间戳。
`FeedRenderCache` 不需要改动：指纹失配路径 + 流式 resume 前缀校验自动回退到一次性渲染。

### 4.3 headless 打印器重同步

```rust
let end = base + lines.len();
if end < printed { printed = 0; }          // daemon 重启/clear → 全量重放
print lines[printed.saturating_sub(base)..]
printed = end;
```

## 5. 不变量与边界

- **任何时刻的 get_state / JSON / SSE 都完整**：全量语义集中在 `WireStatus`。
- **流层丢失任何一帧都可恢复**：Lagged → 全量帧；间隙 → 消费端 stale → get_state。
- **补丁只增不改协议**：旧客户端（无 base 字段 → 0）永远走全量路径。
- **回填（thinking 摘要）不重建历史**：单个 replace 补丁 → 渲染缓存指纹失配 → 仅后缀重渲染。

## 6. 度量目标（发布前验证）

合成基准（10k 块会话，release）：
- 无变更帧：daemon 发布 + TUI apply 合计 < 20µs（现状：O(history) 毫秒级）。
- 流式追加帧：成本与 50ms 窗口内增量成正比，与历史块数无关。
- 中段回填帧：O(1) 补丁 + 渲染后缀 O(受影响块)。

e2e：tmux 长会话流式输出 CPU 采样 ≤ 10%；headless `echo | theway` 重连无缺行/无重复行；
web 状态面（JSON）在流式期间始终完整。
