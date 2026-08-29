## Purpose

Defines first-class FeedBlock variants for tool calls and errors so feed consumers can map events without string-prefix parsing or generic type names.

## ADDED Requirements

### Requirement: Dedicated tool_call FeedBlock

The feed protocol SHALL provide a `tool_call` FeedBlock variant representing a tool invocation. The generic `tool` variant SHALL be removed.

#### Scenario: Tool call is self-describing

- **WHEN** a client receives a tool invocation in the feed
- **THEN** it is represented as `tool_call`
- **AND** the client can identify it without translating a generic `tool` name

### Requirement: Dedicated error FeedBlock

The feed protocol SHALL provide an `error` FeedBlock variant with structured fields for message, code, and recoverability. Errors SHALL NOT be encoded as `plain` text with an `error:` prefix.

#### Scenario: Error is structured

- **WHEN** a client receives an error in the feed
- **THEN** it is represented as `error`
- **AND** it includes `message`, `code`, and `recoverable` fields when available

#### Scenario: Error-prefixed plain is removed

- **WHEN** the feed contains an error
- **THEN** no `plain` block with an `error:` string prefix is emitted

### Requirement: FeedBlock consumers use the same variants

All feed-producing surfaces, including session feed, `GetHistory`, and graph node streaming, SHALL use the same `tool_call` / `error` variants.

#### Scenario: History and node output align

- **WHEN** a client reads history or graph node output
- **THEN** tool invocations and errors use the same first-class variants as the live session feed
