# theway-contract

[English](README.md) | 中文

`theway-contract` 是工作区的叶子契约 crate，存放需要跨越运行时、持久化与协议实现共享、但不能反向依赖这些实现的数据与宿主环境策略。它定义可序列化记录、存储 trait、自动化 sidecar 模型、会话标识、`~/.theway` 路径布局，以及执行本地命令的宿主 shell，不包含 agent 引擎、数据库后端或网络传输。

## 公开模块

| 模块 | 职责 |
|---|---|
| [`config`](src/config.rs) | 解析基础目录，并为各工作目录派生稳定路径。 |
| [`session`](src/session.rs) | 定义原始会话存储记录，以及异步 `SessionReader`、`SessionStore` trait。 |
| [`session_id`](src/session_id.rs) | 校验并规范化持久化会话标识。 |
| [`dag`](src/dag.rs) | 定义持久化 DAG 运行与节点快照及其状态文件路径。 |
| [`triggers`](src/triggers.rs) | 定义会话级动态 trigger 与 cron sidecar 记录。 |
| [`extension`](src/extension/mod.rs) | 定义唯一、无版本的 runtime extension ABI：manifest、生命周期与 action envelope、持久化条目、信任记录、诊断及客户端中立 contribution。 |
| [`user_input`](src/user_input.rs) | 定义一轮用户输入的 canonical 记录：提交原文、有序 part、来源，以及附件 digest 形态。 |
| [`attachments`](src/attachments.rs) | 定义内容寻址的附件字节存储契约及其失败情形。 |
| [`shell`](src/shell.rs) | 解析执行本地命令的宿主 shell：`THEWAY_SHELL` 覆盖、平台默认与平台回落。 |

`theway-core` 负责在带类型的运行时会话条目和这些原始记录之间转换。`theway-storage` 实现持久化 trait；`theway-transport` 复用或重新导出适合位于该叶子层的客户端可见数据。

工作区插件开发 SDK 中签入的 TypeScript 声明和 JSON Schema 由 Rust extension 契约生成。使用 `cargo run -p theway-contract --example generate_extension_artifacts -- sdks/plugin/abi` 重新生成；extension 契约测试会在临时目录中重新生成并拒绝漂移。

无版本 ABI 使生命周期 envelope、hook class 与 action、分支局部持久条目、诊断、信任记录、命令和客户端 contribution 保持引擎中立。这些记录没有敏感值、可执行 runtime 对象或 ABI 版本选择字段。

## 结构化用户输入

[`user_input`](src/user_input.rs) 是一轮输入的 canonical 记录：`UserInput { text, parts, source, source_ref }`。`text` 是用户提交的原文，`@path` token 保持用户输入的形态；`parts` 是有序的 `Vec<InputPart>`，记录附件与预注入内容；`source` 是 `InputSource`（`user` / `trigger` / `subagent` / `host`）；`source_ref` 标识非 `user` 来源。`UserInput::CUSTOM_ROLE`（`user_input`）是会话日志与显示投影共用的唯一 role 字符串。

`InputPart` 为 `File { path, name, digest, bytes, media_type, truncated }`、`Image { name, digest, bytes, media_type }` 或 `Injected { source, name, text }`；`has_attachments` 报告是否存在 `File` 或 `Image` part。字节不进入记录：`digest_bytes` 生成全仓统一的附件标识 `sha256:<64 lowercase hex>`（`DIGEST_PREFIX`，由 `digest_is_valid` 校验），记录只携带该 digest。

[`attachments`](src/attachments.rs) 定义 `AttachmentStore` trait（`put` / `get` / `contains`）与 `AttachmentError`（`InvalidDigest`、`NotFound`、`DigestMismatch`、`Io`）。`put` 返回所写入字节的 digest，`get` 在返回字节前重新校验该 digest。[`config::attachments_dir`](src/config.rs) 从共享基础目录派生附件库根目录 `<base>/attachments/v1`。

该记录是对会话日志的追加：它紧邻它所描述的用户消息之前写入，没有该记录的旧会话仅依据消息渲染。这些记录的 `camelCase` 字段名与 snake_case 枚举标签属于持久化格式行为。

## 宿主 shell

[`shell`](src/shell.rs) 持有本机命令的宿主 shell 策略：`shell::shell()` 返回 shell 程序，以及承载命令行的参数。daemon 的 `bash`、`exec`、hook 与 native 执行路径，以及本机 TUI 控制器的 `LocalToolOps`，都从这里解析 shell。

解析从 `THEWAY_SHELL` 开始：它指定 shell 程序并覆盖一切平台默认；设置时，可选的 `THEWAY_SHELL_ARGS` 提供按空白切分的前缀参数，置于命令之前。未设置 `THEWAY_SHELL` 时，Windows 取 `PATH` 上第一个 `pwsh`，其次 `powershell`，再次 `cmd`，PowerShell 以 `-NoLogo -NoProfile -Command` 运行，`cmd` 以 `/C` 运行；三者都不存在时回落到 `%COMSPEC%`，再回落到 `cmd.exe`。Unix 上的宿主 shell 是 `sh -c`。

## 文档

- [架构与不变量](docs/architecture.md)

## 验证

```bash
cargo test -p theway-contract
cargo doc -p theway-contract --no-deps
```
