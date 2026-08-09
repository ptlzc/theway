# Consolidate Package

规范合并后 theway 包的 feature 矩阵、bin 组装与 workspace 成员。

## ADDED Requirements

### Requirement: Feature 矩阵

`theway` 包 SHALL 提供 `tui` 与 `server` 两个可选 feature,default SHALL 为 headless (两者都不开)。`server` feature SHALL 引入 axum/tonic/prost/tonic-prost/tokio-stream 依赖并启用 `transport` 模块 (gRPC/HTTP/WS + proto 编解码 + health)。`tui` feature SHALL 保持现有 TUI 栈。嵌入方可 `default-features = false` 只取核心。

#### Scenario: 默认构建

WHEN 执行 `cargo build --workspace` (无 features)
THEN theway lib 编译且不含 axum/tonic,不产出 `theway` bin

#### Scenario: 完整 CLI

WHEN 执行 `cargo build --workspace --features tui,server`
THEN lib + bin (`theway` 可执行文件) 均产出,`--web`/`--grpc`/TUI 三模式可用

### Requirement: Bin 组装

`theway` 包的 `[[bin]]` SHALL 名为 `theway`,声明 `required-features = ["tui", "server"]`。bin SHALL 通过包名 (`theway::`) 引用 lib 的公开面。

#### Scenario: bin 与 feature 绑定

WHEN 仅开启 `tui` 或仅开启 `server`
THEN bin 不构建 (required-features 不满足)
AND lib 仍可构建

### Requirement: Workspace 成员集合

workspace SHALL 只包含 `crates/llm-provider`、`crates/core`、`crates/app`、`crates/mcp` 四个成员。`theway-server` 与 `theway-cli` 的职责 SHALL 由 `theway` 包内的 `transport` 模块 (server feature) 与 `[[bin]]` 承担。

#### Scenario: 成员校验

WHEN 检查根 `Cargo.toml` members 与 `crates/` 目录
THEN 仅含四个 crate,无 `crates/server`、`crates/cli` 目录

### Requirement: 依赖方向不回归

合并后 SHALL 保持 `server → app` 的依赖逻辑:transport 模块 (feature 内) 只通过公开面 (`TransportEndpoints`/wire/`SessionOps`) 与核心交互,不访问核心私有实现。

#### Scenario: 模块边界

WHEN 审查 `transport` 模块对核心的引用
THEN 只使用公开类型与通道 (TransportEndpoints 的发送端/共享状态)
AND 不访问 `App` 的私有字段 (同 crate 内也不越界)
