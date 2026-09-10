# theway-storage 架构

[English](architecture.md) | 中文

## 依赖位置

`theway-storage` 实现 `theway-contract` 的记录和异步 trait。它不依赖 `theway-core` 的带类型运行时，也不依赖 `theway-transport` 的协议类型。

Daemon 和 TUI 决定何时使用本地持久化。本 crate 负责本地文件及其恢复行为，不负责会话执行或客户端协调。

## 会话仓库与数据库

[`sqlite_repo.rs`](../src/sqlite_repo.rs) 负责一个仓库目录。`SqliteSessionRepo` 创建 `<uuidv7>.db`、打开指定路径、列举数据库文件，并删除精确指定的会话文件。

[`sqlite_storage.rs`](../src/sqlite_storage.rs) 负责单个会话数据库。`SqliteSessionStorage` 在 `meta` 表保存元数据，并按序列顺序保存追加式 `StoredSessionEntry` JSON 载荷。`append_entries` 在打开单个 SQLite 事务前序列化完整有序批次，并一起提交所有行；序列化、约束或提交失败都不会暴露批次中的任何条目。最新普通条目成为活动叶节点；`leaf` 条目把指针移动到其记录的目标。

`get_extension_entries` 解析显式叶节点或持久化活动叶节点，沿父索引回溯到根，并按重放顺序仅返回目标 extension 的不透明条目。因此，分支切换、进程重启、fork 和归档导入均从会话日志重建 extension 数据，不依赖 daemon 本地缓存。

打开已有会话时执行 SQLite 完整性检查并解码元数据。损坏会话返回 `SessionErrorCode::Corrupted` 且不修改文件，因为 transcript 属于用户数据。`checkpoint` 在归档导入的 staging 数据库重命名前刷出 WAL 页面。

[`session.rs`](../src/session.rs) 在原始 store 上提供创建、恢复、fork、列举、预览、重命名、查找、删除和自动化 sidecar 发现。这些辅助函数仍只处理原始存储条目，不解码 `theway-core::SessionTreeEntry`。

## 会话归档

[`session_archive.rs`](../src/session_archive.rs) 把规范的 `session.jsonl`、manifest 和可选 trigger/cron sidecar 导出到 `.theway-session` tar 归档。Manifest 记录 transcript 哈希、条目数、活动叶节点、来源标识和包含的 sidecar；provider 凭证和独立认证存储不进入归档。

导入只接受固定成员名，执行成员大小上限、schema、transcript SHA-256、条目数、活动叶节点、条目图、UTF-8 与 sidecar 语法校验，然后分配新的 UUIDv7 会话标识。数据先写入不以 `.db` 结尾的 staging 数据库，完成 checkpoint 和 sidecar 写入后，以数据库重命名作为提交点。失败导入会删除 staging 数据库及 sidecar。

除非选择 `ActivateTriggers::On`，导入后的自动化默认关闭。交互式 `Ask` 由调用客户端处理，`import_session` 不执行交互。

## 附件库

[`attachments.rs`](../src/attachments.rs) 把叶子契约的 `AttachmentStore` 实现为独立于会话数据库的本地对象树。`LocalAttachmentStore::new` 接收显式根目录，`from_base_dir` 以 `theway_contract::config::attachments_dir`（`<base>/attachments/v1`）为根，`root` 返回解析后的目录。

对象位于 `<root>/<shard>/<hex>`：`hex` 是 `sha256:<hex>` digest 的 64 个小写十六进制字符，`shard` 是其前两个字符。`put` 对字节取哈希，在对象文件已存在时原样返回 digest；否则创建 shard 目录，在同一目录内写入 `tmp-<pid>-<nanos>` 暂存文件，再以重命名提交——重命名失败会删除暂存文件，因此既不会留下半个对象，也不会残留暂存文件。由于对象名即内容 digest，已存在的对象不会被重写。

`get` 对格式非法的 digest 返回 `InvalidDigest`，对缺失对象返回 `NotFound`，并重新计算读回字节的 digest，在字节与其登记 digest 不一致时返回 `DigestMismatch`，而不是返回这些字节。`contains` 只根据对象是否存在作答，从不读取字节。

## DAG 快照

[`sqlite_dag.rs`](../src/sqlite_dag.rs) 存储 `PersistedRun` 与 `PersistedNode`。`save` 在事务中替换完整快照集合；`load` 跳过无法解码 JSON 的单行记录。

DAG 快照是可重建的运行时状态。数据库无法打开或写入时会丢弃并重建一次；损坏的会话 transcript 则保留并报告。

## 不变量

- 会话数据库是高价值追加式记录，损坏后绝不自动重建。
- 有序会话条目批次是全有或全无的 SQLite 事务。
- Extension 载荷对存储保持不透明，分支选择从持久化父节点和叶节点记录推导。
- DAG 快照数据库是可替换投影，损坏后可以重建。
- 归档导入在暴露最终 `<uuidv7>.db` 路径之前校验全部内容。
- 附件对象不可变且内容寻址：已存在的对象不会被重写，读取在返回字节前校验 digest。
- 附件对象以重命名提交，读取方不会观察到写了一半的对象。
- 原始持久化与带类型运行时、wire 表示保持独立。
- Sidecar 路径通过共享辅助函数从最终会话数据库路径派生。
