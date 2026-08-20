# theway-contract

[English](README.md) | 中文

`theway-contract` 是工作区的叶子契约 crate，存放需要跨越运行时、持久化与协议实现共享、但不能反向依赖这些实现的数据。它定义可序列化记录、存储 trait、自动化 sidecar 模型、会话标识以及 `~/.theway` 路径布局，不包含 agent 引擎、数据库后端或网络传输。

## 公开模块

| 模块 | 职责 |
|---|---|
| [`config`](src/config.rs) | 解析基础目录，并为各工作目录派生稳定路径。 |
| [`session`](src/session.rs) | 定义原始会话存储记录，以及异步 `SessionReader`、`SessionStore` trait。 |
| [`session_id`](src/session_id.rs) | 校验并规范化持久化会话标识。 |
| [`dag`](src/dag.rs) | 定义持久化 DAG 运行与节点快照及其状态文件路径。 |
| [`triggers`](src/triggers.rs) | 定义会话级动态 trigger 与 cron sidecar 记录。 |
| [`extension`](src/extension/mod.rs) | 定义 runtime extension ABI v2 manifest、生命周期与 action envelope、持久化条目、信任记录、诊断及客户端中立 contribution。 |

`theway-core` 负责在带类型的运行时会话条目和这些原始记录之间转换。`theway-storage` 实现持久化 trait；`theway-transport` 复用或重新导出适合位于该叶子层的客户端可见数据。

[`sdk/extension-abi-v2`](sdk/extension-abi-v2) 下签入的 TypeScript 声明和 JSON Schema 由 Rust extension 契约生成。使用 `cargo run -p theway-contract --example generate_extension_artifacts -- crates/theway-contract/sdk/extension-abi-v2` 重新生成；extension 契约测试会在临时目录中重新生成并拒绝漂移。

## 文档

- [架构与不变量](docs/architecture.md)

## 验证

```bash
cargo test -p theway-contract
cargo doc -p theway-contract --no-deps
```
