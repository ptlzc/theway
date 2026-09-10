# theway-daemon

[English](README.md) | 中文

`theway-daemon` 是无头应用内核及 `thewayd` 二进制。它把 `theway-core`、`theway-storage`、`theway-transport`、`theway-llm-provider` 和 `theway-mcp` 组装成一个长驻服务。

Daemon 负责会话运行时组装、面向模型的工具、本地与 sandbox executor 选择、hook、trigger、cron job、嵌套 agent 编排、MCP/LSP 集成、遥测导出和协议侧行为。它没有客户端形态或终端展示概念；`theway-tui` 只是一个协议客户端。

## 入口

- `thewayd` 解析进程参数，并调用公开组合入口 `run(DaemonOptions)`。
- `DaemonPaths` 在启动时一次性解析 base、home、工作目录和额外 skill 目录。
- `DaemonServices` 持有进程生命周期级注册表和命令输出注入。
- `SessionRuntimeBuilder` 是初始、恢复和切换会话运行时的统一内部构建路径；session-scoped runtime extension 的启动上下文由 `SessionExecutionContext` 持有。
- 公开模块为 executor、hook、存储适配器、工具、template、skill、trigger 和 TypeScript 扩展提供支持的扩展点；扩展宿主负责 package 发现、信任、QuickJS 隔离、capability broker、可逆注册、持久状态 projection、静默点重载和客户端中立诊断。

执行环境在运行时选择。`config.toml` 中的 `[executor] kind = "local" | "sandbox"` 由 `theway-tui` 以 `--executor-kind` 传给新启动的 daemon（issue #123）。默认 `local` 绑定 `LocalExecutor`；`sandbox` 绑定 `SandboxExecutor`，不支持的操作以 `ExecutorError::UnsupportedKind` 失败，并省略直接访问操作系统的工具。协议服务也可以把 `ToolOps` 转发到 controller 提供的 gRPC 工具端点。

基于 trigram 索引的 `grep` 后端可以关闭（issue #135）：`config.toml` 中的 `[tools] tgrep = false` 由 `theway-tui` 以 `--no-tgrep` 传给 daemon，从而绑定 `TgrepServerRegistry::disabled()`。此时每次 `grep` 查询都走进程内 walker，不会创建 `tgrep serve` 进程或 `.tgrep` 索引。该设置仅启动时生效，并出现在 `GetConfig` 视图中。

模型目录由 controller 提供（issue #136）。`theway-tui` 解析 `config.toml` 的 `[model]`（`provider`、`model`、`base_url`、`api_key`、`auto_fetch_models`）与 `[[model.custom]]`，经 settings RPC 下发；daemon 在解析模型前注册这些描述符，并按「provider 环境变量 → `api_key` → `auth.json`」的顺序解析凭证。`auto_fetch_models = true` 时 daemon 从 `GET <base_url>/models` 导入目录，并在未配 model 时取第一个条目。headless 等价参数为 `--api-key` 与 `--auto-fetch-models`；原先的 `models.json` 文件不再读取。

## 结构化用户输入

[`attachments/mod.rs`](src/attachments/mod.rs) 负责一轮输入的准入。`PromptAdmission::admit` 通过 `theway_transport::mentions::mentions` 只解析一次该轮的 `@path` mention——按会话 cwd 逐个读取路径、按共享的 64 KiB 窗口截断、跳过无法解析的路径、把重复路径合并为一个 part——再按共享的 `theway_transport::images` 规则校验提交的图片（magic bytes、单图大小、每消息数量），把所有字节写入附件库，最后才返回 `UserInput` 记录；图片校验在首次写入之前完成，因此因图片被拒绝的提交不写入任何内容，`PromptAdmission::with_injected` 追加 skill、trigger 或 extension part。`turn/daemon/input.rs` 把该记录交给 `AgentHarness::prompt_with_input` / `record_user_input_prompt`。

`/skill` 轮次以 `attach_skill_prompt(text, Some(name))` 信封到达，`PromptAdmission::admit` 用 `theway_transport::commands::split_skill_prompt` 拆解它：记录的 `text` 是用户原文，来自 `skill_prompt_preamble` 的信封前言成为 `Injected { source: "skill", name }` part。调用方已持有的模型侧 prompt 不会被改写。

`DaemonServices.attachments` 持有各会话共享的进程生命周期 `LocalAttachmentStore`。`start_process_services` 接收已解析的 `DaemonPaths::base` 并传给 `DaemonServices::with_attachments_base`，由它把附件库根设为 `<base>/attachments/v1`；该 base 依次由 `thewayd --theway-dir`、`$THEWAY_DIR`、`<home>/.theway` 决定，因此 `--theway-dir` 覆盖会让附件库随基础目录布局一起移动。

命令合成的 prompt 同样携带记录。`turn/daemon/input.rs::command_prompt_record` 记录 `CommandOutcome::RunAgentPrompt` 的 prompt——skill 信封记作 `InputSource::User` 并带同样的注入 part，其余（goal 命令、trigger 编写命令、file 命令、宿主注入文本）记作 `InputSource::Host`——`turn/daemon/commands/triggers.rs::trigger_web_rule_now` 把 `WireCommand::TriggerRuleNow { id }` 轮次记作 `InputSource::Trigger`，并在 `source_ref` 中带上规则 id。

trigger 或 cron 轮次注入父会话时，在 `trigger_engine/execution` 的两条分支上都把记录紧接在用户消息之前写入：父 agent 流式输出时进入 follow-up 队列，空闲时依次直接追加到 transcript 与 agent 状态。模型侧消息保留 `[Trigger <trace_id>] ` 前缀，而记录的 `text` 去掉该前缀，`source` 为 `InputSource::Trigger`，`source_ref` 为 trace id。

每个展示面都从该记录派生：live feed、resume 回放（`feed_replay::replay_transcript`）与消息分页（`session_observability.rs`）渲染同一个用户块——提交原文、每个附件一个 chip、该轮来源——而每个 `Injected` part 单独成为一条 `Context` 行，因此注入文本与展开的文件内容都不会出现在用户气泡里。

## 运行与验证

```bash
cargo run -p theway-daemon --bin thewayd -- --help
cargo test -p theway-daemon
cargo doc -p theway-daemon --no-deps --document-private-items
```

[Daemon 架构](docs/architecture.md)说明启动、会话、存储、工具、协议和可观测性归属。
