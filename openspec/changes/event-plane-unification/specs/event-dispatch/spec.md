## ADDED Requirements

### Requirement: broadcast channel 主分发路径

系统 SHALL 在 `run_loop` 模块中提供 `tokio::sync::broadcast::Sender<LoopEvent>` 作为 LoopEvent 的主分发路径。通道容量 MUST 为 256。Sender SHALL NOT 因慢消费者而阻塞 — 当 Receiver 落后超过容量时,Sender 继续发送,落后 Receiver 收到 `Lagged(n)` 错误。

#### Scenario: 多消费者独立接收

- **WHEN** 三个独立 Receiver 分别用于 grpc streaming、持久化日志、dag 状态面板
- **THEN** 每个 Receiver 独立接收所有 LoopEvent 消息,互不影响;任一 Receiver drop 不影响其他消费者与 Sender

#### Scenario: 慢消费者 lag 保护

- **WHEN** 某 Receiver 消费速度低于事件产生速度,落后超过 256 条
- **THEN** 该 Receiver 收到 `tokio::sync::broadcast::error::RecvError::Lagged(n)`,其余 Receiver 不受影响;Sender 不阻塞

#### Scenario: 正常负载无消息丢失

- **WHEN** 所有 Receiver 保持正常消费速度,单周期事件量 ≤200
- **THEN** 容量 256 足以容纳 burst,无 Lagged 错误,无消息丢失

### Requirement: 同步回调辅分发路径

系统 SHALL 在 `run_loop` 模块中提供同步回调注册机制,接受 `Box<dyn Fn(&LoopEvent) + Send + Sync>` 类型的轻量回调,注册数量 MUST NOT 超过 3 个。每个回调 MUST 在独立 `std::panic::catch_unwind` 包裹下执行,单个回调 panic SHALL NOT 影响其他回调或 emit 点。

#### Scenario: 轻量操作使用同步回调

- **WHEN** cost tracker(原子计数,<1μs)注册为同步回调
- **THEN** 每次 LoopEvent 发出时 cost tracker 同步执行,不被 broadcast 通道延迟影响

#### Scenario: panic 隔离

- **WHEN** 某个同步回调 panic
- **THEN** panic 被 `catch_unwind` 捕获,不影响其他回调执行;emit 点继续正常分发

#### Scenario: 注册超限拒绝

- **WHEN** 尝试注册第 4 个同步回调
- **THEN** 系统返回错误或 panic(文档注明硬约束:≤3),防止过度注册导致 emit 点延迟

### Requirement: SessionEvent 分发机制一致

系统 SHALL 为 `SessionEvent` 提供与 `LoopEvent` 相同的 broadcast channel + 同步回调辅路径分发架构。通道容量 MUST 为 128(SessionEvent 事件频率远低于 LoopEvent)。`assembly.rs` 的现有 `HarnessListener` for-await 循环 SHALL 被替换为新架构。

#### Scenario: SessionEvent broadcast 分发

- **WHEN** 会话发生 Started/Compaction/TurnDecision 事件
- **THEN** 事件经 broadcast Sender 发送,所有 Receiver 独立接收

#### Scenario: SessionEvent 同步回调路径

- **WHEN** 审计日志组件需要同步捕获每次 TurnDecision
- **THEN** 审计组件注册为同步回调,在事件 emit 时立即执行

### Requirement: Panic 隔离标准化

系统 SHALL 确保 LoopEvent 与 SessionEvent 的所有分发路径(同步回调 + broadcast)均有 panic 隔离。同步回调路径每个回调独立 `catch_unwind`;broadcast 路径各 Receiver 运行在独立 tokio task 中,panic 限制在 task 边界内。emit 点 SHALL NOT 因任何消费者的 panic 而崩溃。

#### Scenario: broadcast Receiver panic 不影响 emit 点

- **WHEN** grpc streaming Receiver 的 tokio task 因内部错误 panic
- **THEN** emit 点(在 run_loop 主 task 中)不受影响,其他 Receiver 继续接收事件

#### Scenario: 同步回调 + broadcast 并发 panic 隔离

- **WHEN** 一个同步回调 panic 且同一时刻一个 broadcast Receiver 的 task panic
- **THEN** 两个 panic 各自隔离;emit 点正常运行;无连锁崩溃

### Requirement: 旧分发机制移除

系统 SHALL 移除 `run_loop` 模块中 `AgentListener` 的纯 for-await 同步分发循环。所有原 `AgentListener` 订阅者 MUST 迁移至 broadcast channel(长链路)或同步回调(短操作)路径。代码中 SHALL NOT 存在 for-await 循环等待消费者逐个处理事件的模式。

#### Scenario: 旧分发代码不存在

- **WHEN** grep `for-await` 或 `AgentListener` 相关分发逻辑
- **THEN** run_loop 模块中无逐消费者 await 的同步分发循环

#### Scenario: 现有订阅者迁移完成

- **WHEN** cost tracker、grpc streaming、dag 面板等原有订阅者编译通过
- **THEN** 所有订阅者使用新分发路径(broadcast 或同步回调),功能与迁移前一致
