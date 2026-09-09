# theway-daemon

English | [中文](README.zh.md)

`theway-daemon` is the headless application kernel and the `thewayd` binary. It composes `theway-core`, `theway-storage`, `theway-transport`, `theway-llm-provider`, and `theway-mcp` into one long-running service.

The daemon owns session runtime assembly, model-facing tools, local and sandbox executor selection, hooks, triggers and cron jobs, nested-agent orchestration, MCP/LSP integration, telemetry export, and protocol-side behavior. It has no client-form or terminal-presentation concepts; `theway-tui` is one protocol client.

## Entry points

- `thewayd` parses process options and calls the public `run(DaemonOptions)` composition entry point.
- `DaemonPaths` resolves the base, home, working directory, and additional skill directories once at startup.
- `DaemonServices` owns process-lifetime registries and command output injection.
- `SessionRuntimeBuilder` is the internal construction path for initial, resumed, and switched session runtimes; session-scoped runtime-extension startup context is owned by `SessionExecutionContext`.
- Public modules expose supported extension points for executors, hooks, storage adapters, tools, templates, skills, triggers, and TypeScript extensions; the extension host owns package discovery, trust, QuickJS isolation, capability brokers, reversible registrations, durable state projection, quiescent reload, and client-neutral diagnostics.

The execution environment is selected at runtime. `[executor] kind = "local" | "sandbox"` in `config.toml` is carried by `theway-tui` into the spawned daemon as `--executor-kind` (issue #123). `local` is the default and binds `LocalExecutor`; `sandbox` binds `SandboxExecutor`, whose unsupported operations fail with `ExecutorError::UnsupportedKind`, and omits the direct-OS tools. The protocol server can also forward `ToolOps` to a controller-provided gRPC tool endpoint.

The trigram-indexed `grep` backend is opt-out (issue #135): `[tools] tgrep = false` in `config.toml` is carried by `theway-tui` as `--no-tgrep`, which binds `TgrepServerRegistry::disabled()`. Every `grep` query then takes the in-process walker, and no `tgrep serve` process or `.tgrep` index is created. The setting is startup-only and appears in `GetConfig`.

The model catalog is controller-provisioned (issue #136). `theway-tui` parses `[model]` (`provider`, `model`, `base_url`, `api_key`, `auto_fetch_models`) and `[[model.custom]]` from `config.toml` and pushes them through the settings RPC; the daemon registers the descriptors before resolving a model and resolves `api_key` after provider environment variables and before `auth.json`. With `auto_fetch_models = true` the daemon imports `GET <base_url>/models` and fills an unset model id from the first entry. Headless equivalents are `--api-key` and `--auto-fetch-models`; the former `models.json` files are no longer read.

## Running and validation

```bash
cargo run -p theway-daemon --bin thewayd -- --help
cargo test -p theway-daemon
cargo doc -p theway-daemon --no-deps --document-private-items
```

See [the daemon architecture](docs/architecture.md) for startup, session, storage, tool, protocol, and observability ownership.
