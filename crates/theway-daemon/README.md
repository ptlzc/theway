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

## Structured user input

[`attachments/mod.rs`](src/attachments/mod.rs) owns admission for one round of input. `PromptAdmission::admit` resolves the round's `@path` mentions once through `theway_transport::mentions::mentions` — reading each path from the session cwd, truncating at the shared 64 KiB window, skipping paths that do not resolve, and collapsing a repeated path into one part — validates submitted images through the shared `theway_transport::images` rules (magic bytes, per-image size, per-message count), stores every byte in the attachment library, and only then returns the `UserInput` record; image validation runs before the first write, so a submission rejected for its images stores nothing, and `PromptAdmission::with_injected` appends a skill, trigger, or extension part. `turn/daemon/input.rs` hands that record to `AgentHarness::prompt_with_input` / `record_user_input_prompt`.

A `/skill` turn arrives as the `attach_skill_prompt(text, Some(name))` envelope, and `PromptAdmission::admit` splits it with `theway_transport::commands::split_skill_prompt`: the record's `text` is the user's own text, and the envelope preamble from `skill_prompt_preamble` becomes an `Injected { source: "skill", name }` part. The model-facing prompt the caller already holds is not rewritten.

`DaemonServices.attachments` holds the process-lifetime `LocalAttachmentStore` every session shares. `start_process_services` receives the resolved `DaemonPaths::base` and passes it to `DaemonServices::with_attachments_base`, which roots the library at `<base>/attachments/v1`; `thewayd --theway-dir`, then `$THEWAY_DIR`, then `<home>/.theway` decide that base, so a `--theway-dir` override moves the attachment library with the rest of the base-directory layout.

Command-synthesised prompts carry a record too. `turn/daemon/input.rs::command_prompt_record` records a `CommandOutcome::RunAgentPrompt` prompt — a skill envelope as `InputSource::User` with the same injected part, anything else (a goal command, a trigger-authoring command, a file command, or host-injected text) as `InputSource::Host` — and `turn/daemon/commands/triggers.rs::trigger_web_rule_now` records the `WireCommand::TriggerRuleNow { id }` turn as `InputSource::Trigger` with the rule id in `source_ref`.

A trigger or cron round injected into the parent conversation writes its record immediately before the user message on both branches of `trigger_engine/execution`: the follow-up queue while the parent agent streams, and the direct transcript-plus-state append while it is idle. The model-facing message keeps the `[Trigger <trace_id>] ` prefix, while the record's `text` has that prefix stripped, `source` is `InputSource::Trigger`, and `source_ref` is the trace id.

Display derives from the record on every surface: the live feed, resume replay (`feed_replay::replay_transcript`), and message paging (`session_observability.rs`) render the same user block — the submitted text, one chip per attachment, and the round's origin — while each `Injected` part becomes its own `Context` row, so neither injected text nor expanded file content appears in the user bubble.

## Running and validation

```bash
cargo run -p theway-daemon --bin thewayd -- --help
cargo test -p theway-daemon
cargo doc -p theway-daemon --no-deps --document-private-items
```

See [the daemon architecture](docs/architecture.md) for startup, session, storage, tool, protocol, and observability ownership.
