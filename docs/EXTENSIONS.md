# theway 插件编写指南 (TS extensions)

theway 的插件系统是 **core 级模块** (`theway_core::extensions`): 用户把单文件
TypeScript 插件放进 `.theway/extensions/`,运行时自动发现、用 oxc 转译、在**嵌入式
QuickJS** 中执行 — 无外部运行时、无 npm、无构建步骤。

任何运行时能力都可以声明一个**扩展点** (`kind`),插件声明自己实现哪个扩展点并导出
该扩展点的 hooks。目前内置一个扩展点: **压缩算法** (`kind = "compaction"`)。

> 与 pi 插件的关系: pi 的插件是 TS ESM + `package.json` `pi.extensions` 约定,运行在
> pi 自己的 TS 运行时里 (强依赖 `pi-tui` / `pi-coding-agent`),格式无法复用。本系统
> **借鉴 pi 的目录发现方式,但契约是 theway 自己的**。

## 目录结构

```
crates/theway-core/src/extensions/        # 插件系统本体 (core 级模块)
  mod.rs      TsExtension + ExtensionRegistry (发现 / kind 路由 / 契约)
  ts.rs       oxc TS→JS 转译 + rquickjs (嵌入式 QuickJS) hook 执行
crates/theway-core/src/runtime/compaction/
  algorithm.rs   CompactAlgorithm trait + 内置默认 + TsCompactAlgorithm 适配器
  compaction.rs  compact() 编排 — 通过 trait 分发
```

## 快速开始

1. 写一个 TS 文件:

```ts
// ~/.theway/extensions/my-algo.ts
export const kind = "compaction";
export const description = "只保留最近 10 条消息";

export function select_cut_point(ctx: { entries: unknown[] }): { cut_index: number } {
  return { cut_index: Math.max(0, ctx.entries.length - 10) };
}
```

2. 配置选用: `CompactionSettings.algorithm = "my-algo"` (默认 `"builtin"`)。
3. 完成。切点用你的算法,触发判定和摘要生成仍走内置逻辑 (未导出的 hook 自动回退)。

## 发现机制

```
<cwd>/.theway/extensions/*.ts      # 项目级 (同名冲突时优先)
$THEWAY_DIR/extensions/*.ts        # 用户级 (默认 ~/.theway/extensions/)
```

- 文件名 (stem) 即插件名 (压缩场景下即算法名)。
- 没有合法 `kind` 导出、或 TS 语法错误的文件会被跳过并记录诊断
  (`ExtensionRegistry::errors`,永不致命)。
- 发现发生在 harness 启动时;暂无热重载。

## 契约

一个文件 = 一个插件。**必须**导出 `kind` 声明扩展点;`description` 可选。

### Hook 命名规范

- **动词 + 宾语** (动宾结构),全小写下划线 (snake_case)。
- 同一扩展点的 hooks 共享同一命名风格,Rust trait 方法名与 TS 导出名**完全一致**
  (Rust 侧即用 snake_case,无需转换)。
- 每个 hook 语义: "在什么时机,做什么决策/动作"。

示例 (`kind = "compaction"`):

| Hook | 签名 | 语义 |
|------|------|------|
| `decide_compact` | `(ctx) => boolean \| undefined` | 决定本次是否触发压缩 |
| `select_cut_point` | `(ctx) => { cut_index: number } \| undefined` | 选择切点 (entries[..cut_index] 被折叠) |
| `summarize_prefix` | `(ctx) => string \| undefined` | 为被折叠的前缀生成摘要文本 |

**回退规则**: 未导出的 hook、或返回 `undefined`/`null` (主动放弃) → 该步骤回退到
内置默认行为。返回类型错误 (如 `decide_compact` 返回非布尔) → 同样回退并告警。

### JSON 协议 (ctx 结构)

所有字段 snake_case。settings 结构:

```ts
interface CompactionSettingsJson {
  enabled: boolean;
  reserve_tokens: number;
  keep_recent_tokens: number;
  algorithm: string;
}
```

各 hook 的 ctx:

```ts
// decide_compact
interface DecideCompactCtx {
  context_tokens: number;      // 当前上下文 token 估算
  context_window: number;      // 模型上下文窗口
  settings: CompactionSettingsJson;
}

// select_cut_point
interface SelectCutPointCtx {
  entries: unknown[];          // 序列化 SessionTreeEntry[] (serde tags, snake_case)
  settings: CompactionSettingsJson;
}

// summarize_prefix
interface SummarizePrefixCtx {
  messages: unknown[];         // 序列化 AgentMessage[]
  settings: CompactionSettingsJson;
  custom_instructions?: string; // 用户附加指令 (未提供时为 undefined)
}
```

## 内置默认算法 (`"builtin"`)

纯 Rust 实现,无需 TS 文件 (`BuiltinCompactAlgorithm`):
**80% 窗口阈值触发** + **turn 边界安全的 keep_recent_tokens 切点** + **LLM 摘要**
(带溢出预算重试)。它同时是所有 TS 插件未覆盖 hook 的回退实现。

## 完整示例: 自定义压缩算法

```ts
// ~/.theway/extensions/aggressive.ts
export const kind = "compaction";
export const description = "激进压缩: 40% 窗口即触发, 只保留最近 5 条消息, 静态摘要";

export function decide_compact(ctx: {
  context_tokens: number;
  context_window: number;
  settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number; algorithm: string };
}): boolean {
  return ctx.context_tokens > ctx.context_window * 0.4;
}

export function select_cut_point(ctx: {
  entries: unknown[];
  settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number; algorithm: string };
}): { cut_index: number } {
  return { cut_index: Math.max(0, ctx.entries.length - 5) };
}

export function summarize_prefix(ctx: {
  messages: unknown[];
  custom_instructions?: string;
}): string {
  // 序列化的 AgentMessage: { role: "user" | "assistant" | ..., content: ... }
  const userMsgs = ctx.messages.filter((m: any) => m?.role === "user");
  return `早期对话已压缩。用户原话要点: ${userMsgs
    .slice(0, 3)
    .map((m: any) => (typeof m?.content === "string" ? m.content : ""))
    .join(" | ")}`;
}
```

## 调试

- 启动日志: `ExtensionRegistry::errors` 里的诊断 (文件不可读 / 解析错误 / 缺 `kind`)。
- hook 回退: 类型不符、缺 hook、运行时抛异常都会 `tracing::warn` (带算法名),不中断会话。
- 每次 hook 调用都是**全新 QuickJS 上下文** — 插件抛异常不会污染宿主,但也没有跨调用状态。

## v1 限制

- 单文件: 不支持 `import`/`export` 其他模块,无 npm 依赖。
- hook 是同步纯函数: 无 async hook、无宿主回调 (v1 的 `summarize_prefix` 只能返回
  字面文本;LLM 摘要仍是内置算法专属,TS 内调 LLM 是未来扩展)。
- 发现于启动时,无热重载。
- 插件内无文件系统/网络访问 (QuickJS 默认无宿主 IO)。

## 测试

`crates/theway-core/tests/ts_extensions.rs` — 发现、kind 路由、JSON 契约、项目>用户遮蔽、
坏文件诊断、注册表分发与内置回退。
