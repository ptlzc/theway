# tui-busy-snake-loader

Issue: #42

## Problem

#37/#38 落地的 9 点状态轮有两个问题：

1. **太大**：3×3 网格占满 3 行 busy 带，working 标签只有 1 行——轮子高度
   是标签的 3 倍（要求 ≤120%）。
2. **像乱转**：三张 pinned 顺序表轮换点亮 9 个点（3 行网格里跳来跳去），
   不是用户想要的「彩虹色贪食蛇跑来跑去」。

## What changes

busy 带的 loader 从 3×3 网格改为**一条 1 行高的彩虹贪食蛇**：

- **尺寸**：蛇占 1 行（= working 标签高度，≤120% 约束）；`BUSY_STATUS_ROWS`
  3 → 2——蛇轨行（第 1 行）+ working/计时/队列/↑scrolled/stats 行（第 2 行）。
- **贪食蛇**：蛇头（高亮、方向字形 `▸`/`◂` 随运动方向）带蛇身
  （~6 节 `■`，节 0 = 头，节 i 在蛇头走过的轨迹上跟随）；蛇头沿轨道
  左右往返（三角波路径，跑到行尾折返——"跑来跑去"），确定性轨迹。
- **彩虹**：蛇身从头到尾色相渐变（复用 pixel_loader 的
  `HUE_TRAIL_OFFSET_DEG` 40°/节 语义），整体色相随时间推进
  （`HUE_STEP_DEG` 15°/步）；亮度从头到尾衰减（头最亮）。
- **速度语义不变**：`RainbowSpinner` advance/tick 与 `step_delay_ms(cps)`
  照旧（吞吐高蛇跑得快，无流回落 250ms/步）。
- **模块**：新 `ui/snake_loader.rs`（pixel_loader.rs 已 437 行，蛇逻辑
  独立成模块）：`snake_frame(step, cps, track_width) -> SnakeFrame`
  （每节 x 坐标 + glyph + fg + lit，纯函数）；pixel_loader 的
  `hsv_to_rgb` 转 `pub(crate)` 供复用。
- **保留**：`pixel_loader::rainbow_frame` 与 `PixelFrame`（dag band 的
  braille mini spinner 继续使用，不在此范围）；`shimmer_style` working
  标签、stats 行不动。
- 布局：蛇轨行满宽（`area.x .. area.right()`）；标签行起点从 x+8 收紧到
  x+1（网格清除区不再需要）；stats 仍在第 2 行右侧。

## Out of scope

- dag band 的 mini spinner（braille 单格）不改——用户没提。
- thinking 统计行 / 工具块指示器等其它 rainbow_frame 消费方不改。

## Acceptance

- busy 时一条彩虹蛇在 1 行轨道左右跑动，蛇身渐变跟随蛇头，折返方向
  字形翻转；busy 带 2 行（轮子高度 = working 高度 ≤120%）。
- 吞吐高蛇加速、无流回落基准（与现行为一致）。
- `make check` / `make test` / `make lint` / `make fmt-check` 通过；
  tmux e2e 截图验证。
