# theway-transport 架构

[English](architecture.md) | 中文

## 依赖位置

`theway-transport` 使用 `theway-contract` 的共享持久化记录，并使用 `theway-llm-provider` 中被配置和 snapshot 复用的模型数据。它不依赖运行时引擎、daemon 应用或存储实现。

该依赖方向使服务端或客户端无需链接 `AgentHarness`、SQLite、终端渲染或 daemon 工具即可使用协议。

## Wire 模型与端点 API

[`wire.rs`](../src/wire.rs) 是服务端事件循环与 JSON 传输共享的 serde 表示。`WireCommand` 把变更送入串行运行时循环，包括原子 `ActivateSession` 命令和只写 `SetCredential` / `ClearCredential` 命令，使其与 turn 保持有序。`WireStatus` 是客户端权威 snapshot；`WireStatusUpdate` 可携带完整 snapshot，或仅在 base index 与接收方 snapshot 匹配时应用的 feed delta。

[`transport.rs`](../src/transport.rs) 定义面向服务端的 API：

- `TransportEndpoints` 包含命令 channel、状态 broadcaster 与最新 snapshot、agent/DAG 事件 broadcaster、会话标识和路径/配置视图，以及操作 trait object。
- `SessionOps`、`JobOps`、`GraphOps`、`ToolOps` 和 `StorageOps` 暴露无需直接访问运行时状态的请求/响应操作。
- 宿主不支持某个可选操作组时，`Unavailable*` 实现提供明确错误或空行为。

[`host.rs`](../src/host.rs) 定义 `TransportHost`。服务端把 endpoints 交给 transport，再让串行应用循环与 server task 并行运行；具体实现由 daemon 提供。

## gRPC 传输

[`commands.proto`](../proto/commands.proto)、[`events.proto`](../proto/events.proto)、[`state.proto`](../proto/state.proto) 等 protobuf 文件是服务与消息的事实来源。[`build.rs`](../build.rs) 使用 `protox` 和 `tonic-prost-build` 编译全部 proto，因此不要求系统安装 `protoc`。

[`grpc/mod.rs`](../src/grpc/mod.rs) 把 command、session、settings、graph、event、tool、storage 和 health 服务映射到 `TransportEndpoints`。必须与 turn 串行的变更操作入队 `WireCommand`；读取/控制 trait 通过各自 endpoint object 执行。Session 服务暴露 `ActivateSession`、`SetCredential` 和 `ClearCredential`；凭据 secret 只写，不会出现在响应、snapshot 或事件中。事件订阅先收到当前状态，再接收增量 frame；发生 lag 时从最新权威 snapshot 恢复。

[`proto.rs`](../src/proto.rs)、[`tools.rs`](../src/tools.rs) 和 [`state.rs`](../src/state.rs) 负责会话状态、工具操作和运行时存储记录的 protobuf 转换。Proto 变化必须同步更新这些转换，并通过 `make sdk-sync` 更新生成的 TypeScript SDK。

## Web 传输

[`http.rs`](../src/http.rs) 从与 gRPC 相同的 endpoint 集合提供 health、JSON-RPC、SSE 事件和 WebSocket upgrade 路由。[`ws.rs`](../src/ws.rs) 接收 JSON 命令，并以 JSON frame 发布 status、agent 和 DAG 事件。

HTTP 与 WebSocket handler 把 carrier 输入转换为 transport 自有请求或 `WireCommand`，不实现模型、会话、存储、工具或图策略。

## 客户端与 daemon 发现

[`client.rs`](../src/client.rs) 把生成的 tonic 客户端包装为 `GrpcClient`，并暴露带类型的命令、状态流、会话/图控制、controller 工具服务和存储服务调用。

Daemon 发现从 `${THEWAY_DIR:-$HOME/.theway}` 读取按工作目录区分的 port/pid 文件，探测候选 loopback 地址，只在所有权匹配时删除过期记录，并可启动 `thewayd` 后等待就绪。发现机制面向 loopback，不额外定义认证协议。

## 共享客户端记录

[`feed/mod.rs`](../src/feed/mod.rs)、[`commands.rs`](../src/commands.rs)、[`auth.rs`](../src/auth.rs)、[`history.rs`](../src/history.rs)、[`images.rs`](../src/images.rs) 和 [`mentions.rs`](../src/mentions.rs) 等模块定义可复用的客户端/daemon 记录与纯辅助函数。叶子路径、trigger、cron 和原始持久化定义留在 `theway-contract`，只在需要稳定 transport 路径时重新导出。

## Feed 回放与结构化输入记录

[`feed/wire.rs`](../src/feed/wire.rs) 负责 serde feed 块枚举，[`feed/types.rs`](../src/feed/types.rs) 把它镜像为 `Block` 模型，[`feed/model.rs`](../src/feed/model.rs) 负责生产方 daemon 与消费方客户端共享的结构化 `Feed`。用户块在文本之外携带附件 chip 与来源，Context 行是独立块。

[`feed/replay.rs`](../src/feed/replay.rs) 从已存数据重建块：`replay_entries` 消费 `TranscriptEntry::{Message, UserInput}`，因此带结构化记录的会话从记录回放用户块及其 Context 行；`replay_messages` 服务于只有消息的 transcript。一条记录描述紧随其后写入的 `Message::User` 条目，回放恰好跳过该条目，因此一轮只渲染一次。

[`feed/plain.rs`](../src/feed/plain.rs) 负责 Context 行的展示形态：`context_line` 渲染 `[<label>] <text>` 并压平内嵌换行，`Feed::plain_lines`、纯文本行缓存与 TUI 带样式渲染器都调用它，因此一个 Context 块始终算作一行。

[`proto/feed.rs`](../src/proto/feed.rs) 把生成的 `FeedBlock` 消息转换为 wire 块，[`proto/resources.rs`](../src/proto/resources.rs) 把 wire 块转换回去，覆盖附件 chip、来源与 Context 行。Proto 源、双向转换与生成的 SDK 产物同步变化。

## Skill prompt 信封

[`commands.rs`](../src/commands.rs) 负责 skill 信封：让 agent 在回答本轮前先调用指定 skill 的包装文本。`SKILL_PROMPT_LEAD` 与 `SKILL_PROMPT_USER_MARKER` 是该格式的唯一权威定义——`attach_skill_prompt` 用它们包裹 skill 名与用户原文，并在未给 skill 名时原样返回原文——因此构建方、前言与解析器不会各自漂移。

`skill_prompt_preamble` 单独渲染同一前缀，作为本轮之前注入的内容；`split_skill_prompt` 把 prompt 解析回 `(skill 名, 用户原文)` 对，prompt 不是信封时返回 `None`。daemon 的输入准入用 `split_skill_prompt` 拆解 `/skill` 轮次以记录用户原文，因此该格式保持可往返：`split_skill_prompt(&attach_skill_prompt(text, Some(name)))` 返回 `(name, text)`，没有调用方会从渲染后的字符串重新推导该格式。

## 不变量

- Wire 与 protobuf 记录不包含 core 或 daemon 私有类型。
- 所有 carrier 驱动同一套 `TransportEndpoints` 语义；carrier 专用 handler 不承载业务策略。
- 需要排序的运行时变更进入串行命令队列。
- Snapshot delta 只应用于匹配的 base；lag 或不匹配后通过完整权威 snapshot 恢复。
- Proto 源、Rust 转换、服务 handler、客户端调用和生成 SDK 同步变化。
- Transcript 携带结构化记录时，feed 用户块来自该记录；否则来自已存消息。两条路径产出相同的块种类。
- Skill 信封保持唯一定义：`split_skill_prompt` 还原 `attach_skill_prompt` 包裹的名称与原文，`skill_prompt_preamble` 渲染同一前缀。
- 本 crate 不包含客户端外观、终端输入处理、存储后端或 MCP 实现。
