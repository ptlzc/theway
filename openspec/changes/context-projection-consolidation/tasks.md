## 1. Core：custom role 物化

- [ ] 1.1 在 `crates/theway-core/src/types.rs` 的 `default_convert_to_llm` 中物化 `compaction_summary`、`branch_summary`、`collapse_context`，未知 role 继续过滤。
- [ ] 1.2 为 `default_convert_to_llm` 增加单测：三种已知角色生成带标记的 provider 消息，未知角色被过滤。
- [ ] 1.3 更新 `crates/theway-core/src/agent/context/assembly.rs`，将 compact_context 投影改为 `AgentMessage::Custom(role="collapse_context")`。
- [ ] 1.4 更新 core session 测试断言：`build_context()` 中的 collapse 摘要从 `Llm(User)` 改为 `Custom(role="collapse_context")`，并验证经 `convert_to_llm` 后进入 provider 消息。

## 2. Daemon：ContextService / ContextBundle

- [ ] 2.1 新增 `crates/theway-daemon/src/context/service.rs`：`ContextBundle` 与 `ContextService::load(session)`。
- [ ] 2.2 `ContextService::load` 内复用 `session.build_context()` 与 `compose_system_prompt`，不再让 orchestration 自行拼装。
- [ ] 2.3 精简 `crates/theway-daemon/src/context/lineage.rs`：移除 `Previous context summary`，只保留 collapse node / source session 与工具指引。
- [ ] 2.4 更新 `crates/theway-daemon/src/orchestration/session.rs` 只调用 `ContextService::load`。
- [ ] 2.5 更新 daemon context 测试：system prompt 不含 compactText；`ContextBundle.messages` 经 `convert_to_llm` 后包含摘要一次。

## 3. 集成验证与提交

- [ ] 3.1 运行 `cargo test -p theway-core -p theway-daemon` 相关测试。
- [ ] 3.2 运行 `cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check`。
- [ ] 3.3 按 crate 小步提交，Conventional Commits 引用 issue #54；推送前 `git fetch + rebase origin/main`。
