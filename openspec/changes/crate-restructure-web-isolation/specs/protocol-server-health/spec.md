# Protocol Server Health

规范 `theway-server` 的健康检查行为与"纯协议、零前端"约束。

## ADDED Requirements

### Requirement: HTTP 健康检查端点

`theway-server` 的 HTTP 服务器 SHALL 提供 `GET /healthz` 端点,用于 liveness 探测。该端点 MUST NOT 依赖业务状态 (会话、模型、DAG 引擎),进程能响应请求即返回 200。响应体 SHALL 为固定短文本 (如 `ok`),Content-Type 为纯文本。

#### Scenario: 健康检查成功

WHEN 客户端请求 `GET /healthz`
THEN 服务器返回 HTTP 200
AND 响应体为固定短文本,不包含业务快照

#### Scenario: 健康检查不携带业务负载

WHEN 引擎无会话、无 DAG run 时请求 `/healthz`
THEN 依然返回 200
AND 响应不包含会话/DAG 状态,不触发快照构建

### Requirement: gRPC 标准健康服务

`theway-server` 的 gRPC 服务器 SHALL 实现标准 `grpc.health.v1.Health` service (`Check`/`Watch`),供 gRPC 生态的 probe (如 K8s grpc probe) 使用。`Check` SHALL 对健康状态返回 `SERVING`。

#### Scenario: gRPC 健康检查

WHEN 客户端调用 `grpc.health.v1.Health/Check` (空请求)
THEN 返回 `status: SERVING`

#### Scenario: gRPC health 与业务 RPC 共存

WHEN gRPC 服务器同时暴露 `ThewayGrpc` 业务服务与 `grpc.health.v1.Health`
THEN 两个 service 在同一端口共存,互不干扰

### Requirement: 根路径不提供 UI

`theway-server` 的 HTTP 服务器 MUST NOT 在 `/` 提供 HTML 页面或任何前端资源。`GET /` SHALL 返回 404 (未注册路由) 或非 HTML 的协议说明文本;服务器发布产物 MUST NOT 包含 HTML/CSS/JS 前端文件。

#### Scenario: 根路径无内嵌 UI

WHEN 客户端请求 `GET /`
THEN 返回 404 (或非 HTML 响应)
AND 响应中不包含内嵌的前端页面内容

#### Scenario: 仓库无前端资源

WHEN 检查 `crates/server` 目录
THEN 不包含 `.html`/`.js`/`.css` 前端文件
AND `web_index.html` 不存在于任何 crate 的源码树

### Requirement: 启动引导日志

CLI (`theway-cli`) 启动 `--web`/`--grpc` 模式时,SHALL 在 stdout 打印:监听地址、可用端点清单 (HTTP: `/state` `/events` `/ws` `/healthz`;gRPC: 服务名 + health) 与浏览器 UI 的指引 (指向独立前端,如 workmate / theway-console,而非内嵌页面)。

#### Scenario: 启动日志引导

WHEN 用户以 `--web` 启动并绑定 `127.0.0.1:8080`
THEN stdout 输出包含 `listening on http://127.0.0.1:8080`
AND 输出包含端点清单 (`/state`、`/events`、`/healthz` 等)
AND 输出说明浏览器 UI 位于独立前端项目 (不指向内嵌页面)

### Requirement: 前端独立于服务器发布

浏览器端 UI 的构建、发布与生命周期 SHALL 独立于 `theway-server`。服务器升级 MUST NOT 携带前端版本变更;前端升级 MUST NOT 要求服务器重新发布。

#### Scenario: 前后端独立发布

WHEN `theway-server` 发布新版本
THEN 其发布产物不包含任何前端文件,前端由独立仓库/目录提供
AND 前端与服务器的契约仅为 gRPC/HTTP/WS 协议
