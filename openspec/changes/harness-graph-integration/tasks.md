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

## 3. 成本分层 (P7) — core 中性统计, server 应用层能力

- [ ] 3.1 core 去美元化 — `core/src/agent/cost.rs`: `CostTracker`/`CostSnapshot` 移除 USD 聚合 (`total_cost()` 删除, `one_line_summary`/`full_breakdown` 去成本行), 只留 token 统计;修正注释 (不再声称 provider 填充 cost)。
- [ ] 3.2 core 去预算 — `assembly.rs`: 删 `AgentHarnessOptions::budget_cap_usd` 字段与 `check_budget_cap` 调用 (`prompt`/`continue_`/continuation 三处);`CostSnapshot` 引用方适配 (server 侧)。
- [ ] 3.3 llm-provider: 确认 `Usage::cost` 无填充承诺 (anthropic `update_usage` 保持只填 token);`ModelCost` 价格表保留;无 provider 改动。
- [ ] 3.4 server 成本模块 — 新建 `server/src/cost.rs`: `usd_cost(model, usage) -> f64` (读 theway-llm-provider 模型目录价格, tokens × 单价 / 1e6);`/cost` 命令、debug.rs (`usage.cost.total` 消费点)、UI 成本展示改用 server 换算。
- [ ] 3.5 server 预算工具 — run 级预算: 新工具/命令 (组合 `dag_status` 拿节点 token 统计 → server 换算 USD 累计 → 超限调 `dag_cancel`);文档注明 core 零预算概念。
- [ ] 3.6 测试: core 无 USD/预算残留 (grep `budget_cap_usd`/`cost.total` 为空);server `usd_cost` 单测 (1M input + 0.5M output 美元断言、零价格不 panic);`/cost` e2e 显示真实美元;预算工具超限中止测试。

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
