# theway-probe

[English](README.md) | 中文

`theway-probe` 是面向已运行 `thewayd` 的独立 gRPC 可服务性客户端。它在不链接 daemon 或 transport Rust crate 的情况下检查标准 gRPC health 端点、health watch 流、多会话创建/列举行为和会话状态获取。

该二进制直接从 transport 拥有的 protobuf 定义编译 tonic 客户端，其中包括 `health.proto`，编译过程由 [`build.rs`](build.rs) 驱动。这样既保持 probe 独立，也能测试 daemon 实际暴露的同一协议。

## 使用

```bash
cargo run -p theway-probe -- \
  --server-addr http://127.0.0.1:9091 \
  --tests all
```

`--tests` 接受 `all`，或由 `health-check`、`health-watch`、`multi-session`、`get-state` 组成的逗号分隔子集。进程打印通过/失败汇总，任一选中检查失败时以非零状态退出。`--output-dir <path>` 还会为每项检查写入一个 JSON 结果文件。

Probe 不启动或停止 daemon，也不调用真实 LLM provider。

## 文档

- [Probe 架构](docs/architecture.md)

## 验证

```bash
cargo check -p theway-probe
cargo doc -p theway-probe --no-deps --document-private-items
```
