# 主题接口（v2 设计）

[English](theme.md) | 中文

本文设计 theway TUI 的 **v2 主题接口**——v1 `theme.toml` 子集（issue #43 + #49）的继任者。v1 是一个手写解析的窄 TOML 切片：`[colors]` 里的颜色角色、`[blocks.<kind>]` 里的块布局（`bg` / `padding` / `align`）、以及六个 composer 颜色。它无法控制垂直节奏（块间间隔硬编码为一行空行），没有命名颜色，除了 composer 之外没有组件覆盖，也没有主题变体。

设计借鉴了两个参考生态：

| 思路 | 来源 | 我们借鉴什么 |
| --- | --- | --- |
| 命名颜色 palette + `p:name` 引用 | Oh My Posh（`palette`） | 颜色集中在一处；语义角色引用 palette 条目 |
| 超出 hex 的颜色字面量（`transparent`、ANSI 名、`none`） | Oh My Posh | 可选颜色可以被清除；主题文件保持可读 |
| 条件 palette（亮/暗） | Oh My Posh（`palettes.template`） | `follow_system` + 暗/亮变体，由终端 OSC11 驱动 |
| 主题变体 + `auto` | Grok TUI（`ThemeKind` + `Auto`） | 内置命名主题 + 运行时选择器；"auto" 解析为暗/亮 |
| 逐组件的 segment 样式 | Oh My Posh（segments） | 每个 chrome 组件（composer、状态带、选择器、侧栏、DAG band）都有小而扁平的样式表 |
| 按终端色深量化颜色 | Grok TUI（`color_support`） | Truecolor → 256 → 16 降级，弱终端也能用主题 |

pi 自己的主题文件是闭源的；它的*形态*（TOML、语义角色）正是 v1 已经采用的，所以本方案沿用 TOML 并扩展 v1 的 section，而不是发明新格式。

## 目标与不变量

1. **向后兼容。** 每个 v1 文件都能原样解析、渲染结果不变。
2. **默认 = 今天。** 没有主题文件时，渲染必须与当前硬编码的 tokyonight 常量像素级一致。
3. **单一来源。** 所有颜色和布局数字都来自解析后的 `Theme`；硬编码常量收缩为默认主题的值。
4. **渐进式。** v2 的 section 分阶段落地；每阶段可选且独立可用（第一阶段就交付 feed 间隔）。
5. **宽容。** 未知 section/键在 stderr 警告并保留原值——与 v1 姿态一致。

## 文件格式与加载

沿用 `~/.theway/theme.toml`（AGENTS.md 里的运行时状态布局）。v2 把解析从手写子集升级为工作区里的 `toml` crate，然后映射到同一个 `Theme` 结构体。

```toml
# ~/.theway/theme.toml — 用户主题（v2）
theme = "groknight"          # 可选：要叠加的内置变体
follow_system = false        # 可选：通过终端 OSC11 解析暗/亮
```

### 优先级

项目覆盖用户，用户覆盖内置变体，变体覆盖默认主题：

```
<内置变体>                （例如 "groknight"、"tokyonight"）
  └─ ~/.theway/theme.toml         （用户）
       └─ <cwd>/.theway/theme.toml  （项目，可选）
```

项目文件通过现有的 per-cwd 资源发现机制拾取；项目文件缺失不算错误。合并规则与 settings payload 相同：出现的键替换，缺失的键保留。

## 颜色系统

### 1. 语义角色（v1，保留）

`[colors]` 仍是锚点集合。渲染器里每个硬编码颜色（`feed_render`、`prompt_chrome`，以及新组件）都变成角色。

### 2. 命名 palette（新增）

```toml
[palette]
accent    = "#7AA2F7"
muted     = "#565F89"
danger    = "#F7768E"
surface   = "#24283B"
```

任何颜色槽位都接受引用：`p:accent`。palette 条目可以引用其他条目（一层，禁止环——检测到环就警告）。缺失的 palette 键会警告并回退到该槽位的默认值。

### 3. 颜色字面量（新增）

所有槽位走同一个 `parse_color` 路径，接受：

| 字面量 | 示例 | 含义 |
| --- | --- | --- |
| Hex | `"#7AA2F7"` | Truecolor（v1 格式） |
| 短 hex | `"#7AF"` | 展开为 `#77AAFF` |
| ANSI 名 | `"red"`、`"lightBlue"`、`"default"` | 终端调色板引用 |
| 256 索引 | `"146"` | 256 色调色板索引 |
| `"transparent"` | — | 无颜色（清空 `Option<Color>` 槽位） |
| `"none"` | — | 可选槽位中 `transparent` 的别名 |
| `darken(#RRGGBB, 20)` / `lighten(#RRGGBB, 20)` | — | HCL 明度偏移（v2 第三阶段） |

### 4. 条件变体（v2 第三阶段）

```toml
[theme.dark]   # follow_system = true 且终端为暗色时使用
[palette.dark]
accent = "#7AA2F7"
[theme.light]
[palette.light]
accent = "#34548A"
```

`follow_system = true` 通过终端背景查询（OSC11）解析；终端不回答时，最后显式的 `theme` 生效。`theme = "auto"` 是 `follow_system = true` 的别名。

## 屏幕视口

`[screen]` 把**整个** UI 从终端边缘内缩——feed、状态栏、输入框、侧栏、
picker 与浮层都渲染在 margin 之内，布局不再贴着终端边框：

```toml
[screen]
margin = 2             # 四边统一内缩（默认 0）
margin_left = 3        # 单边覆盖；例如左侧多留呼吸空间
margin_top = 0
```

- `margin = N` 同时设置四边；`margin_top` / `margin_right` /
  `margin_bottom` / `margin_left` 单独覆盖某一边（在统一值之后应用）。
- margin 大于终端尺寸时视口收缩为零而非下溢（饱和计算）。
- 默认全为 0（紧贴），因此现有主题与无主题渲染与之前逐字节一致。

## Feed 布局（第一阶段——交付 feed 间隔）

v1 接口无法表达的垂直节奏。`should_separate` 继续决定间隔*放哪里*；主题决定*放多少*。

```toml
[feed]
gap = 1                # 块间空行数（默认 1，0 = 紧贴）
separator = ""         # 可选块间行字形，例如 "─"
separator_style = "p:muted"
```

- `gap = 0` 完全禁用块间空行（单轮对话更紧凑）。
- `separator` 非空时在 gap 空行**下方**渲染一条全宽样式线（总间距 = `gap` 个
  空行 + 1 行分隔线；`gap = 0` 时仅分隔线分隔块）。空字符串 / 缺省 = 纯空行。
- 两者都通过 `FeedRenderOptions` 的主题指纹进入 feed 渲染缓存，改动自然会失效重建。

## 块布局（第二阶段）

`[blocks.<kind>]` 在 v1 的 `bg` / `padding` / `align` 之外增加垂直控制：

```toml
[blocks.tool]
bg = "p:surface"
padding = 1
align = "left"
margin_top = 0         # 该类块上方的额外空行（默认 0）
margin_bottom = 0      # 该类块下方的额外空行（默认 0）
border_top = "none"    # "none" | "thin" | "thick" — 块上方的样式线
border_bottom = "none"
border_style = "p:muted"
```

- `margin_top` / `margin_bottom` 累加（绝不削减）到 `[feed] gap`——用于按类强调，比如每个工具调用都隔开。
- 边框渲染在块的背景带内部，不扰动 feed 节奏。
- v1 文件（只有 bg/padding/align）保持今天的输出。

## 组件覆盖（第二阶段）

其余每个硬编码颜色都收编为主题里的扁平样式表。命名模式是 `<component>_<part>`；每个值都是颜色字面量或 palette 引用。

```toml
[composer]             # v1 键保留；新增标记为 (+)
border_focused = "#4B5C8C"
border_unfocused = "#3C4B78"
prefix = "p:accent"
text = "#C0CAF5"
bg = "#24283B"
info_text = "#A9B1D6"
placeholder = "#565F89"      # (+)
hint = "#565F89"             # (+)
cursor = "#C0CAF5"           # (+)

[statusbar]
bg = "#1F2335"
fg = "#A9B1D6"
accent = "p:accent"
error = "#F7768E"
busy = "#9ECE6A"

[picker]
bg = "#1F2335"
fg = "#7DCFFF"
highlight_bg = "#7DCFFF"     # 选中行
highlight_fg = "#1A1B26"
title = "#E0AF68"
dim = "#565F89"

[sidebar]
bg = "#1F2335"
fg = "#A9B1D6"
heading = "#7AA2F7"
badge = "#9ECE6A"
muted = "#565F89"

[dag_band]
bg = "transparent"
fg = "#A9B1D6"
ok = "#9ECE6A"
failed = "#F7768E"
cancelled = "#E0AF68"
running = "#7DCFFF"
pending = "#565F89"
edge = "#3B4261"
title = "#C0CAF5"

[scrollbar]
thumb = "#3B4261"
track = "transparent"
```

Theme 结构体为每个组件增加一个 `StyleTable`；渲染器只从这里读取。未设置的键回退到默认主题的值，所以只设置 `[feed] gap` 的主题也能完整工作。

## 内置变体（第三阶段）

命名变体内置于二进制（默认 Grok-night，至少再加 TokyoNight 和 Grok-day）。`theme = "<name>"` 选择一个；用户/项目文件逐字段覆盖它。`/theme` 命令和选择器条目列出可用变体，与模型选择器流程一致。

## 量化（第三阶段）

在没有 truecolor 的终端上，解析后的主题在启动时（以及实时变更时）量化一次：truecolor → 最近 256 → ANSI。变体声明是否容忍量化（中性灰 palette 可以；蓝色调在 256 下会浑浊——与 Grok 相同的规则）。

## 迁移

1. v1 解析器保留到第二阶段；v2 使用 `toml` crate，以相同语义接受所有 v1 键。`Theme::parse` 测试套件（默认值匹配硬编码常量、未知键警告、缺失 section 保持默认）原样延续。
2. 新默认值就是当前硬编码常量——什么都不变。
3. 生成一份带注释的 `theme.example.toml` 记录每个 v2 键（`/theme example` 或文档）。

## 落地计划

| 阶段 | 范围 | 交付 |
| --- | --- | --- |
| 1 | `toml` crate 解析器 + `[feed] gap`/`separator` + palette 基础 + `transparent`/`none` 字面量 | 用户诉求：可调块间隔；地基 |
| 2 | 块 margin/border + 组件表（composer 扩展、statusbar、picker、sidebar、dag_band、scrollbar） | 全表面覆盖；每个硬编码颜色角色化 |
| 3 | 内置变体 + `/theme` 选择器 + `follow_system` 暗/亮 + 量化 + `darken`/`lighten` | 与 omp/grok 平齐的主题生态 |

每个阶段都守住不变量：没有主题文件时渲染与今天完全一致。第一阶段小而自洽——本设计确认后即可落地。
