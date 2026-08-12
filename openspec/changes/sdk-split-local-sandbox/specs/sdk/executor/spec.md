## Purpose

Tool execution is decoupled from the agent runtime: tools are defined against
an execution-environment abstraction, so the same harness, session, snapshot
and command surfaces run with a **local** executor (local editing mode, the
default) or a **remote sandbox** executor (future e2b) without client-visible
changes. This capability defines the abstraction and its contracts.

## ADDED Requirements

### Requirement: Executor abstraction
The runtime binds tools to an execution environment through a single
`ToolExecutor` interface covering file operations, command execution, search
and git. Tool definitions (schema, parameters, result shape) are independent
of the executor; an executor reports its kind so callers can distinguish
local from sandbox execution.

#### Scenario: Local editing mode
- **WHEN** the daemon is assembled with the local executor
- **THEN** `read_file`/`write_file`/`run_command` operate on the local
  filesystem and process table, and the executor kind is `local`

#### Scenario: Remote sandbox mode (future)
- **WHEN** the daemon is assembled with a sandbox executor
- **THEN** the same tool calls are dispatched to the sandbox environment and
  the executor kind is `sandbox`; no runtime, snapshot or client change is
  required to switch

### Requirement: Environment-agnostic runtime
The agent harness, session model, feed, snapshots and command framework never
depend on a specific executor — they observe only the abstraction.

#### Scenario: Same client for both modes
- **WHEN** a client connects to a daemon running with either executor kind
- **THEN** the gRPC/HTTP surface (state, frames, commands) is identical and
  the client needs no mode-specific code

### Requirement: Sandbox seam
The SDK ships a sandbox executor stub that fails with an explicit
"unsupported" error until a real implementation (e.g. e2b) lands.

#### Scenario: Unsupported sandbox calls
- **WHEN** a tool call is routed to the stub sandbox executor
- **THEN** the call fails promptly with a clear unsupported-mode error,
  never hangs
