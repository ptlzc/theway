# theway-tui 架构

[English](architecture.md) | 中文

## 归属与依赖方向

`theway-tui` 负责终端客户端形态和 controller 本地资源。它只使用 `theway-transport` 的记录与服务，并用 `theway-storage` 进行 controller 本地持久化；不导入 agent 运行时或 daemon 应用 crate。

客户端专用行为包括终端布局、键盘处理、feed 渲染、本地 picker 与命令、clipboard 图像、daemon 连接默认值，以及如何通过 controller 服务暴露本地文件/进程。鼠标选择和文本复制由 terminal 或 tmux 负责：TUI 不启用鼠标追踪，也不把选中的 feed 文本写入 clipboard。跨客户端运行时行为从 transport 记录开始，并由 daemon 实现。

## 命令分派

[`main.rs`](../src/main.rs) 在启动终端应用前分流命令。会话导出/导入与独立维护操作可直接打开本地仓库；交互使用委托给 [`startup/mod.rs`](../src/startup/mod.rs)。

[`cli/mod.rs`](../src/cli/mod.rs) 负责参数解析和离线会话命令。[`config_payload.rs`](../src/config_payload.rs) 把本地配置与 CLI 覆盖组装为 `WireDaemonConfig`，并通过 settings RPC 下发给运行中的 daemon。

## Controller 启动

交互启动按以下顺序执行：

1. 启动由 [`local_tool_ops.rs`](../src/local_tool_ops.rs) 支持、以所选工作目录为根的 loopback 工具服务。
2. 启动由 [`controller_storage.rs`](../src/controller_storage.rs) 与本地会话仓库支持的 loopback 存储服务。
3. 把两个服务地址写入 daemon 配置载荷。
4. 通过按工作目录区分的 port 文件或默认端口发现兼容 daemon，再通过限定服务的 gRPC 健康检查验证其已配置的 controller 存储；不存在可用 daemon 时启动 `thewayd` 并等待就绪。
5. 下发配置、获取初始 snapshot、应用客户端负责的新建/恢复选择，并构造 `ui::App`。

`LocalToolOps` 为文件读写编辑、命令执行、目录/搜索、memory 和 skill 安装实现 transport `ToolOps`。`ControllerSessionOps` 与 `ControllerStorageOps` 实现会话生命周期和 DAG/trigger/cron 持久化。这些是 controller 策略，不会把 TUI 变成 agent 运行时。

[`startup/connection.rs`](../src/startup/connection.rs) 在整个应用生命周期内持有 controller 服务 task 和 daemon 启动配置。由另一个 controller 持有的健康 daemon 保留其工具与存储端点组合。若 daemon 的 gRPC 状态端点有响应，但已配置的存储服务无响应，该 daemon 不可用：connector 会启动替代进程并恢复当前会话，而不会连接这个半存活进程。

## 应用状态与事件

[`ui/mod.rs`](../src/ui/mod.rs) 负责 `App`、展示状态、overlay、scroll 状态、composer 状态和最新 transport snapshot。[`ui/app/event_loop.rs`](../src/ui/app/event_loop.rs) 与同级模块拆分事件轮询、frame 应用、渲染、交互、panel、status 和 headless 输出，但不创建新的状态归属层。

[`ui/app/snapshot.rs`](../src/ui/app/snapshot.rs) 应用完整 snapshot 与增量 stream frame。会话标识变化会重置会话级展示 cache。Feed delta 只在预期 base 上应用；不匹配或 lag 后由完整 snapshot 恢复。

事件流关闭时，[`ui/app/event_loop.rs`](../src/ui/app/event_loop.rs) 请求 `DaemonConnector` 重新发现或替换 daemon，并恢复 `App::session_id`。只有新事件流和权威 snapshot 都成功后，UI 才标记为已连接并输出 `reconnected to daemon at …; state synchronized` 或 `daemon restarted at …; restored session …`。有界的客户端连接日志会在后续权威 snapshot 后重新加入这些生命周期消息；失败的重试只进入结构化 debug 日志，不会每秒向 feed 添加一行。

用户动作调用带类型的 `GrpcClient` 方法或入队 transport 命令。UI 不调用 `AgentHarness`、图引擎内部模块或 daemon 私有服务。

## Feed 与 composer 渲染

[`feed_cache.rs`](../src/feed_cache.rs) 缓存已渲染 feed line，维护有界窗口，并对仅追加的 assistant/thinking block 增量渲染；非追加编辑回退到完整渲染。[`feed_render.rs`](../src/feed_render.rs) 把 transport feed block 映射为主题化 ratatui line、代码块 span 和链接 overlay。

Assistant Markdown 直接渲染，不添加 `ai` 角色前缀。`Ctrl+O` 与 `Ctrl+T` 只修改客户端展示状态中的 thinking 可见性和工具结果展开状态；这些本地展示操作不会向 feed 追加 system 行。

Turn 忙碌期间，状态区域高度为三行，[`ui/snake_loader.rs`](../src/ui/snake_loader.rs) 渲染由九个圆点组成的稳定 3×3 网格。彩虹蛇头及渐隐尾迹按行蛇形顺序 `0,1,2,5,4,3,6,7,8` 往返；实时字符速率计从 130 ms 到 10 ms 的五档动画间隔中选择速度，并把尾迹从两个点延长到五个点。空闲状态仍占一行。

[`ui/app_input.rs`](../src/ui/app_input.rs) 与 [`ui/app_input/history.rs`](../src/ui/app_input/history.rs) 负责 composer 输入、补全、历史、粘贴和提交。编辑器状态来自 `theway-ratatui-textarea`；终端渲染辅助、链接与 scrollbar 行为来自 `theway-pager-render`。

[`theme.rs`](../src/theme.rs) 是终端外观归属。颜色、间距、前缀、加载指示与 panel 布局不进入 daemon snapshot 或 core event。

## 离线持久化

离线会话导出、导入、列举和删除直接使用 `theway-storage`，因为它们在活动 turn 之外操作本地产物。交互会话创建、切换、重命名和删除使用协议操作，使 daemon 与客户端状态保持串行一致。

## 不变量

- 本 crate 不依赖 `theway-core` 或 `theway-daemon`。
- Controller 工具和存储服务绑定 loopback，并显式下发给 daemon。
- 复用 daemon 要求 daemon 状态 RPC 与其已配置的 controller 存储 RPC 都可用；只有协议端点响应并不充分。
- 连接由另一个存活 controller 持有的 daemon 时，不替换该 controller 的工具或存储端点。
- 运行时状态来自 transport snapshot/event；UI cache 是可重置派生状态。
- 客户端外观与输入选择不会进入共享 wire 类型，除非其他客户端也需要相同行为。
- 交互会话变更使用 transport 操作；直接 SQLite 访问仅限 controller storage 实现和离线维护。
- 流式与一次性 feed 渲染收敛到相同可见内容及 source/link mapping。
