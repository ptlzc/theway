## 1. Core：rolling summary helper

- [x] 1.1 在 `crates/theway-core/src/agent/session/session.rs` 新增 `Session::latest_collapse_summary()`：优先最新 `Compaction` summary，回退到最新 `compact_context.compactText`。
- [x] 1.2 为 `latest_collapse_summary` 增加单测：普通 compaction、collapse child、嵌套 collapse child、两者都有时的优先级。

## 2. Storage：session graph 父子链

- [x] 2.1 在 `crates/theway-storage/src/session_graph.rs` 增加 `link_child(parent_id, child_id)` 或等价的原子边更新接口。
- [x] 2.2 `save_node` / `link_child` 保证 node 与其 parent/child 边在同一事务中提交。
- [x] 2.3 增加嵌套链持久化测试：node2.parent_id = node1；node1.child_ids 包含 node2；重启后仍可读取。

## 3. Daemon：嵌套坍缩与事件型 lineage

- [x] 3.1 `collapse_session_inner` 的 compact_text 来源改为 `request.summary` > `latest_collapse_summary` > 当前 transcript。
- [x] 3.2 在 `make_collapse_node` 中写入 `parent_id`（源 session 的 `collapseNodeId`）并更新父节点 `child_ids`。
- [x] 3.3 嵌套坍缩时生成固定五组件 rolling summary（goal / completed work / key decisions / next steps / critical context），并保持有界。
- [x] 3.4 `render_lineage` 保持只输出 Collapse event 的 node id 与 source session id。
- [x] 3.5 增加嵌套坍缩集成测试：S → C1 → C2 后 node 链可遍历，C2 的 compact_context 包含上一代摘要输入，lineage 不含 compactText。

## 4. Prompt / docs / 验证

- [x] 4.1 更新 `AGENTS.md` 与 harness prompt，确认嵌套坍缩、固定五组件摘要、事件型 lineage 的描述与实现一致。
- [x] 4.2 运行 `cargo test -p theway-core -p theway-storage -p theway-daemon`。
- [x] 4.3 运行 `cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check`。
- [x] 4.4 按 crate 小步提交，Conventional Commits 引用 issue #55；推送前 `git fetch + rebase origin/main`。
