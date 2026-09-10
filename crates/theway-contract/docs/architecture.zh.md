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

[`subagent_settings.rs`](../src/subagent_settings.rs) 包含项目级的"最后一次设置"子代理模型/思考强度覆盖（按 DAG 节点 id 与子代理 spec 名索引），以及 `subagent_settings_path_for_project` 路径规则——文件位于 `<project>/.pi/subagent-settings.json`，独立于会话级状态文件。合并策略与文件读写位于 `theway-daemon`。

[`triggers.rs`](../src/triggers.rs) 包含动态 trigger 规则和 cron job 的 sidecar 表示。轮询、调度、提升和投递位于 `theway-daemon`。

## 用户输入与附件记录

[`user_input.rs`](../src/user_input.rs) 负责一轮输入的 canonical 形态，以及各层命名附件字节所用的 `sha256:<64 lowercase hex>` 标识。记录只携带 digest，[`attachments.rs`](../src/attachments.rs) 声明这些 digest 所解析到的字节存储契约，因此具体存储布局与准入策略都不进入本 crate。

记录以 role 为 `UserInput::CUSTOM_ROLE` 的 `AgentMessage::Custom` 条目进入会话，位置紧邻它所描述的用户消息之前。显示投影与模型请求都在下游派生；没有该条目的会话仅依据消息渲染，因此该记录是追加式的，绝不改写已存历史。

[`config.rs`](../src/config.rs) 依据与其他布局相同的基础目录规则派生 `attachments_dir()` = `<base>/attachments/v1`。trait 实现与对象布局属于 `theway-storage`；解析 mention 并在记录产生之前写入字节的准入属于 `theway-daemon`。

## 宿主 shell 解析

[`shell.rs`](../src/shell.rs) 选择执行本地命令行的宿主 shell：用哪个程序，以及哪些前缀参数承载命令字符串。`shell()` 返回 `&'static ShellSpec`，通过 `OnceLock` 在进程内只解析一次；`ShellSpec { program, args }` 提供 `command_args(&command)`（完整参数向量）与 `display()`（以空格连接的 program 与前缀参数）。daemon 的 `bash`、`exec`、hook 命令与 native 执行路径（`tools/bash.rs`、`tools/exec.rs`、`tools/exec_shell.rs`、`env/native.rs`）以及本机 TUI 控制器的 `LocalToolOps` 都在此处解析 shell。

解析顺序中，非空 `THEWAY_SHELL` 优先于所有平台默认值。其前缀参数取自非空 `THEWAY_SHELL_ARGS`（按 ASCII 空白切分），未设置时按 program 的 file stem 大小写不敏感推导：`pwsh` 与 `powershell` 取 `-NoLogo -NoProfile -Command`，`cmd` 取 `/C`，其它程序取 `-c`。无覆盖时，Windows 宿主取 `PATH` 上第一个 `pwsh`，其次 `powershell`，再次 `cmd`，PowerShell 使用 `-NoLogo -NoProfile -Command`、`cmd` 使用 `/C`；三者都不在时回落非空 `%COMSPEC%`，最后回落 `cmd.exe`，两者都使用 `/C`。无覆盖时，Unix 宿主执行 `sh -c`。

解析不可失败：环境变量未设置、为空或不可用时退化为平台默认值，而不是返回错误。候选可执行文件必须是存在的普通文件，空 `PATH` 条目会被跳过，Windows 探测先尝试裸名，再按变量列出的顺序尝试 `PATHEXT` 扩展名，并以 `.COM;.EXE;.BAT;.CMD` 作为 `PATHEXT` 未设置或为空时的默认值。

该模块属于本 crate：daemon 的各执行路径与 `theway-tui` 在同一宿主上运行命令，而 `theway-tui` 不允许依赖 `theway-core`；`theway-contract` 是二者共同依赖的唯一 crate，而解析 shell 不为其引入任何工作区依赖。

## 不变量

- 附件字节只以 `sha256:` digest 引用；记录不内嵌文件或图片内容。
- 公开记录与具体存储库、传输库保持独立。
- Serde 字段名、默认值和枚举编码属于持久化数据规则，变更必须有往返与兼容性测试。
- 路径派生和会话标识校验由共享函数提供，不在消费 crate 中复制实现。
- 本 crate 不引入需要 LLM provider、daemon 服务、文件系统后端或客户端 UI 的行为。
- Runtime extension 记录只包含无版本、可 JSON 序列化的 ABI 数据；脚本引擎值和客户端专属渲染对象不会进入本 crate。
