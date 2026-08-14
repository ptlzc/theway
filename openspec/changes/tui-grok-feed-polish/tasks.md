# Tasks

```mermaid
graph TD
  A["1-wire: proto/wire usage + config 解析 + markdown 宽度入口"] --> B["2-daemon: usage 发布 + thinking 摘要配置与回填"]
  A --> C["3-tui-chrome: 光标 + 上下文用量 + 滚动跟随修复"]
  C --> D["4-tui-feed: grok 风格用户/工具渲染 + mermaid 宽度 + thinking 三态"]
  D --> E["5-tui-selection: feed 文字选择高亮（不复制）"]
  B --> F["6-verify: 全量检查 + 提交推送 + issue 收尾"]
  E --> F
```

- `1-wire` — theway-transport proto.rs/wire.rs/config.rs、proto/theway_grpc.proto、theway-markdown lib.rs。
- `2-daemon` — theway-daemon turn/daemon.rs、bin/thewayd.rs、theway-transport feed model.rs/types.rs。
- `3-tui-chrome` — theway-tui ui/mod.rs、ui/prompt_chrome.rs。
- `4-tui-feed` — theway-tui feed_render.rs、ui/mod.rs、ui/app_input.rs。
- `5-tui-selection` — theway-tui ui/mod.rs、ui/app_input.rs。
- `6-verify` — 全 workspace check + 定向测试 + clippy；按 crate 分批提交；push main；close #33。
