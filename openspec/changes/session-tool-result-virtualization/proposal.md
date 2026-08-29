# session-tool-result-virtualization

Issue: #49

## Problem

工具结果目前**在工具内部被截断**后才进入会话：`bash` 只保留尾部
（2000 行 / 256 KiB，`truncate_tail`），session entry 里存的就是截断文本。
后果：

1. **源头丢信息** —— 完整输出从未落入 session db，之后无法再读回。
2. **长输出仍然撑爆 LLM 上下文** —— 256 KiB ≈ 65K token，模型真正能用
   的信号只是一小部分。
3. **按年龄"新鲜/陈旧"的方案与 prompt cache 冲突** —— 边界每轮移动，
   使 provider 侧缓存（Anthropic `cache_control`、OpenAI `prompt_cache_key`
   已在用）反复失效。

## What changes

流水线翻转：**全量入库，上下文构建时确定性虚拟化**。

1. 工具返回**全量**输出（只保留宽松保险上限，如 10 MB），session entry
   原样存储；截断统计注记仅保留给 TUI 展示层
   （`compact_tool_output_lines` 已是 display-only，不动）。
2. 上下文构建时（`build_context` / `transform_context` 钩子，
   run_loop/llm.rs 已预留）对超过大小阈值（如 4 KiB）的 tool result 做
   **确定性**占位符替换（自描述、带 tail preview）：
   `[tool_result bash call_abc123: 1.2 MB / 5230 lines, exit 0; tail: …]`
   替换只依赖工具名 / call id / 大小 / exit code / 尾部，**不依赖年龄**，
   上下文前缀跨轮字节级稳定 → prompt cache 命中，仅新 token 追加。
3. 小结果（< 阈值）保持内联不变 —— 常见情形零额外往返。
4. 与 compaction 正交：entries 是 append-only，compaction 只追加
   Compaction 条目不删行，旧 tool result 仍在 db，占位符之后仍可按需读取。

## Out of scope

- 按需读取/检索工具（见 #50，依赖本 change）
- subagent/DAG 的 tool result（在 job registry 而非 session tree，后续单独做）
- TUI 浏览工具结果的界面

## Acceptance

- > 4 KiB 的 bash 输出：session db 存**全量**文本（sqlite 直查验证）
- LLM 请求消息里该结果显示为占位符（tail preview + 字节/行数）
- 两轮连续请求，历史未变时 LLM 上下文前缀字节级一致（测试断言）
- `make test` + `make lint` 全绿
