# theway-contract

`theway-contract` 是工作区的叶子契约 crate，存放需要跨越运行时、持久化与协议实现共享、但不能反向依赖这些实现的数据。它定义可序列化记录、存储 trait、自动化 sidecar 模型、会话标识以及 `~/.theway` 路径布局，不包含 agent 引擎、数据库后端或网络传输。

## 公开模块

| 模块 | 职责 |
|---|---|
| [`config`](src/config.rs) | 解析基础目录，并为各工作目录派生稳定路径。 |
| [`session`](src/session.rs) | 定义原始会话存储记录，以及异步 `SessionReader`、`SessionStore` trait。 |
| [`session_id`](src/session_id.rs) | 校验并规范化持久化会话标识。 |
| [`dag`](src/dag.rs) | 定义持久化 DAG 运行与节点快照及其状态文件路径。 |
| [`triggers`](src/triggers.rs) | 定义会话级动态 trigger 与 cron sidecar 记录。 |

[`theway-core`](../theway-core/README.md) 负责在带类型的运行时会话条目和这些原始记录之间转换。[`theway-storage`](../theway-storage/README.md) 实现持久化 trait；[`theway-transport`](../theway-transport/README.md) 复用或重新导出适合位于该叶子层的客户端可见数据。

## 文档

- [架构与不变量](docs/architecture.md)
- [工作区架构](../../docs/architecture.md)

## 验证

```bash
cargo test -p theway-contract
cargo doc -p theway-contract --no-deps
```
