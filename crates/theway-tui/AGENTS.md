# theway-tui 修改规则

本文件适用于 `crates/theway-tui/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改交互或启动行为前，先阅读 [crate 概览](README.md)、[client/controller 架构](docs/architecture.md)和 [daemon 定位规则](../../AGENTS.md#daemon-positioning)。

## 边界规则

- 不得添加对 `theway-core` 或 `theway-daemon` 的依赖；使用 [`theway-transport`](../theway-transport/README.md) 的消息、客户端和操作 trait。
- 终端颜色、布局、按键、鼠标、loader、picker、clipboard 和 feed 展示留在本 crate。
- 跨客户端状态先在 transport 定义；不得推断 daemon 内部状态，也不得向运行时 snapshot 添加 TUI 专用字段。
- 直接 SQLite 访问只放在 controller storage 和离线会话命令；交互运行时变更使用协议操作。

## Controller 与状态规则

- 连接或启动 daemon 前先启动 controller `ToolService`、`StorageService`，绑定 loopback，并显式下发其地址。
- `LocalToolOps` 以所选工作目录为根，并保留请求校验、输出上限、超时和路径行为。
- 完整 snapshot 是权威状态；校验 delta base，并在会话标识变化时重置会话级 cache。
- 流式 feed cache 是可丢弃派生状态；非追加编辑回退到完整渲染。
- 可在应用 shell 外复用的渲染原语放在专用 markdown、pager 或 textarea crate。

## 测试与文档

- 使用本地 tonic fixture 和 fake operation trait；测试不得启动 provider 调用或依赖用户会话目录。
- 在归属模块覆盖键盘/鼠标动作、snapshot lag 与会话变化、daemon 复用/启动、controller 服务、离线命令和宽度敏感渲染。
- 镜像套件遵循 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md)。
- 启动顺序、controller 归属、状态应用、渲染或离线/交互边界变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-tui`、`cargo doc -p theway-tui --no-deps --document-private-items` 和 `make layering-check`。
