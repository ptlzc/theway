# tui-fork-command-tree-history

Issue: #29

## Problem

pi 上游有会话 fork 和树形会话历史；theway 已移植存储原语
（parentSessionPath metadata、ForkOptions/get_entries_to_fork、
SessionTreeEntry parentId），但没有任何用户可见面：

- 无 `/fork` 命令 —— ForkOptions/get_entries_to_fork 是死代码
- `/resume`、`/sessions`、`--list-sessions` 都是平铺列表，忽略 parentSessionPath

pi 语义：`/fork` = 从历史 user message 创建新会话文件（交互式消息选择器）；
会话文件用 parentSessionPath 串联；历史显示为树（├─/└─/│）。

## What changes

1. 存储：`SessionEntry.parent_id`（解析 metadata parentSessionPath）、
   `fork_session()`（新建 db、重放 path-to-root entries、写 parentSessionPath）、
   树形 flatten helper + pi 风格前缀
2. 守护进程：`/fork` 命令 —— 无参列出 user messages（新→旧编号）；
   `/fork <n>` 在第 n 条 user message 之前 fork（ForkPosition::Before），
   输出 resume 提示
3. TUI/CLI：`/sessions` + `--list-sessions` + `/resume` picker 渲染会话树
   （子会话嵌套在父会话下，├─/└─/│ 字形）

## Out of scope

- TUI 内交互式 user-message 选择器（v1 用编号列表）
- fork 时分支摘要（docs/issues/17）
- --list-sessions-all 的跨 cwd 树

## Acceptance

- /fork 3 生成可 resume 的会话，内容为第 3 条消息之前的 entries，
  parentSessionPath 已设置
- /resume + /sessions 显示 fork 子会话嵌套在父会话下
