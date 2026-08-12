# theway gRPC 服务化探针报告

**日期**: 2026-08-12T02:00:50Z
**仓库**: /root/workspace/theway (ptlzc/theway, Rust)
**探针版本**: crates/probe v0.1.0

## 总览

| 状态 | 数量 |
|------|------|
| PASS | 6 |
| FAIL | 0 |
| **总计** | **6** |

## 能力矩阵

| 能力 | 现状 | 证据 | 缺口 |
|------|------|------|------|
| get-state | ✅ | GetState returned SessionState: model=anthropic:claude-haiku-4-5, cwd=/root/workspace/theway, busy=false, blocks=3, sid=019ff3b3-5d1f-7f | — |
| health-check | ✅ | Check returns SERVING for all service names | — |
| health-watch | ✅ | Watch stream emitted 2 SERVING frames (continuous stream) | — |
| multi-session | ✅ | Created 2 sessions, ListSessions returned 3 total; each session independently addressable | — |
| shutdown-SIGINT | ✅ | clean exit code 0 after SIGINT | — |
| shutdown-SIGTERM | ✅ | clean exit code 0 after SIGTERM | — |

## Plan B 评估: --web 常驻可行性

### --web vs --grpc 对比

| 维度 | --grpc | --web |
|------|--------|-------|
| 协议 | gRPC binary (tonic) | HTTP/SSE + WebSocket (axum) |
| 事件流 | StreamEvents server-streaming | SSE `/events` + WS `/ws` |
| 健康检查 | grpc.health.v1 | `/healthz` HTTP |
| 成熟度 | P0/P1 (较新) | 更早, 更成熟 |
| 客户端 | gRPC client (workmate) | 浏览器原生 EventSource |
| 优雅关闭 | ✅ SIGTERM + SIGINT (本次修复) | ✅ SIGINT (同路径) |
| 服务发现 | ❌ 无 reflection | N/A |

### 结论

`--web` 完全可作为常驻服务使用, 成熟度高于 `--grpc`:
- HTTP `/healthz` + SSE `/events` 已覆盖全部运维需求
- 浏览器原生 EventSource 无需客户端依赖
- 与 `--grpc` 共享同一 `run_transport_loop` 事件循环, 行为一致

### 本次探针修复成果

经过探针验证并修复了两项阻塞缺陷（改动最小原则）:

| 缺陷 | 修复 | 文件 |
|------|------|------|
| Health Watch 非持续流 | `HealthService::watch` 从单帧改为 5s 间隔持续流 | `crates/server/src/transport/grpc.rs:577-583` |
| 无 SIGTERM handler | `run_transport_loop` select! 增加 `tokio::signal::unix::signal(SIGTERM)` 分支 | `crates/server/src/ui/web_loop.rs:191-196,253-260` |

若需 gRPC 生态集成 (grpc_health_probe, gRPC gateway, 统一微服务治理),
仅需补齐 gRPC reflection (低优先级, loopback-only 场景影响小)。
为 workmate UI 服务, `--web` 已覆盖全部需求。
