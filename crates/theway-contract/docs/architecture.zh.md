# theway-contract 架构

[English](architecture.md) | 中文

## 依赖位置

`theway-contract` 不依赖其他工作区 crate。运行时 crate 向内依赖它，避免持久化记录和共享路径规则反向获得 agent 引擎、SQLite 或传输层依赖。

本 crate 只负责数据表示和兼容性规则。选择策略、执行逻辑、具体介质序列化和协议处理留在对应实现 crate 中。

## 路径与标识规则

[`config.rs`](../src/config.rs) 优先解析 `${THEWAY_DIR}`，未设置时使用 `$HOME/.theway`。`sessions_dir_for_cwd` 把基础目录与 `cwd_hash` 生成的确定性哈希组合起来；修改该算法会改变已有会话数据的位置，因此必须明确处理兼容性。

[`session_id.rs`](../src/session_id.rs) 集中定义会话标识校验，使文件实现和协议实现接受同一组标识。

## 会话持久化记录

[`session.rs`](../src/session.rs) 将存储表示与运行时解释分开：

- `StoredSessionEntry` 保存持久化实现所需的原始 JSON 载荷、索引标识、父节点、时间戳和条目类型。
- `validate_session_entries` 校验条目结构，并从追加式记录序列推导活动叶节点。
- `SessionReader` 暴露元数据与树查询，包括按所选分支过滤并以根到叶重放顺序返回 extension 条目。
- `SessionStore` 在读取能力上增加条目创建、叶节点移动和原子有序条目批次；单条追加适配器使用同一批次契约。
- `SessionStore::set_binding` 持久化或清除非机密客户端绑定。默认实现以 `StorageFailure` 和稳定消息 `session store does not support binding updates` 失败关闭；支持绑定更新的存储后端会覆盖该实现。

`theway-core::PersistentSessionStorage` 负责对带类型的 `SessionTreeEntry` 进行编解码。本 crate 不解释 prompt、模型切换、压缩记录或自定义运行时事件。

## Runtime extension ABI 记录

[`extension`](../src/extension/mod.rs) 定义跨运行时层共享的唯一无版本 ABI。Package manifest、permission、信任决定、生命周期事件、hook/action 契约、extension 自有持久化条目、catalog 状态、脱敏诊断、命令结果及声明式客户端 contribution 都是可序列化值，不包含引擎句柄、协议对象或 ABI 选择字段。

`ExtensionDurableEntry` envelope 存储在不透明会话条目内，记录所属 extension、状态 schema、来源生命周期 sequence，以及一项私有状态 mutation、不可变 custom event、模型上下文条目或 migration 记录。存储实现保留 envelope 但不解释其 payload；运行时投影和策略留在本 crate 之外。

JSON Schema derive 与 [`generate_extension_artifacts.rs`](../examples/generate_extension_artifacts.rs) 生成由工作区插件开发 SDK 签入并发布的产物。生成的 TypeScript 文件包含 schema bundle 摘要，测试会将每个生成产物与临时目录中的重新生成结果比较。

## DAG 与自动化记录

[`dag.rs`](../src/dag.rs) 包含持久化图引擎快照所需的可序列化运行、节点、结果、状态和方向记录。图调度器与状态转换规则位于 `theway-core`。

[`triggers.rs`](../src/triggers.rs) 包含动态 trigger 规则和 cron job 的 sidecar 表示。轮询、调度、提升和投递位于 `theway-daemon`。

## 不变量

- 公开记录与具体存储库、传输库保持独立。
- Serde 字段名、默认值和枚举编码属于持久化数据规则，变更必须有往返与兼容性测试。
- 路径派生和会话标识校验由共享函数提供，不在消费 crate 中复制实现。
- 本 crate 不引入需要 LLM provider、daemon 服务、文件系统后端或客户端 UI 的行为。
- Runtime extension 记录只包含无版本、可 JSON 序列化的 ABI 数据；脚本引擎值和客户端专属渲染对象不会进入本 crate。
