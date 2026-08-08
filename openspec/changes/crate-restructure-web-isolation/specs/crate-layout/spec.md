# Crate Layout

规范 theway workspace 的 canonical crate 布局、模块分层与命名边界。

## ADDED Requirements

### Requirement: Workspace 由固定集合的 crate 组成

workspace 的 crate 集合与物理目录 SHALL 为:`crates/llm-provider` (`theway-llm-provider`)、`crates/core` (`theway-core`)、`crates/mcp` (`theway-mcp`)、`crates/app` (包名 `theway`)、`crates/server` (`theway-server`)、`crates/cli` (`theway-cli`)。目录名 SHALL 反映 crate 语义,不沿用历史名称。

#### Scenario: 目录名与包语义一致

WHEN 开发者浏览 `crates/` 目录
THEN 每个子目录名对应其 crate 的实际职责 (llm-provider=协议适配 / core=引擎 / mcp=MCP 客户端 / app=应用层 / server=协议层 / cli=命令行入口)
AND 不存在名为 `harness` 的 crate 目录

#### Scenario: 包名保持兼容

WHEN 外部项目通过 `use theway::...` 嵌入应用层
THEN 包名 `theway` 保持不变,不因目录改名为 `crates/app` 而破坏

### Requirement: core 内模块分层与命名

`theway-core` 内部 SHALL 按语义分层组织模块:裸 Agent 循环内核 (`agent`/`agent_loop`)、运行时层 (`runtime`)、基础设施 (`types`/`node`/`proxy`)。`runtime` 模块 SHALL 承载 AgentHarness、会话存储、压缩、技能加载、触发器、图编排 (DAG 引擎) 与子代理注册表。core 内 MUST NOT 存在名为 `harness` 的模块路径。

#### Scenario: 运行时层路径

WHEN 代码引用引擎的运行时层
THEN 使用路径 `theway_core::runtime::...` (例如 `theway_core::runtime::graph_engineering::engine::DagEngine`)
AND `theway_core::harness::...` 路径 MUST NOT 被任何代码引用

#### Scenario: 裸 Agent 与运行时分离

WHEN 嵌入方只需要无状态 agent 循环内核
THEN 可以通过关闭 `harness` feature 仅依赖 `agent`/`agent_loop` 模块,不引入 `runtime` 层依赖

### Requirement: harness 术语边界

术语 "harness" 在代码库中 SHALL 仅作为 `AgentHarness` 类型名存在。模块名、crate 名、目录名 MUST NOT 使用 "harness"。

#### Scenario: 术语唯一性

WHEN 在 workspace 中搜索标识符 `harness` (模块路径 / crate 名 / 目录名)
THEN 结果仅包含 `AgentHarness` 类型及相关派生标识符 (如 `AgentHarnessOptions`)
AND 不包含 `harness` 模块路径或 `crates/harness` 目录

### Requirement: 单向依赖约束

crate 依赖关系 SHALL 严格单向:`theway-cli` → `theway-server` → `theway` → `theway-core` → `theway-llm-provider`,以及 `theway` → `theway-mcp`。任何 crate MUST NOT 依赖其下游 crate,workspace 中 MUST NOT 存在包级依赖环。

#### Scenario: 依赖方向检查

WHEN 对 workspace 执行 `cargo metadata` 并构造依赖图
THEN 依赖边只沿上述方向存在,图中无环
AND `theway-server` 不依赖 `theway-cli`,`theway` 不依赖 `theway-server`(库层)

### Requirement: 协议归属

wire 模型类型 (`WebStatus`/`WebCommand`) SHALL 定义在 `theway` (应用层),因为 `App` 的快照构建依赖它们;proto 文件、protobuf 编解码 (`proto.rs`) 与传输服务器实现 SHALL 位于 `theway-server`。`theway` 库层 MUST NOT 依赖 axum/tonic 等服务器框架依赖。

#### Scenario: 服务器框架依赖位置

WHEN 检查 `theway` crate 的 Cargo.toml
THEN 不包含 axum、tonic、prost、tokio-tungstenite 等服务器依赖
AND 这些依赖位于 `theway-server` 的 Cargo.toml

#### Scenario: 服务器只对公开接口编程

WHEN `theway-server` 实现 HTTP/gRPC/WS 端点
THEN 只通过 `theway` 公开的 `AppHandle` 接口 (snapshot/command/events) 访问运行时状态
AND 不访问 `App` 的私有或 `pub(crate)` 字段
