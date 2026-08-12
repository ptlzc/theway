## Purpose

The `thewayd` daemon is a discoverable local service: a client can find it,
verify it is alive, subscribe to its state, and drive it — without embedding a
copy of the agent runtime.

## ADDED Requirements

### Requirement: Daemon port discovery
The daemon publishes its bound port to `<THEWAY_DIR>/daemon-port` whenever it
serves gRPC or HTTP, so clients can find it without a fixed port contract.

#### Scenario: Fixed port
- **WHEN** `thewayd --port 44777` starts and binds successfully
- **THEN** `<THEWAY_DIR>/daemon-port` contains `44777`

#### Scenario: Random port
- **WHEN** `thewayd --port 0` starts and binds a random port
- **THEN** `<THEWAY_DIR>/daemon-port` contains the actual bound port

#### Scenario: MCP mode
- **WHEN** `thewayd --mcp` runs (no listener)
- **THEN** the port file is removed if it existed

### Requirement: Daemon health probe
A client can verify a daemon is alive by asking for the current session state
(`get_state`); a dead or absent daemon yields a connection error, not a hang.

#### Scenario: Alive daemon
- **WHEN** a client calls `get_state` on a running daemon
- **THEN** it receives a `SessionState` snapshot

#### Scenario: Dead daemon
- **WHEN** a client calls `get_state` on an unreachable port
- **THEN** the call fails promptly with a connection error

### Requirement: Client command surface
A single client wrapper exposes subscribe (state + event frames) and the
command set: send message, set model, cancel, approve control-plane prompt,
switch session.

#### Scenario: Subscribe to state stream
- **WHEN** a client opens `stream_events` on a running daemon
- **THEN** it receives snapshot frames and event frames until the stream closes

#### Scenario: Drive the agent
- **WHEN** a client sends a message while the daemon is idle
- **THEN** the daemon starts a turn and the stream carries the resulting state

### Requirement: Daemon spawn orchestration
A client binary can start a daemon when none is running and wait for readiness.

#### Scenario: Spawn on demand
- **WHEN** a client finds no daemon at the default port or port file
- **THEN** it spawns `thewayd` (inheriting cwd and environment) and waits until
  `get_state` succeeds or the spawn fails

#### Scenario: Reuse running daemon
- **WHEN** a daemon is already reachable
- **THEN** the client connects to it and does not spawn a second one
