## 1. Transport client + gRPC session switch (daemon side first)

- [x] 1.1 Add `switch_session` RPC to the gRPC service: proto stub extension,
      `GrpcState::switch_session` handler delegating to `DaemonApp`
      (`WebCommand::SwitchSession` semantics: validate id, abort turn, swap
      harness, publish snapshot); returns `CommandResult`
- [x] 1.2 Add `client` module in `theway-transport`: `GrpcClient` wrapper
      (connect to host:port, `get_state`, `stream_events` frame stream with
      snapshot+event decoding, typed calls for send_message / set_model /
      cancel / approve / switch_session / graph_*)
- [x] 1.3 Tests: client round-trip against a spawned `thewayd` (get_state,
      send_message → snapshot change, switch_session to existing/unknown id,
      stream frames arrive)

## 2. Daemon service polish

- [x] 2.1 `thewayd` readiness: confirm `get_state` works immediately after bind
      (port file written before serve); no change expected, verify
- [x] 2.2 Multi-client sanity: two simultaneous `stream_events` subscribers both
      receive frames (broadcast already supports it; add a test)
- [x] 2.3 Expose daemon discovery helpers in transport client: read
      `<THEWAY_DIR>/daemon-port`, default port 44777, health probe
      (`get_state` with short timeout)

## 3. TUI client rewrite

- [x] 3.1 Delete TUI in-process runtime: `web_loop.rs`, App transport methods,
      kernel/feed ownership, `session_factory` module, startup assembly
      (harness/session/triggers/DAG/MCP construction)
- [x] 3.2 App state model: `latest: WebStatus` cache updated from
      `get_state` + stream snapshot frames; feed rendered from
      `feed_blocks`/`feed_lines`; busy/model/goal/control-plane from snapshot
- [x] 3.3 App event loop: select over crossterm events + client stream frames +
      reconnect timer; submit → `send_message`, Ctrl-C → `cancel`, control
      plane → `approve`, model picker → `set_model`, session switch →
      `switch_session`; offline banner on stream drop
- [x] 3.4 Local surfaces kept: `/login` (auth.json), `--list-sessions` /
      export/import / delete (local SQLite repo) — verify unchanged behavior
- [x] 3.5 Startup orchestration in `theway` main: probe port file / default
      port → reuse running daemon, or spawn `thewayd` (inherit cwd/env;
      `-c`/`--resume-id` become launch args) → wait ready → connect

## 4. Cleanup and verification

- [x] 4.1 Remove now-dead TUI code (web_loop, transport methods, kernel
      imports) and unused deps (ratatui stays for rendering; kernel/feed stay
      in server for the daemon)
- [x] 4.2 Rewrite TUI e2e tests: spawn `thewayd` + `GrpcClient` where they
      previously drove the in-process App; keep CLI-surface tests local
- [x] 4.3 Docs: README/CLI help updated (`theway` connects to daemon; daemon
      lifecycle; tool-cwd semantics)
- [x] 4.4 Full verification: `cargo test --workspace`, clippy -D warnings,
      fmt; smoke: `thewayd & theway` interactive session, tool runs in daemon
      cwd, Ctrl-C abort, session switch, daemon-death reconnect banner
