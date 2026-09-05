# Runtime observability configuration

English | [中文](observability.zh.md)

This document describes how to configure the `thewayd` runtime observer: OpenTelemetry OTLP export, Prometheus metrics, structured operation logs, and the opt-in full-context payloads used by Langfuse. The mechanism lives in [`crates/theway-daemon/src/observability.rs`](../crates/theway-daemon/src/observability.rs) and the observations are produced by [`crates/theway-core/src/observability.rs`](../crates/theway-core/src/observability.rs).

## Configuration sources

All settings are read once at daemon startup by `TelemetryConfig::from_env`. Two sources are merged:

1. The process environment of the `thewayd` process.
2. The runtime env file `$THEWAY_DIR/observability.env`, defaulting to `~/.theway/observability.env`.

The process environment wins over the file. The file is a plain `KEY=value` list: blank lines and `#` comments are ignored, double quotes around a value are stripped, and malformed lines are dropped. Because the daemon reads the file itself, any launcher (TUI, workmate, scripts) picks up observability settings without inheriting shell exports.

## OTLP traces

The OpenTelemetry trace exporter uses OTLP over HTTP/protobuf and is built when one of these is non-empty:

| Variable | Meaning |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Generic endpoint. The exporter appends `/v1/traces` before sending spans. |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | Traces-specific endpoint, used verbatim. It must include the signal path, for example `https://collector.example.com:4318/v1/traces`. |

Headers are read from `OTEL_EXPORTER_OTLP_HEADERS`, with `OTEL_EXPORTER_OTLP_TRACES_HEADERS` taking precedence. The format is `k1=v1,k2=v2`; only the first `=` splits key from value, so Basic authentication tokens containing `=` are preserved.

Every operation becomes a span. Child operations share the parent trace and span id, so the exported data is one events tree per agent run: `agent.run` → `agent.turn` → `llm.request` / `tool.execute`, plus `session.compaction`, `multiagent.job`, `dag.run`, and `dag.node`.

Span attributes include the operation id, parent id, session/run/job/node/turn correlation values, tool name, provider/model, outcome, duration, error category, and token/cache measurements. The SDK resource is `service.name=thewayd`, the workspace version, and a process-specific `service.instance.id`.

## OTLP metrics

The metric exporter is separate from traces. It is built only when `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` is non-empty; a traces-only consumer does not start a metric exporter pointed at a default endpoint.

`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` is used verbatim and must include `/v1/metrics`. Headers follow the same precedence as traces: `OTEL_EXPORTER_OTLP_METRICS_HEADERS`, then `OTEL_EXPORTER_OTLP_HEADERS`.

## Prometheus

Set `THEWAY_METRICS_ADDR` to a socket address such as `127.0.0.1:9464`. The daemon serves Prometheus text format at `GET /metrics` on that address. Exposed metrics:

- `theway_runtime_operations_total{operation,outcome,error_category}`
- `theway_runtime_operation_duration_seconds{operation,outcome,error_category}` (histogram)
- `theway_runtime_measurements_total{operation,measurement}` (`characters`, `turns`, `tool_calls`)
- `theway_runtime_tokens_total{provider,model,direction}` (`input`, `output`, `cache_read`, `cache_write`)
- `theway_runtime_active{operation}`
- `theway_runtime_observations_dropped_total`

## Queue

`THEWAY_OBSERVABILITY_QUEUE_CAPACITY` sets the bounded observation queue size and defaults to `4096`. Invalid or zero values use the default. Dropped observations are counted in the Prometheus metric and never fail an agent run.

## Full-context export

By default observations are content-safe: prompts, messages, tool arguments, tool results, generated text, and raw error strings are absent from observation records. Set `THEWAY_OBSERVABILITY_FULL_CONTENT=true` (also `1`, `yes`, or `on`) to opt in to full-context export.

When enabled, finished operations carry JSON input/output payloads:

| Operation | Input | Output |
| --- | --- | --- |
| `llm.request` | The normalized request: system instructions, messages, visible tools, generation options | The full assistant message including text and tool calls |
| `tool.execute` | Tool name and arguments | `content`, `details`, `executed`, `isError` |
| `multiagent.job` | Agent name, source, run/node ids | Status, final text, and the full message transcript with nested tool results |
| `dag.node` | Agent and task | Status, output, error |
| `dag.run` | Run name and node task list | Status and error |

The OTLP worker writes the payloads to `langfuse.observation.input` and `langfuse.observation.output` span attributes, sets `langfuse.observation.type` (`generation` for `llm.request` and `session.compaction`, `span` otherwise), and puts a Langfuse trace name on the root span. Each content attribute is capped at one million characters; a clipped payload is flagged with `theway.content.truncated`.

Prometheus metrics and structured logs stay content-free even when full-content export is enabled.

## Langfuse

Langfuse accepts OTLP HTTP/protobuf at `<base-url>/api/public/otel/v1/traces` with Basic authentication. Create `$THEWAY_DIR/observability.env` (permissions `0600`) with:

```sh
# Langfuse OTLP export
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT="https://<langfuse-host>/api/public/otel/v1/traces"
OTEL_EXPORTER_OTLP_TRACES_HEADERS="Authorization=Basic <base64(publicKey:secretKey)>"
THEWAY_OBSERVABILITY_FULL_CONTENT=true
```

Restart `thewayd` to load the file. Traces appear in the Langfuse UI with the root name `theway agent.run` and the nested span tree described above.

## Structured operation logs

The observer worker emits `operation_started` and `operation_finished` tracing events under the target `theway::runtime`. The daemon writes logs to `~/.theway/logs/<session-id>.log`; `RUST_LOG` filters what is recorded. Log fields are stable identifiers and measurements only, never message content.

## Failure isolation

Observation queue pressure, exporter construction failures, and export errors are logged but never enter agent control flow. The same events mark the shared observer status degraded, and `WireStatus.observability` carries the state into the client: the TUI appends `observer error` to the ready/status rule and shows the latest message in the busy band. A successful export clears the degraded state automatically. Shutdown drains the queue and calls the OpenTelemetry SDK provider shutdown methods within bounded timeouts.
