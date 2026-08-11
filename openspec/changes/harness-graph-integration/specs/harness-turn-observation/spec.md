## ADDED Requirements

### Requirement: TurnObserver — 一等 per-turn 观测通道

`AgentHarness` SHALL 提供 `turn_observer` 配置槽(类型 `Arc<dyn Fn(TurnObservation) + Send + Sync>`),在每个 prompt/continue 周期的每次 agent turn 完成后触发。`TurnObservation` SHALL 携带 `turn_index`(周期内 0-based)、本 turn 最终 assistant 文本、累计 input/output tokens。观测 SHALL 是被动的:不返回决策、不改变执行、不参与 continuation 计数,与 `OnTurnEndHook`(控制面)职责正交。

#### Scenario: 节点运行期实时同步

- **WHEN** runner 为 DAG 节点构造 harness 并配置 `turn_observer`,节点 prompt 产生多个 turn
- **THEN** observer 在每个 turn 完成后收到 `TurnObservation`(含累计 tokens),engine 的 idle watchdog 与实时 preview 由此刷新

#### Scenario: observer 与 hook 的触发顺序

- **WHEN** 同一 harness 同时配置 `turn_observer` 与 `on_turn_end`
- **THEN** observer 先于 hook 触发(观测先于决策),且 observer 的触发不影响 hook 的 `TurnEndAction` 决策

#### Scenario: 无 observer 时零开销

- **WHEN** harness 未配置 `turn_observer`
- **THEN** 每次 turn 完成不产生额外调用,行为与现状完全一致

### Requirement: prompt 系列返回 RunSummary

`prompt` / `prompt_with_images` / `continue_` / `prompt_from_template` SHALL 返回 `Result<RunSummary, AgentRunError>`;`RunSummary` SHALL 含最终 assistant 文本(`text`,无文本输出为空串)、累计 `input_tokens` / `output_tokens`、`interrupted`(turn 被中断提前结束)。**BREAKING**: 所有调用方适配新返回类型。

#### Scenario: 节点产出记录

- **WHEN** `run_agent` 调用 `sub.prompt(task)` 完成
- **THEN** 返回的 `RunSummary` 提供最终文本与 tokens,runner 无需订阅 `AgentEvent::MessageEnd` 或 post-hoc 读取 agent state 即可记录节点产出

#### Scenario: 中断的 run

- **WHEN** 节点被 cancel 级联导致 `TurnInterrupted`
- **THEN** `RunSummary.interrupted == true`,文本为中断前已产出的部分(若有),runner 据此标记节点状态

#### Scenario: 无文本输出

- **WHEN** 一次 run 完成但未产生任何 assistant 文本(如纯工具调用后被终止)
- **THEN** `RunSummary.text` 为空串,`interrupted` 反映真实终止原因
