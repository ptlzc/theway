# theway-storage

`theway-storage` 为 [`theway-contract`](../theway-contract/README.md) 的原始持久化接口提供本地耐久实现。它为每个会话保存一个 Turso/SQLite 数据库，管理会话发现和 sidecar 路径，导入导出 `.theway-session` 归档，并保存持久化 DAG 快照。

本 crate 不解释带类型的 agent 消息或 DAG 状态转换规则。在运行时工作区 crate 中，它只依赖 `theway-contract`，不导入 [`theway-core`](../theway-core/README.md) 或 [`theway-transport`](../theway-transport/README.md)。

## 公开模块

| 模块 | 职责 |
|---|---|
| [`sqlite_storage`](src/sqlite_storage.rs) | 为单个会话数据库实现 `SessionReader` 和 `SessionStore`。 |
| [`sqlite_repo`](src/sqlite_repo.rs) | 在一个仓库根目录下创建、打开、列举和删除会话数据库文件。 |
| [`session`](src/session.rs) | 提供创建、恢复、fork、列举辅助函数，以及会话预览和 trigger/cron sidecar 路径。 |
| [`session_archive`](src/session_archive.rs) | 导出和导入经过校验的 `.theway-session` tar 归档。 |
| [`sqlite_dag`](src/sqlite_dag.rs) | 替换并恢复持久化 DAG 运行快照。 |

## 文档

- [持久化架构与失败行为](docs/architecture.md)
- [叶子记录定义](../theway-contract/docs/architecture.md)
- [工作区架构](../../docs/architecture.md)

## 验证

```bash
cargo test -p theway-storage
cargo doc -p theway-storage --no-deps --document-private-items
make layering-check
```
