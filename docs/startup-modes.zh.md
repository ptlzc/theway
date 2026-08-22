# 启动模式

[English](startup-modes.md) | 中文

本文档说明 `theway` 与 `thewayd` 的启动方式，以及它们的生命周期关系。

## 进程

- `theway` 是终端客户端/控制器。
- `thewayd` 是拥有 agent runtime 的无头守护进程。
- 客户端通过 per-cwd 端口文件发现正在运行的 daemon，存在则复用，不存在则启动一个。

## 两种生命周期模式

### 附加模式（`theway` 启动 daemon 时的默认行为）

当 `theway` 启动且没有可用 daemon 时，它会启动控制器服务（tool service 和 storage service），并携带控制器存储地址启动 `thewayd`。

此时 daemon 是 controller-backed：它会持续探测控制器存储服务，并在该服务消失时关闭。

结果是 daemon 生命周期与 TUI 绑定。TUI 退出后，daemon 会在很短时间内停止。

### 独立后台模式

当手动启动 `thewayd` 且不携带控制器存储地址时，它使用本地存储并独立运行。

它不会因为任何客户端退出而停止，之后运行的 `theway` 可以发现并复用它。

这是长期后台 daemon 应使用的模式。

## 提议的 CLI：`theway --daemon`

为了让后台模式在客户端侧更明确，给 `theway` 增加 `--daemon` 参数：

- `theway`（默认）：启动一个与 TUI 共享生命周期的附加 daemon。
- `theway --daemon`：启动一个生命周期独立的后台 daemon。当前 TUI 连接它，之后的 TUI 运行复用它。

在实现上，`theway --daemon` 启动 `thewayd` 时不携带控制器存储地址，并且应当 detach daemon 进程，使其在终端会话关闭后仍能存活。

## 实现说明

- 手动使用 `thewayd` 时保持默认独立模式。
- 当前的 `--storage-service-addr` 参数继续作为 controller-backed 存储的内部/wire 机制，但面向用户的生命周期开关应放在 `theway` 上。
- 为了在终端或 SSH 会话关闭后真正后台存活，被启动的 daemon 应当 detach：
  - Unix：使用 `setsid`、忽略 `SIGHUP`，并重定向标准流。
  - Windows：使用 `DETACHED_PROCESS` 或 `CREATE_NEW_PROCESS_GROUP`（或等价方式）。

## 模式对比

| 模式 | 命令 | Daemon 存储 | TUI 退出后 | 下次 TUI |
|---|---|---|---|---|
| 附加模式 | `theway` | controller-backed | daemon 停止 | 启动新 daemon |
| 手动独立模式 | `thewayd --cwd ...` | 本地 | daemon 继续运行 | 复用 daemon |
| 客户端独立模式（提议） | `theway --daemon` | 本地 | daemon 继续运行 | 复用 daemon |
