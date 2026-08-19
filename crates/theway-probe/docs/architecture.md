# theway-probe 架构

## 构建期协议归属

[`build.rs`](../build.rs) 读取 `theway-transport` 拥有的 protobuf 源，包括 [`commands.proto`](../../theway-transport/proto/commands.proto) 和 [`health.proto`](../../theway-transport/proto/health.proto)，使用 `protox` 编译 command、session、graph、event、health 定义，并用 `tonic-prost-build` 生成客户端。Probe 不依赖 Rust `theway-transport`，以独立消费者身份验证外部 gRPC 服务。

任何影响已导入服务的 protobuf 变化，都必须保证该构建脚本仍能编译完整 import graph。Transport crate 是 proto 定义的唯一来源。

## 运行流程

[`main.rs`](../src/main.rs) 解析 server 地址、选中检查和可选输出目录。检查顺序执行，使结果和 server 变更保持确定：

- `health-check` 调用 `grpc.health.v1.Health/Check` 并要求 `SERVING`。
- `health-watch` 调用 `Health/Watch` 并要求从流中收到 serving 更新。
- `multi-session` 创建会话，并确认 session service 能列出可独立寻址的标识。
- `get-state` 调用 command service，并确认可以获取会话 snapshot。

每项检查返回带证据和可选失败详情的 `TestResult`。命令打印全部结果，按需写 JSON 文件；通过数与选中数不一致时以状态 1 退出。

## 不变量

- Probe 只连接操作方提供的地址，绝不启动 daemon。
- 检查与 provider 无关，不提交需要 API key 的 prompt。
- 每个选中检查产生一个终止结果；未知检查名称明确失败。
- Protobuf 定义从 `theway-transport` 消费，不复制到本 crate。
- 进程退出状态反映所有选中检查的汇总结果。
