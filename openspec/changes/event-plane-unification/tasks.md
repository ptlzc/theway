# Tasks: event-plane-unification

## 1. 事件重命名 — LoopEvent (core: run_loop 模块)

- [ ] 1.1 `crates/core/src/agent/run_loop/` 中重命名 `AgentEvent` → `LoopEvent`;变体 `AgentStart`→`RunStarted`、`AgentEnd`→`RunEnded`、`TurnEnd`→`TurnCompleted`;其余变体去 `Agent` 前缀。
- [ ] 1.2 `crates/core/src/agent/` 下全域适配: assembly.rs、cost.rs、goal.rs、所有 `AgentEvent::` 引用 → `LoopEvent::`。
- [ ] 1.3 `crates/server/src/` 下全域适配: runner.rs、registry、dag_tools、ui 等所有 `AgentEvent` 消费者 → `LoopEvent`。
- [ ] 1.4 `cargo check --workspace` 通过;grep 确认 `.rs` 文件中无 `AgentEvent` / `AgentStart` / `AgentEnd` / `TurnEnd` 残留(历史注释除外)。

## 2. 事件重命名 — SessionEvent (core: assembly 模块)

- [ ] 2.1 `crates/core/src/agent/assembly.rs` 中重命名 `HarnessEvent` → `SessionEvent`;变体 `SessionStart`→`Started`、`TurnEnded`→`TurnDecision`;其余变体名不变。
- [ ] 2.2 `crates/core/src/agent/` 下全域适配: 所有 `HarnessEvent::` 引用 → `SessionEvent::`、`SessionStart`→`Started`、`TurnEnded`→`TurnDecision`。
- [ ] 2.3 `crates/server/src/` 下全域适配: session_factory、trigger_engine、ui 等所有 `HarnessEvent` 消费者 → `SessionEvent`。
- [ ] 2.4 `cargo check --workspace` 通过;grep 确认 `.rs` 文件中无 `HarnessEvent` / `SessionStart` / `TurnEnded` 残留。

## 3. 分发机制 — broadcast channel + 同步回调 (core: run_loop)

- [ ] 3.1 `run_loop` 模块新增 broadcast Sender: `tokio::sync::broadcast::Sender<LoopEvent>` 容量 256;在 emit 点替换 for-await 循环为 `sender.send(event)`。
- [ ] 3.2 `run_loop` 模块新增同步回调注册: `Vec<Box<dyn Fn(&LoopEvent) + Send + Sync>>`,上限 3;每个回调 `catch_unwind` 包裹;emit 点同时触发同步回调与 broadcast send。
- [ ] 3.3 迁移现有订阅者: cost tracker → 同步回调;grpc streaming → broadcast Receiver;dag 面板 → broadcast Receiver。
- [ ] 3.4 移除旧 `AgentListener` for-await 循环与相关代码;模块文档更新为新分发架构说明。

## 4. 分发机制 — SessionEvent 同步改造 (core: assembly)

- [ ] 4.1 `assembly.rs` 新增 broadcast Sender: `tokio::sync::broadcast::Sender<SessionEvent>` 容量 128;替换 `HarnessListener` for-await 循环。
- [ ] 4.2 `assembly.rs` 新增同步回调注册(与 run_loop 一致架构);迁移审计日志等短操作。
- [ ] 4.3 移除旧 `HarnessListener` 分发代码;模块文档更新。

## 5. 事件面文档与验证

- [ ] 5.1 `assembly.rs` 模块文档: 三套事件面职责边界(LoopEvent/SessionEvent/AgentJobEvent)与消费方指南。
- [ ] 5.2 `run_loop/mod.rs` 模块文档: LoopEvent 职责说明 + broadcast/同步回调使用指南。
- [ ] 5.3 全局 grep 确认旧分发模式(for-await AgentListener / HarnessListener 循环)已移除。

## 6. 编译与测试验证

- [ ] 6.1 `cargo check --workspace` 通过(所有重命名适配完成)。
- [ ] 6.2 `cargo test --workspace --no-fail-fast` 全绿(现有测试需适配新类型名,无新增测试要求)。
- [ ] 6.3 `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- [ ] 6.4 `cargo fmt --all --check` 通过。
- [ ] 6.5 grep 确认无旧名残留: `AgentEvent` / `AgentStart` / `AgentEnd` / `TurnEnd` / `HarnessEvent` / `SessionStart` / `TurnEnded` 在 `.rs` 文件中均为零。
