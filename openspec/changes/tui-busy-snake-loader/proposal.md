# tui-busy-snake-loader

Issue: #42

## Problem

#37/#38 落地的 9 点状态轮有两个问题：

1. **太大**：3×3 网格占满 3 行 busy 带，working 标签只有 1 行——轮子高度
   是标签的 3 倍（要求 ≤125%，且与 working 水平对齐）。
2. **像乱转**：三张 pinned 顺序表轮换点亮 9 个点，不是用户想要的
   「彩虹贪食蛇跑来跑去」。

用户澄清：**9 个点保留，总高度压到 1 行**（≤ working 高度 125%，与 working
标签水平对齐）；样子
参考 pi 的状态轮。已调研 `~/pi-src/extensions/working-indicator`：pi 用
单格盲文转轮 `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`（10 帧）——彩虹逐帧色相（hue = i/10 × 300°）、
固定屏幕位置（无布局重排）、速度随 token 速率（分档 130/85/50/25/10ms）。
我们吸收「固定位置 + 彩虹 + 速率驱动」的语义，但**保留 9 个点**而不用
盲文字符（盲文单格一个前景色，无法逐点彩虹）。

## What changes

busy 带的 loader 从 3×3 网格改为**单行 9 点彩虹贪食蛇**（pi 式固定位置）：

- **尺寸**：9 点排成 1 行（9 列 × 1 行轨道），总高度 ≤ working 标签高度的
  125%（1 行即 100%，满足）；**蛇行与 working 标签行水平对齐（同一行）**；
  `BUSY_STATUS_ROWS` 删除——busy 带从 3 行缩为 **1 行**
  （与 idle 同高，无布局跳动）：蛇轨道 + working/计时/队列/↑scrolled +
  右侧 stats 同行。
- **贪食蛇**：蛇头（最亮、BOLD）带渐暗蛇尾，沿 9 点轨道左右往返
  （三角波路径，跑到行尾折返——"跑来跑去"）；折返时蛇尾翻到运动方向
  背面（方向感由蛇尾位置体现，不引入方向字形）。蛇头位置 = step 的
  确定性三角波函数，轨道固定 9 格无需宽度参数。
- **彩虹**：蛇尾从头到尾色相渐变（复用 `HUE_TRAIL_OFFSET_DEG` 40°/节），
  整体色相随时间推进（`HUE_STEP_DEG` 15°/步）；亮度从头到尾衰减，
  尾长随吞吐增长（rest 2 → 上限 8，复用 trail 语义）。
- **速度语义不变**：`RainbowSpinner` advance/tick 与 `step_delay_ms(cps)`
  照旧（吞吐高蛇跑得快，无流回落 250ms/步）；pi 的分档映射仅参考，不改。
- **模块**：新 `ui/snake_loader.rs`（pixel_loader.rs 已 437 行，蛇逻辑独立）：
  `snake_frame(step, cps) -> SnakeFrame`（9 个 `SnakeDot { glyph, fg, lit }`，
  纯函数）；pixel_loader 的 `hsv_to_rgb` 转 `pub(crate)` 复用。
  蛇点字形 `●`，未点亮的轨道点保留暗色底（可数出 9 个点）。
- **保留**：`pixel_loader::rainbow_frame` 与 `PixelFrame`（dag band 的
  braille mini spinner 继续使用）；`shimmer_style` working 标签、stats 行
  逻辑不动（移到同行）。
- 布局：蛇轨道在行首（x+1），标签从蛇后 2 格起，stats 右对齐；窄终端
  stats 已有宽度保护（宽度不足不画）。

## Out of scope

- dag band 的 mini spinner（braille 单格）不改——用户没提。
- 不改速率→速度映射曲线（pi 的分档 bucket 不作迁移）。

## Acceptance

- busy 时一条彩虹蛇在 9 点轨道左右跑动，蛇尾渐变跟随蛇头、折返；
  busy 带 1 行（与 idle 同高，无跳动），轮子高度 ≤ working 高度 125%，
  蛇与 working 标签水平对齐（同一行）。
- 吞吐高蛇加速、无流回落基准（与现行为一致）。
- `make check` / `make test` / `make lint` / `make fmt-check` 通过；
  tmux e2e 截图验证。
