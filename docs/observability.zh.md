# 运行时可观测性配置

[English](observability.md) | 中文

本文档描述 `thewayd` 运行时 observer 的配置方式：OpenTelemetry OTLP 导出、Prometheus 指标、结构化操作日志，以及供 Langfuse 使用的可选完整上下文载荷。机制实现在 [`crates/theway-daemon/src/observability.rs`](../crates/theway-daemon/src/observability.rs)，观测数据由 [`crates/theway-core/src/observability.rs`](../crates/theway-core/src/observability.rs) 产生。

## 配置来源

所有设置在 daemon 启动时由 `TelemetryConfig::from_env` 读取一次，并合并两个来源：

1. `thewayd` 进程自身的环境变量。
2. 运行时环境文件 `$THEWAY_DIR/observability.env`，默认位置为 `~/.theway/observability.env`。

进程环境变量优先于文件。文件是纯文本 `KEY=value` 列表：空行和 `#` 注释会被忽略，值两端的双引号会被剥离，格式错误的行会被丢弃。由于 daemon 自己读取该文件，任何启动方（TUI、workmate、脚本）无需继承 shell 导出即可获得可观测性配置。

## OTLP traces

OpenTelemetry trace 导出器使用 OTLP HTTP/protobuf，并在以下任一变量非空时构建：

| 变量 | 含义 |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | 通用端点。导出器发送 span 前会追加 `/v1/traces`。 |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | traces 专用端点，按原样使用。必须包含信号路径，例如 `https://collector.example.com:4318/v1/traces`。 |

Header 从 `OTEL_EXPORTER_OTLP_HEADERS` 读取，`OTEL_EXPORTER_OTLP_TRACES_HEADERS` 优先。格式为 `k1=v1,k2=v2`；只有第一个 `=` 用于分割键值，因此含 `=` 的 Basic 认证 token 会完整保留。

每个操作都会成为一个 span。子操作与父操作共享 trace 和 span id，因此导出数据是每次 agent run 一棵事件树：`agent.run` → `agent.turn` → `llm.request` / `tool.execute`，以及 `session.compaction`、`multiagent.job`、`dag.run` 和 `dag.node`。

Span 属性包括操作 id、父 id、session/run/job/node/turn 关联值、工具名、provider/model、outcome、耗时、错误类别和 token/cache 用量。SDK resource 为 `service.name=thewayd`、工作区版本号以及进程级的 `service.instance.id`。

## OTLP metrics

指标导出器与 traces 相互独立。仅当 `OTEL_EXPORTER_OTLP_ENDPOINT` 或 `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` 非空时才会构建；只消费 traces 的场景不会启动一个指向默认端点的指标导出器。

`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` 按原样使用，必须包含 `/v1/metrics`。Header 优先级与 traces 相同：`OTEL_EXPORTER_OTLP_METRICS_HEADERS`，然后是 `OTEL_EXPORTER_OTLP_HEADERS`。

## Prometheus

设置 `THEWAY_METRICS_ADDR` 为套接字地址，例如 `127.0.0.1:9464`。daemon 会在该地址的 `GET /metrics` 提供 Prometheus 文本格式。暴露的指标：

- `theway_runtime_operations_total{operation,outcome,error_category}`
- `theway_runtime_operation_duration_seconds{operation,outcome,error_category}`（直方图）
- `theway_runtime_measurements_total{operation,measurement}`（`characters`、`turns`、`tool_calls`）
- `theway_runtime_tokens_total{provider,model,direction}`（`input`、`output`、`cache_read`、`cache_write`）
- `theway_runtime_active{operation}`
- `theway_runtime_observations_dropped_total`

## 队列

`THEWAY_OBSERVABILITY_QUEUE_CAPACITY` 设置有界观测队列大小，默认为 `4096`。非法或零值使用默认值。被丢弃的观测会计入 Prometheus 指标，且绝不会导致 agent run 失败。

## 完整上下文导出

默认情况下观测记录是内容安全的：prompt、消息、工具参数、工具结果、生成文本和原始错误字符串都不会进入观测记录。设置 `THEWAY_OBSERVABILITY_FULL_CONTENT=true`（也接受 `1`、`yes`、`on`）可显式开启完整上下文导出。

开启后，完成的操作会携带 JSON 输入/输出载荷：

| 操作 | 输入 | 输出 |
| --- | --- | --- |
| `llm.request` | 标准化请求：system instructions、消息、可见工具、生成选项 | 完整 assistant 消息，包括文本和工具调用 |
| `tool.execute` | 工具名和参数 | `content`、`details`、`executed`、`isError` |
| `multiagent.job` | Agent 名、来源、run/node id | 状态、最终文本，以及包含嵌套工具结果的完整消息转录 |
| `dag.node` | Agent 和任务 | 状态、输出、错误 |
| `dag.run` | Run 名称和节点任务列表 | 状态和错误 |

OTLP worker 会把载荷写入 `langfuse.observation.input` 和 `langfuse.observation.output` span 属性，设置 `langfuse.observation.type`（`llm.request` 和 `session.compaction` 为 `generation`，其余为 `span`），并在根 span 上设置 Langfuse trace 名称。每个内容属性上限为一百万字符；被裁剪的载荷会打上 `theway.content.truncated` 标记。

即使开启完整上下文导出，Prometheus 指标和结构化日志仍然保持不含内容。

## Langfuse

Langfuse 在 `<base-url>/api/public/otel/v1/traces` 接受带 Basic 认证的 OTLP HTTP/protobuf。创建 `$THEWAY_DIR/observability.env`（权限 `0600`）：

```sh
# Langfuse OTLP export
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT="https://<langfuse-host>/api/public/otel/v1/traces"
OTEL_EXPORTER_OTLP_TRACES_HEADERS="Authorization=Basic <base64(publicKey:secretKey)>"
THEWAY_OBSERVABILITY_FULL_CONTENT=true
```

重启 `thewayd` 加载该文件。Langfuse UI 中会以根名称 `theway agent.run` 出现上述嵌套 span 树。

## 结构化操作日志

observer worker 会在 target `theway::runtime` 下发出 `operation_started` 和 `operation_finished` tracing 事件。daemon 把日志写入 `~/.theway/logs/<session-id>.log`；`RUST_LOG` 控制记录级别。日志字段只包含稳定标识符和测量值，不包含消息内容。

## 故障隔离

观测队列压力、导出器构建失败和导出错误只会记录日志，绝不进入 agent 控制流。关停时会在有界超时内排空队列并调用 OpenTelemetry SDK provider 的 shutdown 方法。
