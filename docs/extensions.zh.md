# 运行时扩展（ABI v2）

[English](extensions.md) | 中文

运行时扩展是在 daemon 内嵌 QuickJS 中执行、受 capability 约束的 JavaScript 或 TypeScript package。Package 通过 `@theway-ai/plugin-sdk` 注册生命周期 hook、工具、命令、provider、request policy、prompt section 和客户端中立 contribution；它没有环境式的文件系统、进程、网络、环境变量、凭据、provider、持久化、daemon 或终端访问权。

本文档是 ABI v2 package 结构、生命周期 hook、action、注册、状态、信任、重载、诊断及压缩兼容格式的规范参考。插件开发 SDK 位于 `sdks/plugin`，生成的契约声明和 JSON Schema 位于 `sdks/plugin/abi-v2`。

## Package 布局与发现

每个 ABI v2 扩展都是一个目录，包含严格的 `theway-extension.json` manifest 及其入口模块。项目 package 会遮蔽具有相同扩展 ID 的全局 package。

```text
<cwd>/.theway/extensions/<extension-id>/theway-extension.json
<cwd>/.theway/extensions/<extension-id>/index.js
$THEWAY_DIR/extensions/<extension-id>/theway-extension.json
$THEWAY_DIR/extensions/<extension-id>/index.js
```

发现过程是确定性的。格式错误、不受支持、未受信任或已故障的 package 会留在扩展 catalog 中并带有结构化诊断，不会阻止 daemon 或无关 package 启动。Catalog 按 manifest priority 降序、来源层和扩展 ID 排列有效 package。

### Manifest

```json
{
  "id": "workspace-policy",
  "version": "1.0.0",
  "abi": 2,
  "entry": "index.js",
  "priority": 100,
  "scope": "session",
  "stateSchema": 1,
  "permissions": ["session.write", "tools.register"],
  "optionalPermissions": ["tools.override"]
}
```

| 字段 | 必需 | 契约 |
|---|---:|---|
| `id` | 是 | 1–64 个小写 ASCII 字母、数字或单个连字符；它是状态、诊断、信任和 effect owner 的命名空间。 |
| `version` | 是 | 语义化版本字符串。 |
| `abi` | 是 | 必须为 `2`。 |
| `entry` | 是 | 非空的 package 相对模块路径，不允许父目录穿越。 |
| `priority` | 否 | 有符号排序值；默认为 `0`，较大值先分发。 |
| `scope` | 是 | `process`、`session`、`run` 或 `request`；注册不能超过 manifest scope 的生命周期。 |
| `stateSchema` | 否 | 正数形式的持久状态 schema 版本；package 拥有版本化状态或迁移时需要。 |
| `permissions` | 否 | 必需 capability；拒绝其中一项会阻止 package。 |
| `optionalPermissions` | 否 | Package 可用 `api.capabilities.has` 探测的 capability；拒绝不会阻止加载。 |

未知 manifest 字段、重复 permission、必需/可选重叠、不支持的 ABI 值以及不安全入口路径都会在入口求值前拒绝 package。

## 信任与 capability

项目 package 需要已记录的项目或精确 package 信任决定。精确 package 记录绑定扩展 ID、规范路径、内容摘要和请求的 permission 集合。内容或 permission 增加都需要新的决定。全局 package 遵循配置的全局策略：允许已声明 permission、要求记录或拒绝。

TUI 通过以下命令暴露客户端中立的协议操作：

```text
/extensions
/extension-trust project <trusted|denied> [permissions...]
/extension-trust package <extension-id> <trusted|denied> [permissions...]
/extension-reload [--cancel]
/ext:<registered-command> {"argument":"value"}
```

可信 TUI 决定省略 permission 时，会授予所选 catalog 条目声明的 permission。显式列表只授予其子集；每项必需 permission 都必须存在。信任记录在 theway 基础目录下原子写入，特权决定和 broker 调用生成经过脱敏的审计记录。

| Permission | 公开权限 |
|---|---|
| `session.write` | 排队写入命名空间状态变更、自定义事件、持久模型上下文和 follow-up 消息。 |
| `tools.register` | 注册面向模型的工具。 |
| `tools.override` | 请求覆盖现有工具；effect 结束时恢复被替换工具。 |
| `commands.register` | 注册 daemon 命令。 |
| `providers.register` | 注册声明式 provider/model 元数据。 |
| `client.contribute` | 发布经过校验的客户端中立 contribution。 |
| `workspace.read` | 读取允许的工作区根目录内的规范路径。 |
| `workspace.write` | 写入允许的工作区根目录内的规范路径。 |
| `process.spawn` | 通过 daemon executor 和取消策略运行进程。 |
| `network.connect` | 在网络策略和 quota 约束下发出 broker HTTP 请求。 |
| `provider.raw` | 注册原始 provider header/payload hook，并读取调用局部的原始 provider 数据。 |
| `secrets.read:<name>` | 只读取一个具名 secret；通配 secret 访问无效。 |

直接环境中不存在 `process`、`require`、`fetch`、`XMLHttpRequest`、`WebSocket`、环境变量访问和宿主资源对象。每个 broker 都检查已授予 capability、取消状态、调用上下文、规范路径或目标，以及每次调用的操作 quota。

## TypeScript setup API

将 `@theway-ai/plugin-sdk` 安装为开发依赖，并在编译 extension 时将其 import 保持为 external；它是唯一可导入的宿主模块。每个扩展实例由 `defineExtension` 接收一次异步 setup 函数。Setup 通过 handle 返回注册；每个 handle 支持幂等 `dispose()` 和经过校验的 `update(descriptor)`。

```sh
npm install --save-dev @theway-ai/plugin-sdk
```

```ts
import { defineExtension } from "@theway-ai/plugin-sdk";

export default defineExtension(async (api) => {
  api.registerTool({
    name: "workspace-note",
    label: "Workspace note",
    description: "Write one note inside the workspace.",
    inputSchema: {
      type: "object",
      properties: { path: { type: "string" }, text: { type: "string" } },
      required: ["path", "text"],
    },
    scope: "session",
  }, async ({ arguments: args }) => {
    await api.workspace.writeText(args.path, args.text);
    return { content: [{ type: "text", text: `wrote ${args.path}` }], details: {} };
  });

  api.on("before_model_request", {
    priority: 20,
    payloadSchema: { type: "object", required: ["request"] },
  }, async ({ payload }) => ({
    abiMajor: 2,
    actions: [{
      kind: "replace_model_request",
      payload: { request: payload.request },
    }],
  }));
});
```

Setup API 暴露 `capabilities.has`、`workspace.readText/writeText`、`process.run`、`network.fetch`、`secrets.read`、`providerRaw.read`、`state.get/set/delete`、`events.replay/append`、`modelContext.append`、临时 `memory.get/set/delete/clear`、`migrateState`、下文描述的全部注册方法以及 `on`。Broker 方法是公开权限边界；实现全局对象和 Rust 类型不属于 ABI。

`api.on(event, descriptor?, handler)` 校验 `class`、`payloadSchema`、`allowedActions`、`priority`、`deadline`、`delivery` 和 `failure`。Descriptor 可以收窄 payload 投递并设置 priority，但显式声明的 `allowedActions`、`deadline`、`delivery` 或 `failure` 必须与下述规范契约完全一致。

## Hook 执行契约

每个 handler 收到包含 `abiMajor`、`event`、`context` 和 `payload` 的 `ExtensionEventEnvelope`。Context 始终包含宿主持有的 `extensionId`、`sessionId`、规范 `cwd` 和单调递增 `sequence`；适用时还包含 run、turn、request、message、tool-call scope ID，已选 provider/model，交互式客户端可用性，取消状态和 deadline。扩展不能替换宿主持有的 context 字段。

有效 package 的 hook 按 catalog 顺序运行。同一 event 和 class 的注册按 hook priority 降序、再按注册顺序运行。Transform hook 构成串行 waterfall，后一个 hook 看到最后一个有效值。Gate hook 在首个 `deny` 或 `cancel` 处终止。Observe hook 不能通过返回 action 改变状态，其失败不会改变运行时操作。

| Class | 返回契约 | 允许的 action | 失败策略 |
|---|---|---|---|
| `observe` | `null` 或空 batch | 无 | `continue`；诊断并隔离失败。 |
| `transform` | 不带 decision 的 action batch | 一个 event 专属替换加公共次级 action | `keep_last_value`；无效结果保留 waterfall 的最后有效值。 |
| `gate` | `abstain`、`allow`、`deny` 或 `cancel`，可附带公共次级 action | 公共次级 action | `deny`；超时、异常、owner 禁用或 malformed 输出都会拒绝操作。 |
| `register` | 不带 decision 的 action batch | `register_effect`、`dispose_effect`、`emit_diagnostic` | `reject_registration`；setup 与其他 package 隔离。 |

公共次级 action 是 `set_state`、`delete_state`、`append_custom_event`、`append_model_context`、`enqueue_follow_up` 和 `emit_diagnostic`。`input` 还允许 `emit_command_outcome`。持久 action 会完整校验并作为一个 batch 提交，然后临时 effect 才可见；持久化或 quota 失败会拒绝整个 batch。

默认宿主 deadline 是 `fast` 100 ms、`standard` 500 ms、`long` 2 s；部署可以配置时长，但不能更改 event 的 deadline class。`message_update` 和 `tool_execution_update` observation 使用有界合并队列；其他投递均为 inline。

### 规范 hook 参考

表中列出省略 `descriptor.class` 时选择的 class。任何 event 也接受显式 `observe` 注册；`extension_load` 还接受 `register`。“公共”表示上文定义的公共次级 action。

| Event | 默认 class | Payload | 主要/允许的 action | Deadline | Delivery | Failure |
|---|---|---|---|---|---|---|
| `extension_load` | observe | setup 和迁移后的 `{reason}` | 无 | long | inline | continue |
| `session_start` | observe | session 实例启动时的 `{reason}` | 无 | long | inline | continue |
| `input` | transform | 从客户端或 follow-up 接受的 `{message}` | `replace_input`、`emit_command_outcome`、公共 | standard | inline | keep last value |
| `before_session_switch` | gate | 分支 `{kind,fromEntryId,toEntryId}` 或会话 `{kind,targetSessionId}` | 公共 | standard | inline | deny |
| `session_switched` | observe | 已完成的分支/会话切换及结果 ID | 无 | standard | inline | continue |
| `before_session_fork` | gate | `{branchEntryId}` | 公共 | standard | inline | deny |
| `session_forked` | observe | `{targetSessionId}` | 无 | standard | inline | continue |
| `before_model_selection` | gate | 目标 `{provider,model}` | 公共 | standard | inline | deny |
| `model_selected` | observe | 已选 `{provider,model}` | 无 | standard | inline | continue |
| `before_run` | transform | 当前 run 选项 | `patch_run_context`、公共 | standard | inline | keep last value |
| `run_started` | observe | run 启动元数据 | 无 | standard | inline | continue |
| `turn_started` | observe | turn 启动元数据 | 无 | standard | inline | continue |
| `context` | transform | request 组装前的 `{messages}` | `replace_context`、公共 | standard | inline | keep last value |
| `before_model_request` | transform | `{request}` 规范 system/messages/tools/generation draft | `replace_model_request`、公共 | standard | inline | keep last value |
| `before_provider_request_headers` | transform | `{request}` 类型化 provider header request | `replace_provider_headers`、公共；需要 `provider.raw` | standard | inline | keep last value |
| `before_provider_request_raw` | transform | `{request}` 类型化原始 provider payload | `replace_provider_payload`、公共；需要 `provider.raw` | standard | inline | keep last value |
| `provider_response` | observe | `{response}` 规范 provider response 元数据 | 无 | standard | inline | continue |
| `provider_request_failed` | observe | `{failure}` 规范 provider failure | 无 | standard | inline | continue |
| `message_start` | observe | `{message}` 初始 message snapshot | 无 | standard | inline | continue |
| `message_update` | observe | `{message,updateKind}` 流式 snapshot | 无 | fast | bounded coalescing | continue |
| `message_end` | transform | `{message}` 已接受的最终 message | `replace_message`、公共 | standard | inline | keep last value |
| `tool_call` | gate | `{assistantMessage,toolCall,args}` | 公共 | standard | inline | deny |
| `tool_execution_start` | observe | `{toolName,args}` 和 scoped tool-call ID | 无 | standard | inline | continue |
| `tool_execution_update` | observe | `{toolName,update}` 和 scoped tool-call ID | 无 | fast | bounded coalescing | continue |
| `tool_execution_end` | observe | `{toolName,result,isError}` 和 scoped tool-call ID | 无 | standard | inline | continue |
| `tool_result` | transform | `{toolCall,args,result,isError}` | `replace_tool_result`、公共 | standard | inline | keep last value |
| `turn_completed` | observe | `{message,toolResults}` | 无 | standard | inline | continue |
| `run_ended` | observe | `{outcome}` run 终止结果 | 无 | standard | inline | continue |
| `run_error` | observe | `{category,message}` 终止错误 | 无 | standard | inline | continue |
| `run_settled` | observe | 所有 run 清理后的 `{outcome}` | 无 | standard | inline | continue |
| `before_compaction` | gate | `{algorithm,fromHook}` | 公共 | standard | inline | deny |
| `compaction_succeeded` | observe | 已提交的 compaction 元数据 | 无 | standard | inline | continue |
| `compaction_failed` | observe | failure/cancellation 元数据 | 无 | standard | inline | continue |
| `session_shutdown` | observe | session teardown 期间的 `{reason}` | 无 | long | inline | continue |
| `extension_unload` | observe | 实例 dispose 前的 `{reason}` | 无 | long | inline | continue |

### 生命周期顺序

Package 启动顺序是 `discovery → trust evaluation → isolated entry evaluation → setup registrations → state replay/migration → extension_load → session_start`。普通 run 顺序是 `input → before_run → run_started → [turn_started → context → before_model_request → provider request/response → message lifecycle → tool lifecycle → turn_completed]* → run_ended or run_error → run_settled`。会话切换和 fork 在持久操作前放置对应 `before_*` gate，并在 rehydrate 后发送完成 observation。关闭顺序是 `session_shutdown → extension_unload → reverse-order effect disposal → QuickJS instance disposal`。

## 可逆注册

注册作为完整候选集合接受校验。每个已接受 effect 由扩展 ID、session、scope 和 conflict key 标识 owner。Request、run、session、process、显式 handle、故障、重载或关闭结束时，匹配的 effect 按注册逆序 dispose。Dispose 是幂等的。

| API | Descriptor 契约 | 必需 permission |
|---|---|---|
| `registerTool` | `name`、`label`、`description`、JSON `inputSchema`、可选 `resultSchema`、`permission`、`scope` 和 `override` | `tools.register`；override 还需要 `tools.override` |
| `registerCommand` | 小写连字符 `name`、`label`、`description`、对象 `argumentSchema`、可选 `availability` 和 `scope` | `commands.register` |
| `registerProvider` | `providerId`、HTTP(S) `baseUrl`、`format`、可选 `credentialRef`、1–256 个 `models` 和 `scope` | `providers.register`；credential 还需要 `secrets.read:<credentialRef>` |
| `registerPromptSection` | `sectionId`、文本、priority、provider/model/interactive predicate 和 scope | 无 |
| `registerRequestPolicy` | `policyId`、priority、predicate、scope 和 transform handler | 无；action 仍需要对应 capability |
| `contribute` | 带 owner 和 scope 的已校验 contribution envelope | `client.contribute` |

工具冲突默认拒绝。授权的 `override: true` 创建恢复栈，因此 unload 或 dispose 覆盖注册时会恢复被替换工具。命令 handler 返回 `success`、`rejected` 或 `cancelled` outcome，并且不接收终端 API。

Provider `format` 只能是 `openai_chat_completions`、`openai_responses` 或 `anthropic_messages`。每个 model 声明 `id`、`name`、可选 `reasoning`、输入 modality、正数 `contextWindow`，以及不超过 context window 的正数 `maxTokens`。现有 provider serializer 负责 wire protocol；扩展注册元数据而不是可执行 transport 代码。

Availability 和 request predicate 包含可选的精确 `providers`、`models` 集合及 `requiresInteractiveClient`。空集合匹配全部值。Prompt section 在 request policy 前按 priority 追加；policy 作为普通 `before_model_request` transform 执行。

客户端 contribution 使用以下一种声明式 payload：

| Kind | 字段 |
|---|---|
| `notification` | `level`、`title`、`body` |
| `status_item` | `label`、`value`、可选 `detail` |
| `command` | 已校验的 command descriptor |
| `detail_panel` | `title`、对象/数组 `data` |
| `form_action` | `title`、对象 `schema`、`submitCommand` |

客户端只从 transport 数据渲染支持的 kind，并忽略未知 kind。Contribution 和常规 lifecycle status 不会向会话 feed 追加合成 assistant message 或 operation-log 行。

## 持久状态、上下文与迁移

`api.state` 按 key 存储扩展私有、分支局部的值。`api.events.append` 记录命名空间自定义事件，`api.events.replay` 按分支顺序返回事件。`api.modelContext.append` 存储稳定 `contextId` 条目，placement 为 system-prompt section 或 model message；相同 ID 替换其已有 projection，因此恢复上下文严格一次。`api.memory` 是 session 实例局部的临时状态，绝不序列化。

持久写入在一个 hook 内排队。宿主在一次 append 前校验 action kind、owner、ABI、state schema、origin sequence、ID、内容 placement、action 数、条目大小和每扩展 quota。持久化失败会保持 durable projection 和临时 effect 不变。

Resume、分支切换、fork 或 runtime relocation 时，宿主只从所选分支上的条目重建状态和模型上下文。Manifest 提高 `stateSchema` 的 package 注册一个 `api.migrateState(handler)` callback。迁移在普通 session hook 前运行；成功时追加迁移值和迁移 marker，失败时仅将该 package 标为 faulted，并保留历史条目。

## 重载与诊断

Catalog 变更检测和显式重载会在替换 active catalog 前，用隔离 QuickJS 实例校验每个候选。如果 run 或工具执行处于 active，重载会保持 pending，直到 `run_settled` 或工具 settlement 边界；`--cancel` 请求受控取消，但不会在 settlement 前交换实例。应用顺序是旧 cleanup hook、broker 取消、逆序 effect dispose、候选加载、状态 replay/migration、生命周期启动、catalog 发布和 revision 递增。失败候选会保留 active catalog 及其注册不变。

Catalog 状态和诊断通过 gRPC、HTTP JSON-RPC、SSE/WebSocket snapshot、typed client 和 TUI 暴露。诊断包含宿主持有的 extension/session/event/sequence 元数据、稳定 code 与 severity、公开 detail 以及脱敏字段名。Secret 值、原始凭据、扩展私有状态、模型内容和特权 broker 参数不会成为诊断字段。`emit_diagnostic` 只接受公开诊断 payload，action batch 提交后由宿主补充 owner 和关联元数据。

连续三次调用失败会打开 session 局部 circuit breaker。Circuit 打开会禁用受影响 owner、取消其 broker 并反转其 effect；必需 gate 语义仍然 fail-closed。成功调用会重置连续失败计数。

## DeepSeek Anchor 参考 package

参考 package 位于 `extensions/deepseek-anchor`。将该目录复制到项目或全局扩展根目录，审查 `anchor-config.json`、记录信任并重载 catalog。随附配置使用 `zeroAnchor: true`，因此安装后保持 inactive，直到显式启用。

| 配置 | 含义 |
|---|---|
| `providerPredicates` | 与规范 provider ID 匹配的非空 glob 列表。 |
| `modelPredicates` | 与规范 model ID 匹配的非空 glob 列表。 |
| `bootstrapPrompt` | 仅用于匹配且未晋升 request 的最小 system instruction，除非 persona scope 为 session-wide。 |
| `promotionCondition` | `first_assistant`、`first_tool_call` 或 `assistant_or_tool_call`，可附带文本正则和工具名过滤。 |
| `personaScope` | `bootstrap_only` 在晋升后移除 bootstrap persona；`session` 将其前置到之后匹配的 request。 |
| `bootstrapTokenLimit` | 仅 bootstrap 使用的可选正数覆盖；省略时保留 model/request 默认值。 |
| `restoredContext` | 晋升时追加、严格一次的稳定 system-prompt context。 |
| `maxEditorOutputChars` | Editor view 的正数输出裁剪限制。 |
| `zeroAnchor` | 为 true 时绕过所有 transform 和工具注册，但不删除分支局部晋升状态。 |

启用且匹配时，未晋升 request 只接收 `bash` 和兼容 `str_replace_editor`、配置的 bootstrap prompt、空 message 列表，以及未改变的 token limit（除非存在 `bootstrapTokenLimit`）。必需 schema 缺失或不兼容会拒绝完整 transform，使基础 request 保持不变。Editor 只通过 `api.workspace` 实现 `view`、`create`、唯一字面量 `str_replace` 和按行 `insert`。

匹配晋升条件的已接受 finalized assistant message 会在下一次 request 前原子存储幂等晋升 marker、自定义晋升事件和恢复模型上下文。晋升后的 request 保留不可变的完整基础工具 catalog 和留存对话。Resume 与分支切换从所选分支重建 phase；晋升前 fork 保持 bootstrap，晋升后 fork 保持 promoted，并发 session 相互隔离，临时选择不匹配 model 会绕过 Anchor 而不清除 phase 状态。

相同的规范 bootstrap 和晋升行为通过 OpenAI Chat Completions、OpenAI Responses 与 Anthropic Messages provider serializer 验证。DeepSeek 和 Anchor 专属策略完全保留在该 package 中；core 与 provider crate 只暴露通用生命周期及规范 request seam。

## 压缩兼容格式

扩展根目录正下方的顶层 `.ts` 文件可以声明 `export const kind = "compaction"`。该兼容格式支持 `decide_compact`、`select_cut_point` 和 `summarize_prefix`；hook 缺失、返回 null、无效或失败时回退到内置压缩算法。

兼容文件的每个 hook 都在全新受限 QuickJS context 中运行。它们不接收 ABI v2 setup 对象、broker、permission、状态、注册、生命周期 hook 或客户端 contribution。包含 `theway-extension.json` 的 package 目录是所有通用 runtime extension 的格式。

## 验证

```bash
cargo test -p theway-contract --test extension
cargo test -p theway-daemon --test deepseek_anchor_extension
cargo test -p theway-daemon --test ts_extension_dispatcher
cargo test -p theway-daemon --test ts_extension_state
make fmt-check
make file-size-check
make layering-check
make doc-sync
make lint
make test
```
