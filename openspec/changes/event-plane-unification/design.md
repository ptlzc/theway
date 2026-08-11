# Design: event-plane-unification

## Context

theway 的事件面由三套类型组成: `AgentEvent`(20 变体,在 `run_loop/` 模块发射)、`HarnessEvent`(6 变体,在 `assembly.rs` 发射)、`AgentJobEvent`(4 变体,在 `registry/events.rs` 发射)。命名与分发机制存在两个根本问题:

**命名问题**: `AgentEvent` 的 20 个变体中 16 个是 `run_loop` 机制的内部事件(AgentStart/TurnStart/ModelRequest/ToolExecution 等),但类型名"AgentEvent"被外部观测者视为通用 agent 事件,导致误订阅;`HarnessEvent` 的名字暴露了实现细节("Harness"是组装层内部概念),而其真实语义是会话生命周期;更严重的是 `HarnessEvent::TurnEnded` 与 `AgentEvent::TurnEnd` 仅一字之差但语义完全不同 — 前者是决策/审计事件(TurnDecision),后者是数据事件(TurnCompleted)。`AgentJobEvent` 命名准确(高频作业面),保留。

**分发机制问题**: `AgentListener`(run_loop 内)通过 `for-await` 同步分发 — emit 点 `await` 每个订阅者的处理完成,慢消费者直接阻塞事件循环;且无 panic 隔离(`HarnessListener` 有 `catch_unwind` 而 `AgentListener` 没有,不一致)。trait 观察者方案(runtime 动态 add/remove)被评估为过度设计 — 当前规模小(订阅者≤3),不值得引入 trait 对象开销。

评估结论来源: 两个独立 subagent 对事件面做了完整分析,本设计直接采用其结论。

## Goals / Non-Goals

**Goals:**

- 重命名 `AgentEvent` → `LoopEvent`、`HarnessEvent` → `SessionEvent`,变体名同步对齐(见 D1),消除歧义与实现细节泄露。
- 替换 `AgentListener` 的纯同步 for-await 分发为 broadcast channel 主路径 + 轻量同步回调辅路径(方案 D),覆盖 panic 隔离与慢消费者保护。
- 保持 `AgentJobEvent` 不变。
- 文档化三套事件面的职责边界与消费方指南。

**Non-Goals:**

- 不改变事件携带的数据字段(仅改名,不增减字段)。
- 不改变 `AgentJobEvent` 的分发机制(保持 registry 现有实现)。
- 不合并事件类型(三套面职责不同,不强行统一)。
- 不引入 trait 对象观察者模式。

## Decisions

### D1: 事件类型重命名 (命名契约)

| 旧名 | 新名 | 理由 |
|------|------|------|
| `AgentEvent` | `LoopEvent` | 20 变体中 16 个是 run_loop 机制的数据事件;新名反映职责域 |
| `AgentEvent::AgentStart` | `LoopEvent::RunStarted` | "Agent"冗余;`Run` 是 run_loop 的自然周期 |
| `AgentEvent::AgentEnd` | `LoopEvent::RunEnded` | 同上 |
| `AgentEvent::TurnEnd` | `LoopEvent::TurnCompleted` | "Completed"明确是数据完成事件,与 `TurnDecision` 区分 |
| `HarnessEvent` | `SessionEvent` | "Harness"是实现细节;实际语义是会话生命周期 |
| `HarnessEvent::SessionStart` | `SessionEvent::Started` | 类型名已带 `Session`,变体去冗余 |
| `HarnessEvent::TurnEnded` | `SessionEvent::TurnDecision` | 核心修正:此事件承载决策/审计语义(与 `TurnCompleted` 一字之差是根本缺陷) |
| `AgentJobEvent` | (保留) | 命名准确,职责明确(高频作业面:Started/Output/Metrics/Completed) |

其余变体遵循 `PascalCase` 规范,保持与现有 `AgentEvent` 变体名的结构对应(如 `LoopEvent::TurnStart`、`LoopEvent::ModelRequest` 等),仅去掉 `Agent` 前缀。

**为什么不用更长的描述性名字** (如 `RunLoopInternalEvent`): `LoopEvent` 简洁且在 `run_loop` 模块上下文中含义自明;`SessionEvent` 与会话生命周期语义完全对应。导出路径 `theway_core::agent::run_loop::LoopEvent` 自带命名空间消歧义。

**BREAKING**: 所有 `use theway_core::agent::AgentEvent` / `AgentEvent::` 引用需改为 `LoopEvent::`;所有 `HarnessEvent::` 引用需改为 `SessionEvent::`。影响范围: `crates/core/src/agent/` 下约 20+ 处、`crates/server/src/` 下约 15+ 处(grep 确认)。

### D2: 分发机制 — broadcast channel 主路径 + 同步回调辅路径 (方案 D)

**架构**:

```
emit 点 (run_loop / assembly)
    │
    ├── 同步回调路径 (轻量, 无阻塞)
    │   └── Vec<Box<dyn Fn(&LoopEvent)>>
    │       例如: cost tracker (原子计数, <1μs)
    │
    └── broadcast 路径 (异步, 有界通道)
        └── tokio::sync::broadcast::Sender<LoopEvent>
            ├── Receiver A: grpc streaming (外部订阅者)
            ├── Receiver B: 持久化/审计日志
            └── Receiver C: dag-orchestrator 状态面板
```

**两条路径的职责边界**:

| 维度 | 同步回调 | broadcast 通道 |
|------|---------|---------------|
| 延迟要求 | <1μs | 允许毫秒级 |
| panic 容忍 | `catch_unwind` 逐回调隔离 | Receiver drop 不影响 Sender |
| 慢消费者 | 禁止(开发者约束,文档注明) | 有界通道 + lag 检测,Sender 永不阻塞 |
| 订阅数量 | ≤3 (硬约束) | 无上限 |
| 注册方式 | `register_sync_callback(fn)` | `subscribe() -> Receiver` |

**有界通道容量**: `broadcast::channel(256)`,溢出时最旧消息被丢弃(lagging receiver 收到 `Lagged(n)` 错误,自行决定 resubscribe 或跳过)。256 足够容纳正常 burst(TurnStart→ModelRequest→...→TurnCompleted 单周期 ≤20 事件 × 10 节点 ≤200),且不会在慢消费者时堆积内存。

**为什么不用 `tokio::sync::mpsc`**: broadcast 支持多消费者每人独立接收(不消费掉消息),grpc/持久化/面板三者各自持有 Receiver 互不干扰;mpsc 是单消费者,需要转发层。

**为什么不用 trait 观察者**: 运行时 add/remove 在当前规模(≤3 订阅者)下收益为零,引入 trait 对象增加间接调用开销与生命周期复杂度。同步回调用 `Box<dyn Fn>` 已足够,类型擦除成本可接受。

### D3: Panic 隔离标准化

当前不一致: `run_loop.rs` 的 `AgentListener` 分发无 `catch_unwind`,而 `assembly.rs` 的 `HarnessListener` 有。D2 统一:

- **同步回调路径**: 每个回调独立 `std::panic::catch_unwind` 包裹,一个回调 panic 不影响其他回调或 emit 点。
- **broadcast 路径**: `tokio::sync::broadcast` 天然隔离 — Receiver 端的 panic 只影响该 task。

### D4: 迁移策略 — 渐进式,不拆两套

事件重命名(D1)与分发机制(D2)在同一个 PR 内完成,不拆两套:

1. 先做类型重命名(纯搜索替换,机械操作),`cargo check` 确认编译通过。
2. 再做分发机制替换(新增 broadcast sender + 同步回调注册,替换 for-await 循环)。
3. 全局 grep 确认无旧类型名残留。

**为什么不分步**: 分发机制替换需要理解每个订阅者的语义(cost tracker 走同步?grpc 走 broadcast?),这与新类型名(`LoopEvent`/`SessionEvent`)一起理解最准确;分步会导致中间态(旧分发+新命名 或 旧命名+新分发)的适配成本更高。

## Risks / Trade-offs

- [BREAKING 全域重命名] 改名触及所有消费者 → grep+sed 机械替换 + `cargo check --workspace` 兜底;core 与 server 的所有测试覆盖改名路径。
- [broadcast 通道 lag] 慢消费者导致消息丢失 → `Lagged(n)` 错误传递给 Receiver,文档注明消费者职责(处理 Lagged 或提升消费速度);256 容量在正常负载下不丢消息。
- [同步回调阻塞] 开发者误将耗时操作注册为同步回调 → 文档硬约束(同步回调 MUST <1μs) + code review 把关;不做运行时超时检测(过度设计)。
- [TurnCompleted vs TurnDecision 混淆期] 改名后旧代码可能残留旧名 → grep 复核 + CI clippy 覆盖,确保零残留。
- [AgentJobEvent 保持同步分发] `AgentJobEvent` 的 registry 内部分发(metrics_listener 等)本轮不改 → 理由:registry 的 emit 已在 `tokio::spawn` 内,不阻塞调用方;等下一轮统一。

## Migration Plan

1. **Step 1: 重命名** — 全局替换 `AgentEvent`→`LoopEvent`、`AgentStart`→`RunStarted`、`AgentEnd`→`RunEnded`、`TurnEnd`→`TurnCompleted`(run_loop 模块内);`HarnessEvent`→`SessionEvent`、`SessionStart`→`Started`、`TurnEnded`→`TurnDecision`(assembly 模块内);`cargo check --workspace` 确认编译;分笔提交。
2. **Step 2: 分发机制** — `run_loop` 新增 broadcast sender + 同步回调注册;替换 `AgentListener` 的 for-await 循环;迁移现有订阅者(cost tracker→同步回调,grpc→broadcast receiver,dag 面板→broadcast receiver);`assembly` 侧 `HarnessListener` 保持一致;`cargo test --workspace --no-fail-fast` 全绿;分笔提交。
3. **Step 3: 验证** — grep 确认无 `AgentEvent` / `HarnessEvent` 旧名残留(除注释中的 historical note);clippy + fmt 全绿。
4. **回滚**: 每步独立 commit,可逐 commit revert。

## Open Questions

- `AgentJobEvent` 的分发机制是否需要在下一轮统一为 broadcast 通道?(默认是,本轮仅文档标记为后续演进)
- 同步回调硬约束值: 初定 50µs 在 benchmark 落地后(实测 52.75ns/3回调)被修正为 **单回调 <1µs、路径合计 ≤5µs**(实测值 ~60 倍余量,恰好挡住系统调用/等锁;`emit_sync_only` 均值若超 1µs 即触发架构审查)。
- ~~`SessionEvent::TurnDecision` 是否需要在 `dag_inspect` 中展示?(默认本期不做,留待 UI 需求确认)~~ — 已撤销:该条系撰写时误挂,`TurnDecision` 是会话层回合决策审计,与 `dag_inspect`(DAG 节点视角)无直接关系;且 server `ui/listener.rs` 已在消费该事件,无悬而未决的 UI 需求。
