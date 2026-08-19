# theway-contract 修改规则

本文件适用于 `crates/theway-contract/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改持久化记录或路径规则前，先阅读[crate 概览](README.md)和[架构说明](docs/architecture.md)。

## 归属规则

- 只有当多个运行时层需要同一种与引擎无关的表示或持久化接口时，才把类型放入本 crate。
- 运行时策略、数据库代码、协议转换和 UI 行为不得进入本 crate。
- 不得添加工作区依赖；`theway-contract` 必须保持为运行时数据的依赖叶子。

## 兼容性规则

- serde 字段名、枚举编码、可选字段默认值和 `StoredSessionEntry` 校验均属于持久化格式契约。
- `config::cwd_hash`、会话目录布局和会话标识校验必须兼容已有磁盘名称。
- 原始记录与 core 运行时类型之间的转换放在 [`theway-core`](../theway-core/README.md)，协议消息转换放在 [`theway-transport`](../theway-transport/README.md)。

## 测试与文档

- 记录结构变化要补往返测试，校验规则变化要补非法输入测试。
- 模块归属、兼容性行为或公开 trait 变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-contract` 和 `cargo doc -p theway-contract --no-deps`。
