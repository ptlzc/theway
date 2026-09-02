## Why

theway 的 runtime extension 体系（`runtime-extension-lifecycle` 已落地）已具备 package manifest、会话级持久 QuickJS host、observe/transform/gate/register 语义、effect ledger、信任与安全边界。但相对一个完整的插件体系仍有明确缺口：

- 顶层单文件 `.theway/extensions/*.ts` 只有 `kind = "compaction"` 一个扩展点，无法声明 tool / action / prompt / hook / service。
- 插件可见的事件面与内部 `ExtensionLifecycleEvent` 直接耦合：事件名是内部 snake_case 枚举名，没有稳定的公共事件面、没有 `emit`（发布）、没有明确的五种分发语义。
- 桥 API 缺 `registerAction`、`registerPromptVariable`、`emit`、`provide`/`get`、`getConfig`、`effect`、`native`、`log`、`runtime`。
- 插件实例生命周期缺 config 注入与校验、JS 侧 disposer 逆序执行、依赖未就绪的 PENDING 语义。
- 安装层级只有 Global/Project 两层，缺 platform-managed 层与"最近者优先"的显式解析规则。

本 change 冻结 theway 插件体系的**最终目标设计**（能力面、事件面、生命周期、scope、桥 API），作为 #82–#86 的实施蓝图；spec delta 与代码实现按各子 issue 单独落地。

## What Changes

- 在顶层 `.theway/extensions/*.ts` 上新增扩展点 kind：`tool` / `action` / `prompt` / `hook` / `service`，`compaction` 保留为兼容格式；kind 与权限集绑定，未声明 kind 拒绝加载。
- 建立稳定的**公共事件面**（`namespace/action` 命名空间事件 + 平铺 payload），与内部 `ExtensionLifecycleEvent` 通过桥层双向映射；旧 snake_case 内部名保留为别名。补齐缺失发射点：session resume、agent end、approval 流、workspace file-write、sandbox exec、notification send、agent status。
- 事件总线支持五种分发模式 `emit` / `parallel` / `serial` / `bail` / `waterfall`，监听按作用域过滤，订阅随插件卸载自动回卷；持久事件通道（append/replay）与活的事件缝语义分离。
- TS 桥升级为 v2 完整面：`registerTool` / `registerAction` / `registerPromptSection` / `registerPromptVariable` / `on` / `once` / `emit` / `provide` / `get` / `getConfig` / `effect` / `native` / `log` / `runtime` / `capabilities.has`；能力差异一律通过 capability 门控暴露为明确错误或降级，不要求插件按宿主维护多套源码。
- 插件生命周期演进为显式状态机：PENDING（依赖未就绪等待）→ LOADING（apply）→ ACTIVE → UNLOADING → DISPOSED（或 FAILED）；`apply(ctx, config)` 入口（`defineExtension` 与顶层副作用两种形态）；卸载按固定顺序执行 JS disposer（逆序）与 Rust effect ledger（逆序）；单插件失败只隔离自身。
- 安装层级扩展为三层 `managed` / `user` / `project`，同名插件按最近者优先解析（project > user > managed）；安装层级与实例作用域（process/session/run/request）正交命名。
- manifest 增加可选 `configSchema`；配置按 默认值 < 实例配置 < 会话覆盖 合并，`getConfig()` 返回合并结果。
- 服务模型：`provide(name, service)` / `get(name)` / `inject`，依赖未就绪时插件保持 PENDING，提供方消失时依赖插件自动卸载、恢复后自动重载。
- 全量更新 `docs/extensions.md` 与 `sdks/plugin` 声明，示例插件覆盖全部新增能力。

## Capabilities

### New Capabilities

- `plugin-install-scopes`: managed / user / project 三层发现与最近者优先解析。
- `plugin-extension-kinds`: 顶层单文件扩展点 kind（tool/action/prompt/hook/service）的分发与权限绑定。
- `plugin-ts-bridge-v2`: TS 桥 v2 完整 API 面、双入口/双签名兼容、capability 门控。
- `plugin-event-bridge`: 公共事件面、五种分发模式、作用域过滤与事件名映射。
- `plugin-session-lifecycle`: 插件实例状态机、config 注入、disposer 逆序、失败隔离与热重载。
- `plugin-config-and-services`: 配置 schema 与合并、provide/get/inject 服务模型。

### Modified Capabilities

- 无（新增能力在 `runtime-extension-lifecycle` 已建成的 package host / effect ledger / 信任边界之上叠加，不改动既有单 ABI 契约）。

## Impact

- `crates/theway-contract`: 安装层级第三层、公共事件名映射与新事件变体、Action/Service 注册 DTO、`configSchema` 字段、新权限（`actions.register` / `services.provide` 等）。
- `crates/theway-core`: 按所有权补齐新发射点端口（session resume/end、approval、agent status）；不发现或执行插件。
- `crates/theway-daemon`: `ts_extensions` 演进——事件名映射层、事件总线、服务注册表、config broker、JS disposer、native/log/runtime 桥、三层 catalog、kind 路由。
- `crates/theway-transport`: 如需向客户端暴露目录层级/状态时加 wire 字段；不承载插件执行。
- `sdks/plugin`: 声明与 schema 同步 v2 API 面。
- 参考 GitHub issues：#87（父）、#82、#83、#84、#85、#86。
