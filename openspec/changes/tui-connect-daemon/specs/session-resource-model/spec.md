## Purpose

Existing capability `session-resource-model` (session lifecycle over the
transport surface). This change adds the missing gRPC session-switch RPC so a
client can move the daemon to another session without restarting it.

## ADDED Requirements

### Requirement: gRPC session switch
The gRPC surface exposes session switching (`SwitchSession`) with the same
semantics as the HTTP `WebCommand::SwitchSession`: validate the id, abort an
in-flight turn, rebuild the harness, and reflect the new session in subsequent
snapshots.

#### Scenario: Switch to an existing session
- **WHEN** a client calls `switch_session` with an existing session id while a
  turn is running
- **THEN** the turn is aborted, the daemon switches to the target session, and
  the next snapshot reports the new session id

#### Scenario: Unknown session id
- **WHEN** a client calls `switch_session` with an id that matches no session
- **THEN** the call fails and the daemon stays on its current session
