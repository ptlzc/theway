## Purpose

The SDK is a first-class crate split by execution environment: `common`
(environment-agnostic), `local` (local editing mode) and `sandbox` (remote
sandbox mode). The daemon runtime lives in its own crate; clients depend only
on the SDK. This capability defines the crate layout, the dependency
boundary and the command-registry layering.

## ADDED Requirements

### Requirement: SDK crate layout
A client-facing SDK crate (`theway`) exposes `common`, `local` and `sandbox`
layers. `common` contains no execution-environment code; `local` implements
local editing mode; `sandbox` holds the remote-mode seam.

#### Scenario: Client depends only on the SDK
- **WHEN** a client crate (e.g. `theway-tui`) depends on the SDK
- **THEN** its dependency graph contains no daemon runtime code (no tools,
  triggers, skills, MCP or LSP implementations)

#### Scenario: Daemon depends on the SDK
- **WHEN** the daemon crate assembles a runtime
- **THEN** it consumes `common` types and the `local`/`sandbox` executor
  implementations from the SDK and adds its own runtime-only code

### Requirement: SDK surface
The SDK exposes the shared client/server surface: session management
(repo open/resume/list/delete), session archive export/import/activation,
auth credential store, prompt history, image loading, mention expansion,
secret redaction, config/base-dir resolution, the feed render model, and the
command framework (registry, command outcome, local command set).

#### Scenario: Local surfaces keep working
- **WHEN** the TUI runs `/login`, `--list-sessions`, `/session export`,
  `/session import` or renders the feed
- **THEN** the behavior is unchanged from before the split (same files,
  same wire model, same output)

### Requirement: Command registry layering
The SDK registers only commands that need no daemon runtime (quit, clear,
help, login, logout, session list/export/import); the daemon appends its
runtime commands on top. A client can enumerate and dispatch local commands
without any daemon dependency.

#### Scenario: Local commands work offline
- **WHEN** a client invokes a local command (e.g. `/login` or `/session
  export`) without a connected daemon
- **THEN** the command runs entirely from the SDK and needs no daemon
  round-trip

### Requirement: Crate naming and paths
The SDK package keeps the name `theway`; existing client paths
(`theway::session`, `theway::app::feed`, …) keep resolving after the split.
The daemon package is `theway-daemon` with its binary unchanged (`thewayd`).

#### Scenario: Zero client code churn
- **WHEN** the split lands
- **THEN** client `use theway::…` paths compile unchanged; only the crate
  dependency (path) in `Cargo.toml` changes
