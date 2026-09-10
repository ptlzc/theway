# theway-transport

[English](README.md) | 中文

`theway-transport` 负责跨客户端 wire 模型，以及控制 theway daemon 的 gRPC 与 web 传输。它提供生成的 protobuf 服务、HTTP JSON-RPC、SSE、WebSocket 事件、带类型的 gRPC 客户端、daemon 发现辅助函数和面向传输的操作 trait。

本 crate 独立于 `theway-core`、`theway-daemon` 和 `theway-storage`。服务端实现 `TransportHost` 并提供 `TransportEndpoints`；客户端只使用 wire/protobuf 类型和 `GrpcClient`，不访问运行时内部状态。

## 协议入口

- `wire` 定义命令、完整和增量状态 snapshot、图/job 事件、配置、操作请求/结果记录，以及开放的 runtime extension catalog、诊断、命令、contribution、信任和重载记录。
- `transport` 定义 `TransportEndpoints` 以及 `SessionOps`、`JobOps`、`GraphOps`、`ToolOps`、`StorageOps`。
- `grpc`、`http` 和 `ws` 通过 protobuf RPC、JSON-RPC、SSE 与 WebSocket 暴露这些操作。
- `proto`、`tools` 和 `state` 在内部 wire 记录与生成的 protobuf 消息之间转换。
- `client` 包装 tonic 客户端，并按工作目录发现或启动 daemon。
- `feed`、`commands`、`auth`、`history`、`images`、`mentions` 等共享模块定义不绑定具体 carrier 的客户端/daemon 数据。

MCP 传输不在本 crate 实现：外部 MCP 客户端位于 `theway-mcp`，daemon 的 MCP server 位于 `theway-daemon`。

## Feed 块与 transcript 回放

`WireFeedBlock::User` 除了该轮自身的文本，还携带 `attachments: Vec<WireFeedAttachment>` 与 `source: Option<WireFeedSource>`：附件为 `{ kind: "file" | "image", name, detail }`，其中 `detail` 仅供展示；来源为 `{ kind: "user" | "trigger" | "subagent" | "host", label }`。`WireFeedBlock::Context { label, text, timestamp }` 是模型看到该轮之前被注入内容的独立行。新增字段在 wire 上可选，因此不含这些字段的旧载荷仍可反序列化。

`TranscriptEntry::{Message, UserInput}` 与 `replay_entries` 从两种来源重建已存 transcript：`user_input` 记录提供用户块——提交原文、每个 `File` / `Image` part 一个 chip、来源——以及每个 `Injected` part 一条 `Context` 行，随后跳过该记录所描述的 `Message::User`。`user_input_blocks` 是记录到块的纯映射函数，`replay_messages` 重建只有消息的 transcript。Context 行渲染为 `[<label>] <text>`——label 为空时渲染裸文本——内嵌换行被压平为空格；`feed::plain::context_line` 是该行的唯一实现，供纯文本投影、纯文本行缓存和 TUI 带样式渲染器共用。

`proto/session.proto` 镜像这些形态：`FeedBlock.context = 9`，`UserBlock.attachments = 3`、`UserBlock.source = 4`，`FeedAttachment.kind = 1` / `name = 2` / `detail = 3`，`FeedSource.kind = 1` / `label = 2`，`ContextBlock.label = 1` / `text = 2` / `timestamp = 3`。

`mentions::mentions` 是唯一的 `@path` 解析器：daemon 在准入时解析一轮的 mention 一次并记录它读取的文件，客户端不再展开 mention。`mentions::expand` 是文本路径辅助函数，逐个读取 mention 并把 `Files in context:` 块追加到 prompt 文本之后。

## Skill prompt 信封

`attach_skill_prompt(text, Some(name))` 用 `SKILL_PROMPT_LEAD` 与 `SKILL_PROMPT_USER_MARKER` 渲染 skill 信封——这两个常量是该格式的唯一权威定义——未给 skill 名时原样返回 `text`。`skill_prompt_preamble(name)` 单独渲染前言，`split_skill_prompt` 是其逆操作，把信封解析回 skill 名与用户原文，对非信封的 prompt 返回 `None`。daemon 的输入准入用 `split_skill_prompt` 拆解提交的信封，因此该格式必须保持可往返：`split_skill_prompt(&attach_skill_prompt(text, Some(name)))` 返回 skill 名与构建方包裹的原文。

## 文档

- [Wire 与传输架构](docs/architecture.md)

## 验证

```bash
cargo test -p theway-transport
cargo doc -p theway-transport --no-deps --document-private-items
make layering-check
```
