# theway-storage

[English](README.md) | 中文

`theway-storage` 为 `theway-contract` 的原始持久化接口提供本地耐久实现。它为每个会话保存一个 Turso/SQLite 数据库，以原子方式提交有序条目批次，从所选分支重放 extension 条目，管理会话发现和 sidecar 路径，导入导出 `.theway-session` 归档，并保存持久化 DAG 快照。

本 crate 不解释带类型的 agent 消息或 DAG 状态转换规则。在运行时工作区 crate 中，它只依赖 `theway-contract`，不导入 `theway-core` 或 `theway-transport`。

## 公开模块

| 模块 | 职责 |
|---|---|
| [`attachments`](src/attachments.rs) | 在本地对象树中存储内容寻址的附件字节。 |
| [`sqlite_storage`](src/sqlite_storage.rs) | 为单个会话数据库实现 `SessionReader` 和 `SessionStore`。 |
| [`sqlite_repo`](src/sqlite_repo.rs) | 在一个仓库根目录下创建、打开、列举和删除会话数据库文件。 |
| [`session`](src/session.rs) | 提供创建、恢复、fork、列举辅助函数，以及会话预览和 trigger/cron sidecar 路径。 |
| [`session_archive`](src/session_archive.rs) | 导出和导入经过校验的 `.theway-session` tar 归档。 |
| [`sqlite_dag`](src/sqlite_dag.rs) | 替换并恢复持久化 DAG 运行快照。 |

## 附件库

[`attachments`](src/attachments.rs) 把契约的 `AttachmentStore` 实现为 `LocalAttachmentStore`：以某个目录为根的内容寻址对象树。`new(root)` 接收显式根目录，`from_base_dir()` 以 `<base>/attachments/v1` 为根——即 `theway_contract::config::attachments_dir` 的布局——`root()` 返回解析后的目录。

对象位于 `<root>/<ab>/<sha256>`：`<ab>` 是 digest 的前两个十六进制字符，对象文件名使用完整的 64 个十六进制字符。`put` 按内容 digest 去重，把新字节写入目标目录内的临时文件，再以重命名提交，因此并发读取方看到的是完整对象或什么都没有。`get` 重新计算读回字节的 digest，在字节与所登记 digest 不一致时返回 `DigestMismatch`，而不是返回该内容；`contains` 只根据文件是否存在作答，不读取内容。

## 文档

- [持久化架构与失败行为](docs/architecture.md)

## 验证

```bash
cargo test -p theway-storage
cargo doc -p theway-storage --no-deps --document-private-items
make layering-check
```
