# 实施任务

> 对应 GitHub issues：#87（父）→ #85、#82、#86、#84、#83。每项完成一个 commit，Conventional Commits + issue 引用。
> 契约层（theway-contract）先行，全部实现不得出现外部产品引用。

## 1. 安装层级三层与最近者优先（issue #85）

- [ ] 1.1 `theway-contract`：`ExtensionSourceLayer` 增加 `Managed` 变体；排序 Managed < Global < Project（**变体名保留 `Global`**，避免 wire/fixture 破坏，文档称 user 层）；catalog entry / trust / diagnostics 同步。
- [ ] 1.2 `catalog.rs`：发现 root 增加 managed 目录（平台托管，daemon 配置传入）；user 目录 = 现有 base 层。
- [ ] 1.3 同名解析改最近者优先（project > user > managed），shadowed 标记与诊断保留；单测覆盖三层遮蔽与移除后自动生效。
- [ ] 1.4 文档：InstallLayer 与 InstanceScope 命名区分。

## 2. 扩展点 kind（issue #82）

- [ ] 2.1 `theway-contract`：注册 DTO 增加 PluginAction（区别于 hook 返回的 ExtensionAction）与 Service 注册；权限新增 `actions.register` / `prompts.register` / `hooks.subscribe` / `services.provide`。
- [ ] 2.2 effect ledger / OwnedRegistration 增加 Action、Service 变体（含 scope 绑定与回卷）。
- [ ] 2.3 顶层单文件 `.ts`：kind 路由支持 `tool` / `action` / `prompt` / `hook` / `service`，合成最小 manifest 进入 package host 链路；kind 与权限绑定；声明与注册不符 → 拒绝 + 诊断。
- [ ] 2.4 `compaction` 与无 kind 行为回归不变；测试覆盖五种 kind 单文件加载与工具/动作/prompt/hook/service 注册生效。

## 3. TS 桥 v2 兼容（issue #86）

- [ ] 3.1 facade：`registerAction`、`registerPromptVariable`、`provide`、`get`、`getConfig`、`effect`、`native(name, args)`、`log`、`runtime`；`registerTool` 双签名。
- [ ] 3.2 入口兼容：`defineExtension` 默认导出与顶层副作用（注入全局桥）两种形态；setup 可收到 config。
- [ ] 3.3 capability 门控：新 API 调用检查声明能力，未声明返回结构化明确错误（含 native 白名单、actions/prompts/services 权限）。
- [ ] 3.4 SDK：`sdks/plugin` 类型声明与 JSON Schema 同步 v2；双入口类型与示例。
- [ ] 3.5 桥测试：每个新 API 的成功路径与未授权拒绝路径；config 合并结果正确。
- [ ] 3.6 注：`emit` / `once` 属事件桥面，由 issue #84 节点（4.1/4.3）落地；本节点只实现 `once` 的 JS 侧封装约定与 API 预留。

## 4. 事件桥（issue #84）

- [ ] 4.1 公共事件名 ↔ 内部 `ExtensionLifecycleEvent` 双向映射表（含 payload 适配）；旧 snake_case 名保留为别名，双名订阅去重。
- [ ] 4.2 新发射点：`session/resume`、`agent/end`、`approval/request`、`approval/resolved`、`workspace/file-write`、`sandbox/exec`、`notification/send`、`agent/status`（按 crate 所有权接线）。
- [ ] 4.3 事件总线：`emit` 发布 + 订阅路由（同会话实例），五种分发模式 `emit/parallel/serial/bail/waterfall` 精确语义落地与行为测试。
- [ ] 4.4 scope 过滤：dispatch 带会话身份，监听按注册作用域过滤；并发会话互不串扰测试。
- [ ] 4.5 活缝（on/emit）与持久通道（events.append/replay）文档分离；订阅随卸载回卷回归测试。

## 5. 会话级生命周期（issue #83）

- [ ] 5.1 实例状态机补 PENDING/LOADING/FAILED/UNLOADING 显式语义（基于现有 phase 演进），依赖未就绪等待。
- [ ] 5.2 JS disposer：`effect()` 队列 + 卸载时逆序执行、逐个隔离；VM 销毁前执行。
- [ ] 5.3 config 注入：manifest `configSchema`、加载时校验 + 默认值、实例配置 < 会话覆盖合并、`getConfig()`。
- [ ] 5.4 服务生命周期联动：inject 等待、提供方消失 → 依赖自动卸载、恢复 → 自动重载；服务值 = JSON 可序列化快照（v1 值语义），提供非 JSON 值返回明确错误。
- [ ] 5.5 失败隔离与热重载回归：apply 抛错只 faulted 自身；HMR 走 dispose→apply 无残留。
- [ ] 5.6 卸载顺序端到端测试：JS disposer 逆序 → Rust ledger 逆序 → VM 销毁。

## 6. 文档与验证

- [ ] 6.1 `docs/extensions.md`（含 zh）与 `sdks/plugin` 全量更新：能力面、公共事件面表、分发模式、生命周期状态机、InstallLayer/InstanceScope、示例。
- [ ] 6.2 端到端验收（design.md 验收标准 1–10 逐条对应测试）；workspace 测试与 clippy/fmt 全绿。
- [ ] 6.3 示例插件覆盖全部新增能力（单文件 kind 与 package 两形态）。
