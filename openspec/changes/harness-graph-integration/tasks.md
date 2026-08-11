# Tasks: harness-graph-integration

## 1. TurnObserver + RunSummary (P2/P1 残余/P4 部分, 核心)

- [ ] 1.1 `assembly.rs`: 新增 `TurnObservation` 类型 (turn_index / text / input_tokens / output_tokens) 与 `TurnObserver` 类型别名;`AgentHarnessOptions` 增加 `turn_observer: Option<TurnObserver>` 字段 + `new()` 默认 None。
- [ ] 1.2 `assembly.rs` `run_turn_with_continuation`: 每次 agent turn 完成后 (`finish_persisted_run` 之后、hook 之前) 触发 `turn_observer`;文档注明 observer 先于 hook 触发。
- [ ] 1.3 `assembly.rs`: 新增 `RunSummary` (text / input_tokens / output_tokens / interrupted);`prompt` / `prompt_with_images` / `continue_` / `prompt_from_template` 返回 `Result<RunSummary, AgentRunError>`;text 取自本周期最后一次 assistant 消息。
- [ ] 1.4 `multiagent/runner.rs`: 删除 final_text 收集器 (Arc<Mutex<String>> + MessageEnd 订阅) 与 post-hoc `sub.cost()` hack;改用 `harness_opts.turn_observer` (每 turn 回调给 engine) + `sub.prompt()` 的 `RunSummary` (记录节点产出)。
- [ ] 1.5 server 调用方适配: grep `\.prompt(` / `\.continue_(` / `prompt_from_template` (session_factory / trigger_engine / ui / commands) 适配 `RunSummary` 返回 (忽略或取 text)。
- [ ] 1.6 测试: core 新增 TurnObserver 触发顺序/次数测试 (tests/harness_e2e/ 或 run_loop 套件);runner 相关测试适配;`cargo test -p theway-core --no-fail-fast` 全绿。

## 2. graph 身份 + ephemeral session (P5)

- [ ] 2.1 `assembly.rs`: `AgentHarnessOptions` 增加 `run_id` / `node_id: Option<String>`;`HarnessEvent` 全部变体增加可选 `run_id` / `node_id` 字段;`emit_harness_event` 填充。
- [ ] 2.2 `assembly.rs`: `session` 字段改 `Option<Session>`;新增 `AgentHarnessOptions::ephemeral(model)`;`new(model, session)` 保留兼容 (Some 包装)。
- [ ] 2.3 `assembly.rs`: `set_model` / `set_thinking_level` 在无 session 时仅改 agent state;`move_to` / `rehydrate_from_session` / `reload_skills_from_disk` 无 session 时返回类型化错误 (新增 `SessionError::NoSession` 或复用现有错误)。
- [ ] 2.4 `runner.rs`: 节点 harness 用 `ephemeral` 构造 + 传 `run_id` / `node_id`;registry 的 session_id 元数据保留 (现有逻辑)。
- [ ] 2.5 事件消费者适配 (server ui/listener、goal.rs、harness_e2e 测试) + 新增 ephemeral 行为测试 (无 session 时 move_to 报错、set_model 不写审计)。

## 3. run 级成本 (P7)

- [ ] 3.1 registry: job 记录增加 `cost_usd: Option<f64>`;`AgentJobEvent::Completed` / Metrics 增加 `cost_usd`;`metrics_listener` 或 runner 在 finish 时从 `sub.cost()` 快照写入。
- [ ] 3.2 engine: run 记录 (graph/types.rs 运行时状态) 增加 `run_cost_usd: Option<f64>`;`on_node_completed` 时累计节点成本。
- [ ] 3.3 `AgentRunOptions` 增加 `budget_cap_usd: Option<f64>` (run 级);engine 调度点在节点完成时检查累计成本,超限 → 后续节点不启动、运行中节点完成、run 标记预算受限 (DagStatus 复用或新增状态)。
- [ ] 3.4 展示: dag_tools 渲染 (dag_inspect/status) 与 server UI 展示 run 总成本 / 单节点成本 (若有 UI 消费点)。
- [ ] 3.5 测试: 多节点 run 成本累计、预算超限中止后续节点 (engine 测试套件);单会话 per-harness budget 行为不变测试。

## 4. 事件面 + 执行模型文档 (P6/P3)

- [ ] 4.1 `assembly.rs` 模块文档: 四套事件面归属 (HarnessEvent / AgentEvent / AgentJobEvent / engine 状态) 与消费方;TurnObserver 为外部 per-turn 观测首选。
- [ ] 4.2 `OnTurnEndHook` doc: 内联控制面边界 + 演进路径 (`OnRunEndEvent`),本轮不改 hook 行为。
- [ ] 4.3 `multiagent/mod.rs` 文档: 补充事件面职责与"编排驱动,输出是记录"的定位。
- [ ] 4.4 (可选) HarnessEvent → AgentJobEvent 桥接 (节点生命周期事件进 registry 作业面);默认跳过,仅在 UI 需求确认后做。

## 5. 验证与收尾

- [ ] 5.1 `cargo test --workspace --no-fail-fast` 全绿;`cargo clippy --workspace --all-targets -- -D warnings`;`cargo fmt --all --check`。
- [ ] 5.2 `cargo check -p theway-core --no-default-features` 通过 (feature 门控未破坏)。
- [ ] 5.3 结构校验: grep 确认 runner.rs 无 `AgentEvent::MessageEnd` 裸订阅 (除 registry metrics_listener 内部);grep 确认 `final_text` 收集器已删除。
- [ ] 5.4 分笔提交并推送 (core 改动 / server 适配 / 文档 各一笔,Conventional Commits)。
