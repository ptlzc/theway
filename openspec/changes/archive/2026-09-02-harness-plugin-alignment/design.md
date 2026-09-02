## Context

`crates/theway-daemon/src/ts_extensions/` 已建成一套完整的 runtime extension 体系：package manifest（`theway-extension.json`）、持久 QuickJS 实例池、注册 effect ledger（逆序回卷）、observe/transform/gate/register 四类 hook 语义、circuit breaker、project trust 与 capability broker。顶层单文件 `.theway/extensions/*.ts` 作为 legacy 格式仅支持 `kind = "compaction"`。

这套体系的内部语义（注册即 effect、卸载回卷、失败隔离、确定性排序）与主流 agent-harness 插件设计一致，但**插件作者可见的表面**仍停留在内部实现细节上：

1. **没有公共事件面**：TS 插件只能按内部 `ExtensionLifecycleEvent` 的 snake_case 枚举名订阅（`turn_started`、`tool_execution_start`…），事件名与 Rust 内部重构耦合；没有稳定的 `namespace/action` 公共名；没有发布（`emit`）；五种分发模式没有对插件显式暴露。
2. **桥 API 不全**：有 `registerTool/Command/Provider/PromptSection/RequestPolicy/contribute/on` 与安全 broker（workspace/process/network/secrets/state/events/modelContext/memory），缺 action、prompt variable、服务、config、disposer、native 白名单、log、runtime 信息。
3. **生命周期不完整**：实例 phase 只有 Loaded→Started→Disposed，无 config 注入/校验、无依赖未就绪等待（PENDING）、JS 侧注册的 disposer 在 VM 销毁时不会执行。
4. **安装层级只有两层**：Global（`<base>/extensions`）与 Project（`.theway/extensions`），缺平台托管层与显式最近者优先规则。
5. **扩展点只有一种**：单文件插件无法声明 tool/action/prompt/hook/service 扩展点。

本 design 冻结最终目标：在既有内部语义之上定义稳定的插件作者表面。公开模型参考了主流 agent harness 的插件设计（函数式 `apply(ctx, config)` 插件、fiber 状态机、注册即 effect、事件总线与服务注入），全部以 theway 自身命名落地。

## Goals / Non-Goals

### Goals

- 一份稳定的、与内部事件解耦的公共事件面（`namespace/action`），含 payload 与分发语义，覆盖会话、turn、工具、请求、审批、工作区、沙箱、通知、插件自身与会话结构事件。
- TS 桥 v2 完整能力面；能力差异只通过 capability 门控体现。
- 显式插件生命周期状态机：apply(ctx, config)、依赖等待、disposer 逆序回卷、失败隔离、热重载复用同一条 dispose→apply 路径。
- managed / user / project 三层安装与最近者优先解析；与实例作用域正交。
- 保留现有 ABI：旧 manifest、旧内部事件名（作为别名）、`kind = "compaction"`、`defineExtension` 入口全部继续可用。

### Non-Goals

- 把内部 Rust 事件（`LoopEvent`/`SessionEvent` 等）直接暴露给插件。
- 在本次设计中引入第二个执行引擎或跨进程插件 RPC（仍为进程内 QuickJS）。
- 让插件控制终端渲染或加载 UI 代码。
- 移除内置工具或替换现有 provider/model 目录。
- 定义一个新的持久事件日志协议（沿用现有 durable entry 机制）。

## Decisions

### 1. 插件最终能力面（TS 桥 v2）

`api`（setup 参数）与注入的全局桥提供以下能力，全部通过 capability 门控（manifest permissions / 运行时声明）约束；未声明能力的调用返回**明确错误**（结构化 error，绝不静默）。

| API | 语义 | 现状 → 目标 |
| --- | --- | --- |
| `registerTool(descriptor)` / `registerTool(name, desc, schema, fn)` | 注册 Agent 工具 | 已有 → 双签名兼容 |
| `registerAction(name, fn)` | 注册动作（平台/命令通道调用） | **新增** |
| `registerCommand(descriptor, handler)` | 注册 daemon 命令（`/ext:`） | 已有 |
| `registerProvider(descriptor)` | 声明式 provider/model | 已有 |
| `registerPromptSection(descriptor)` | 有序 Prompt 段 → system prompt 装配 | 已有 → 签名对齐 |
| `registerPromptVariable(...)` | 注册 Prompt 变量 | **新增** |
| `registerRequestPolicy(descriptor, handler)` | 请求策略 | 已有 |
| `contribute(descriptor)` | 客户端中性贡献 | 已有 |
| `on(event, handler[, opts])` / `once(...)` | 订阅公共事件（返回 disposer） | 已有 → 公共事件名 + prepend/once |
| `emit(event, payload)` | 发布事件（活的事件总线） | **新增** |
| `provide(name, service)` | 提供服务 | **新增** |
| `get(name)` | 读取服务（可选依赖现场查询） | **新增** |
| `effect(disposer)` | 注册清理函数（卸载时逆序执行） | **新增** |
| `getConfig()` | 读取合并配置 | **新增** |
| `native(name, args)` | 白名单宿主原生能力（如 `notify` / `httpRequest`） | **新增**（替代按域对象的路由入口，按域对象保留） |
| `log(level, msg)` | 结构化日志（进审计） | **新增** |
| `runtime` | `{ version, pluginId, sessionId }` | **新增** |
| `capabilities.has(permission)` | 宿主能力探测 | 已有 |
| workspace/process/network/secrets/state/modelContext/memory/events.replay | 安全 broker 能力 | 已有 |
| `migrateState(handler)` | 状态迁移 | 已有 |

入口兼容两种插件形态：`defineExtension((api, config) => …)` 默认导出（现有），以及顶层副作用式 `register()`（桥注入全局、入口直接调用）；二者都视为一次 `apply(ctx, config)`。

### 2. 扩展点 kind

顶层单文件 `.theway/extensions/*.ts` 通过 `export const kind` 声明扩展点；新增五种 kind，`compaction` 保留：

| kind | 语义 | 绑定权限 | 实现路由 |
| --- | --- | --- | --- |
| `tool` | 注册 Agent 工具 | `tools.register` | 合成最小 manifest → package host |
| `action` | 注册动作 | `actions.register`（新增） | 同上 |
| `prompt` | 注册 Prompt section / variable | `prompts.register`（新增） | 同上 |
| `hook` | 订阅事件 / lifecycle hook | `hooks.subscribe`（新增） | 同上 |
| `service` | 提供服务 | `services.provide`（新增） | 同上 |
| `compaction` | 压缩算法（legacy） | 无（现状） | legacy 路径不变 |

kind 声明与文件实际注册内容不符（如 `kind = "tool"` 但未调用任何注册）→ 结构化诊断并拒绝该文件，不拖垮其它文件。package 形态（带 manifest 的目录）不受 kind 限制，可自由组合多种注册。

### 3. 公共事件面（事件桥）

**命名规则**：公共事件名采用 `namespace/action` 小写命名空间风格；payload 平铺、字段稳定；插件只依赖公共名，不依赖内部事件枚举。

**事件名映射层**：桥层维护公共名 ↔ 内部 `ExtensionLifecycleEvent` 双向映射表（订阅解析与投递外发都过映射层）；旧的内部 snake_case 名保留为订阅别名（同一事件双名订阅去重）。内部 enum 不删除、不改名，公共面只是其上的稳定投影 + 新发射点。

**最终公共事件面**（payload 均为平铺 JSON；模式列给出该事件的主要分发语义，全部事件都允许 observe）：

| 分组 | 公共事件 | 模式 | 主要 payload | 内部映射 / 发射点 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 会话 | `session/start` | emit | session_id、cwd、scope | SessionStart | 已有 |
| 会话 | `session/resume` | emit | session_id、cwd | **新发射点**（会话恢复） | 新增 |
| 会话 | `agent/end` | emit | session_id、turn_id | **新发射点**（run settle 后） | 新增 |
| Turn | `before_turn` | emit | session_id、turn_id、input | TurnStarted | 改名外发 |
| Turn | `after_turn` | emit | session_id、turn_id、result | TurnCompleted | 改名外发 |
| 工具 | `before_tool_call` | emit | tool_name、args、scope | ToolExecutionStart | 改名外发 |
| 工具 | `after_tool_call` | emit | tool_name、result、scope | ToolExecutionEnd | 改名外发 |
| 工具 | `tools/result` | emit | tool_name、result | ToolResult（observe 口；transform 替换能力仍可注册） | 改名外发 |
| 请求 | `agent/request` | waterfall | 规范化请求快照 | BeforeModelRequest（transform） | 改名外发 |
| 审批 | `approval/request` | waterfall | approval_id、kind、scope | **新发射点**（ask 决策 → 认领/委托） | 新增 |
| 审批 | `approval/resolved` | emit | approval_id、outcome | **新发射点** | 新增 |
| 工作区 | `workspace/file-write` | emit | path、operation | **新发射点**（文件写工具/workspace broker） | 新增 |
| 沙箱 | `sandbox/exec` | emit | command、cwd | **新发射点**（process.run broker） | 新增 |
| 通知 | `notification/send` | emit | notification_id、channel | **新发射点**（通知通道） | 新增 |
| 状态 | `agent/status` | emit | status | **新发射点**（agent 状态快照） | 新增 |
| 插件 | `plugin/loaded` | emit | plugin_id | ExtensionLoad | 改名外发 |
| 插件 | `plugin/disposed` | emit | plugin_id | ExtensionUnload | 改名外发 |
| 会话结构 | `compaction` | emit | session_id、before_tokens、after_tokens | CompactionSucceeded（CompactionFailed 经内部别名可订阅） | 改名外发 |
| 会话结构 | `branch` | emit | session_id、branch_id | SessionForked | 改名外发 |
| 会话结构 | `chat/composition_selected` | emit | chat_id、composition_id | ModelSelected | 改名外发 |

**五种分发模式**（精确语义，向插件显式暴露）：

| 模式 | 语义 | 说明 |
| --- | --- | --- |
| `emit` | 广播，忽略监听者返回值 | 观察语义 |
| `parallel` | 所有监听者并发执行，全部 settle 后 resolve | — |
| `serial` | 按注册顺序依次 await，直到某个监听者返回 bail 值（非 null/false/undefined）即停止 | 首个 bail 值即结果 |
| `bail` | 同步按序调用，首个 bail 值短路 | 门控/策略语义 |
| `waterfall` | 洋葱中间件：监听者收到 `(payload, next)`，必须调 `next()` 才放行下游；不调 = 短路（拦截/网关） | 变换语义 |

模式与内部四类 hook 语义的对应：observe→emit/parallel、serial/bail→gate 风格短路、waterfall→transform 链；事件总线上每种公共事件按表注册默认模式，监听者可声明 class/优先级（沿用现有 descriptor）。

**作用域过滤（scope-filtered dispatch）**：每个 dispatch 携带会话/agent 作用域身份；监听按其注册作用域只收到匹配事件（两个并发会话的同名监听互不串扰）。安装层与实例作用域都不改变事件过滤语义。

**两条通道分离**：活的事件缝（`on`/`emit`，进程内实时、随卸载回卷）与持久自定义事件（`events.append` + `events.replay`，进 session log、可重放）语义不同，文档明确区分；公共事件面属于前者。插件 `emit` 的自定义事件经内部 `Custom` 事件变体路由到同会话实例，不写持久日志。

### 4. 安装层级与实例作用域

**安装层级（InstallLayer，决定"哪个插件生效"）**：

| 层 | 目录 | 说明 |
| --- | --- | --- |
| `managed` | 平台托管目录（安装/发行目录，`<base>/extensions-managed`） | 随发行自带，用户只读 |
| `user` | `<base>/extensions` | 用户级（现状 Global 层；**内部变体名保留 `Global` 以兼容 wire 与 fixtures**，文档与诊断称 user 层） |
| `project` | `<cwd>/.theway/extensions` | 项目级（现状 Project 层） |

同名插件按**最近者优先**解析：project > user > managed；被遮蔽者保留 catalog 记录并标记 shadowed。`ExtensionSourceLayer` 增加 `Managed` 变体；三层目录均参与信任评估与诊断。

**实例作用域（InstanceScope）**：manifest `scope`（process/session/run/request）继续约束注册效果的存活边界，与安装层级正交。两个概念在文档、诊断与类型命名上严格区分，杜绝混用。

### 5. 会话级生命周期

插件实例最终状态机：

```
PENDING → LOADING → ACTIVE
              ↘ FAILED      （apply 抛错 → 只挂该插件，其余照常）
ACTIVE → UNLOADING → DISPOSED
```

| 状态 | 语义 |
| --- | --- |
| PENDING | 已发现且信任通过，但声明的依赖服务未就绪（等待） |
| LOADING | 依赖就绪，config 校验通过，正在执行 apply |
| ACTIVE | 运行中，事件/注册正常派发 |
| FAILED | apply 或运行期致命失败；记录诊断，效果回卷 |
| UNLOADING | 正在执行卸载序列 |
| DISPOSED | 完全卸载 |

**apply（进入 LOADING→ACTIVE）**：实例化持久 VM → 注入合并 config（见 §6）→ 执行顶层入口（`defineExtension` 或顶层副作用，均可收到 config）→ 注册校验（公共事件名/class/优先级/权限逐项核对，未知名返回明确错误）→ 外发 `plugin/loaded`。

**dispose（UNLOADING→DISPOSED，固定顺序）**：
1. 外发 `session/end` 上下文事件与 `plugin/disposed`（若处于 Started）；
2. 执行 JS 侧 disposer 队列（`effect()` 注册项）——**逆序**、逐个隔离（单个失败只记诊断）；顺序敏感的清理在单个 effect 内自行串行；
3. Rust effect ledger 按注册逆序回卷（工具覆盖还原、订阅移除、服务注销、状态清理）；
4. 销毁 VM 实例。

失败隔离：初始化抛错 → 标记 faulted + 诊断，跳过该插件继续加载其余；运行期连续失败 → circuit breaker 熔断 → 同一回卷序列。会话内热重载（源码/信任/配置变更）复用同一条 dispose→apply 路径，不产生旧实例残留。

**依赖驱动生命周期**：`inject` 声明必需服务；服务未就绪 → PENDING；运行中服务消失（提供方卸载）→ 依赖插件自动 UNLOADING → 服务恢复后自动重载。可选依赖用 `get(name)` 现场查询（可能为 undefined）。

### 6. 配置

- manifest 新增可选 `configSchema`（JSON Schema）；缺失时插件无配置（`getConfig()` 返回空对象或 null）。
- 加载时由宿主按 schema 校验并填默认值，非法配置**响亮失败**（该插件 FAILED，不静默降级）。
- 合并优先级：schema 默认值 < 实例配置（manifest/安装层提供）< 会话级覆盖；`getConfig()` 返回合并结果。
- 插件规则："两个部署可能设置不同的值必须是配置字段"，不硬编码可调值。

### 7. 服务模型

- 服务 = 挂在会话 host 上的具名能力；任意插件可 `provide(name, service)`（返回 disposer，卸载自动注销；同名冲突返回明确错误），其它插件 `get(name)` 或 `inject` 消费。
- **v1 值语义**：服务值是 JSON 可序列化快照（跨 QuickJS VM 不传递 live 对象）；`get(name)` 返回深拷贝，方法调用形态不在 v1 范围，后续以 broker 化服务调用扩展。
- 同一会话内多个 host 实例天然隔离；`isolate` 语义不在 v1 范围（会话即隔离边界）。
- `provide` 的注册内容进入 effect ledger，随实例作用域与卸载回卷。

### 8. 兼容与迁移

- `theway-extension.json` 契约不变（新增可选字段向后兼容）；`abi` 等未知字段仍拒绝。
- 内部 snake_case 事件名作为订阅别名永久可用；公共事件面只增不改（新事件为新增变体/发射点）。
- `kind = "compaction"` 单文件、`defineExtension` 单参数形态继续可用。
- `docs/extensions.md`、`sdks/plugin` 声明与示例随实现同步更新，覆盖全部新增能力；示例同时演示单文件 kind 与 package 两种形态。

## 验收标准（端到端）

1. 同一份 TS 插件工程（使用 `registerTool`、`registerAction`、`on('tools/result')`、`effect`、`getConfig`）以 package 形态与 `kind = "tool"` 单文件形态都能加载，注册的工具名、参数 schema 与执行行为一致。
2. 插件订阅 `tools/result`、`agent/request`、`approval/request` 等公共事件，在对应生命周期点收到事件且 payload 字段与本文档一致；旧 snake_case 别名订阅同一事件不重复触发。
3. `emit` 的五种分发模式各有行为测试（bail 短路、serial 首个 bail、waterfall 必须 `next()`、parallel 并发、emit 忽略返回值）。
4. 两个并发会话监听同一事件互不串扰（scope 过滤）。
5. project、user、managed 三层同名插件：最近者优先生效，被遮蔽者带 shadowed 诊断；移除 project 层后 user 层自动生效。
6. 插件 `apply` 抛错：该插件 faulted + 诊断，其余插件正常；会话结束执行完整回卷顺序（JS disposer 逆序 → Rust ledger 逆序 → VM 销毁），disposer 失败不影响其它清理。
7. `inject` 服务未就绪 → PENDING；提供方卸载 → 依赖自动卸载；恢复 → 自动重载。
8. 未声明 capability 的 API 调用（如未授权 `native`、`actions.register`）返回明确错误，不静默执行。
9. 非法 config 使该插件 FAILED 并给出可操作错误；合法 config 按 默认值 < 实例 < 会话覆盖 合并。
10. `kind = "compaction"` 旧格式与既有 package 插件行为回归不变。
