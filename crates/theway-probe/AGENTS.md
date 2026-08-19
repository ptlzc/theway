# theway-probe 修改规则

本文件适用于 `crates/theway-probe/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改协议覆盖前，先阅读 [crate 概览](README.md)和[probe 架构](docs/architecture.md)。

## 边界规则

- Probe 保持为独立外部 gRPC 客户端，不依赖 `theway-daemon`、`theway-core` 或 Rust `theway-transport` crate。
- 从 [`../theway-transport/proto/health.proto`](../theway-transport/proto/health.proto) 所在的 transport 定义编译 protobuf 客户端，不在这里复制协议文件。
- 检查必须确定、无需密钥，并能安全面对操作方提供的 daemon 端点。
- 未明确扩展范围时，不在可服务性检查中添加 daemon 进程启停、文件系统修改或 LLM 调用。

## 输出规则

- 每个选中检查返回一个 `TestResult`，未知名称明确失败。
- Stdout 对人类可读，可选 JSON 结果文件对自动化保持稳定。
- 任一选中检查失败时保持非零退出状态。

## 测试与文档

- 添加或删除检查，或修改 CLI/输出行为时，更新 [`docs/architecture.md`](docs/architecture.md) 和 [`README.md`](README.md)。
- Transport proto 变化影响导入服务后重新构建 probe。
- 运行 `cargo check -p theway-probe` 和 `cargo doc -p theway-probe --no-deps --document-private-items`；行为变化时还要针对本地测试 daemon 执行受影响检查。
