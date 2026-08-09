# Session Resource Model

规范协议层 session 作为顶层资源的生命周期与 graph 挂载语义。

## ADDED Requirements

### Requirement: Session 生命周期 RPC

gRPC 服务 SHALL 提供 `ListSessions` / `CreateSession` / `SwitchSession` / `RenameSession` / `DeleteSession` 五个 RPC,HTTP 提供对应 `/sessions` 路由。`ListSessions` SHALL 返回全部会话摘要 (`SessionSummary`) 与当前连接绑定的 session id。`CreateSession` SHALL 创建新 JSONL 会话并将其设为当前。`SwitchSession` SHALL 将当前绑定切换到指定会话 (进程内 resume 语义)。`RenameSession` SHALL 写入会话 label。`DeleteSession` SHALL 删除会话及 sidecar 文件。

#### Scenario: 列出会话

WHEN 客户端调用 `ListSessions`
THEN 返回 `sessions` 数组 (每项含 session_id/name/cwd/model/created_at/last_activity_at/graph_count/active_graph_count/busy)
AND 返回 `current_session_id` 指向当前绑定会话

#### Scenario: 创建并切换

WHEN 客户端调用 `CreateSession{name?}`
THEN 新建会话并设为当前绑定
AND 后续 `GetState` 返回新会话的 `session_id`

#### Scenario: 切换会话

WHEN 客户端调用 `SwitchSession{session_id}`
THEN 当前绑定切换为目标会话 (进程内 resume,复用既有 transcript)
AND 目标不存在时返回错误,当前绑定不变

#### Scenario: 重命名会话

WHEN 客户端调用 `RenameSession{session_id, name}`
THEN 会话 label 更新,`ListSessions` 返回新 name

#### Scenario: 删除会话

WHEN 客户端调用 `DeleteSession{session_id}` 且该会话无运行中 graph
THEN 会话与其 JSONL/sidecar 文件被删除
AND 若删除的是当前绑定会话,当前绑定回退到最近会话 (或空)

### Requirement: Graph 挂载在 session 之下

`SessionState.dags` SHALL 只包含当前绑定 session 拥有的 DAG run;`SessionState.subagents` SHALL 只包含当前 session 的 job。`GraphList(session_id)` SHALL 返回指定 session 的全部 run。每个 `DagRun` SHALL 带有其属主 `session_id`。

#### Scenario: 状态快照按会话过滤

WHEN 引擎同时存在 session A 与 session B 的 run,当前绑定为 A
THEN `GetState` 返回的 `dags` 只含 A 的 run
AND `subagents` 只含 A 的 job

#### Scenario: 按会话列出 graph

WHEN 客户端调用 `GraphList{session_id: "B"}`
THEN 返回 B 的全部 run (含已完成的),不含其他 session 的 run

### Requirement: 事件面带 session 维度

`DagEvent` (node_status/run_status) SHALL 携带属主 `session_id`;`SubagentJob` SHALL 携带 `session_id` (注册时 stamp)。客户端可按会话过滤事件流。

#### Scenario: 事件带属主

WHEN DAG run 的节点状态变化产生 `DagEvent`
THEN 事件包含该 run 的 `session_id`
AND 订阅方可用它区分多会话事件

### Requirement: 删除保护

`DeleteSession` SHALL 拒绝删除仍有运行中 graph 的会话,错误信息 SHALL 列出运行中的 run id;调用方须先 `GraphCancel` 后再删。

#### Scenario: 拒绝删除活跃会话

WHEN 客户端删除一个含 running run 的会话
THEN 返回错误且会话保留
AND 错误包含运行中 run 的 id

### Requirement: 切换不中断进程级引擎

切换 session SHALL 只改变当前绑定 (对话/快照/命令的默认作用域),不停止进程级 DAG 引擎;其它 session 的运行中 graph 继续执行。

#### Scenario: 切换后他会话 graph 继续

WHEN 当前绑定 A 有 running run,客户端切换到 B
THEN A 的 run 继续执行
AND `GraphList{A}` 仍可见其状态更新
