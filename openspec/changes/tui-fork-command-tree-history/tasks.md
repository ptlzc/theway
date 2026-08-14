# Tasks: tui-fork-command-tree-history

Issue: #29. 单会话实现（存储 + daemon 命令 + TUI/CLI 显示三层），
每步小 commit（Conventional Commits，引用 #29）。

graph TD
  1-storage --> 2-fork-command
  2-fork-command --> 3-tree-display
  3-tree-display --> 4-verify

## 1-storage [executor] — 存储层

- [x] 1.1 SessionEntry.parent_id + list_entries 解析 parentSessionPath（path→id 映射）
- [x] 1.2 fork_session()：新 db 重放 path-to-root entries + set_parent_session_path + build_context 校验
- [x] 1.3 flatten_session_tree()：pi /tree 风格前缀（每层祖先延续列 + ├─/└─），环保护
- [x] 1.4 find_session_path 快路径（stem 匹配先于开库，避开 live daemon 锁）
- [x] 1.5 list_entries 对锁定的 live session 降级（stem id，不硬失败）

## 2-fork-command [executor] [depends: 1-storage] — daemon /fork

- [x] 2.1 ForkCommand：无参列出 user messages（新→旧编号）
- [x] 2.2 /fork <n> → ForkPosition::Before → fork_session → resume 提示
- [x] 2.3 注册进 with_daemon_commands

## 3-tree-display [executor] [depends: 2-fork-command] — 树形显示

- [x] 3.1 /sessions（daemon auth.rs）树形 + (empty) 占位
- [x] 3.2 CLI --list-sessions 树形
- [x] 3.3 /resume picker：时间序树行 + prefix 渲染（PickerRow.prefix）

## 4-verify [checker] [depends: 3-tree-display] — 验证

- [x] 4.1 cargo test（storage 30 / tui 57 / daemon commands）全绿，clippy 0 警告
- [x] 4.2 PTY e2e：/fork 列表+创建、resume fork、fork-of-fork、
      /sessions / --list-sessions / --resume picker 树形（3 代嵌套）
- [x] 4.3 push + close #29
