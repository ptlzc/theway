## Purpose

Defines per-session cache hit rate metrics and the client-side prefix-overlap algorithm used to estimate KV cache hits for theway's advanced context management.

## ADDED Requirements

### Requirement: Unambiguous cache usage fields

Session usage wire fields SHALL distinguish cached tokens, non-cached new tokens, and total input tokens. The system SHALL NOT expose a single ambiguous `input_tokens` field as the cache-hit denominator.

#### Scenario: Cache usage is explicit

- **WHEN** a session reports usage after an LLM call
- **THEN** it exposes `cached_tokens`, `new_tokens`, and `total_input_tokens`
- **AND** `total_input_tokens` equals `cached_tokens + new_tokens`

### Requirement: Provider cache hit rate

The system SHALL compute a per-session provider cache hit rate as `cache_read_tokens / total_input_tokens`, using provider-reported cache read tokens.

#### Scenario: Provider reports cache reads

- **WHEN** a provider reports `cache_read_tokens` and total input tokens
- **THEN** the session provider hit rate is `cache_read_tokens / total_input_tokens`

#### Scenario: Provider does not report cache reads

- **WHEN** a provider does not report cache read tokens
- **THEN** the provider hit rate is absent/unknown
- **AND** the prefix hit rate remains available

### Requirement: Prefix cache hit rate

The system SHALL compute a per-session prefix cache hit rate from the longest common prefix of the final provider context across consecutive requests. The calculation SHALL use chunked hashing and SHALL NOT require a tokenizer or embedding model.

#### Scenario: Append-only conversation has high prefix hit

- **WHEN** a new request only appends messages after an unchanged context prefix
- **THEN** the prefix hit rate is high and approaches 100%

#### Scenario: Mid-context insertion lowers prefix hit

- **WHEN** a change is inserted into the middle or start of the context
- **THEN** the prefix hit rate drops proportionally to the shortened matching prefix

#### Scenario: Tool-result virtualization keeps prefix stable

- **WHEN** large tool results are replaced with deterministic placeholders and no earlier context changes
- **THEN** the prefix hit rate remains stable across turns

### Requirement: Per-session accumulation and reset

Cache hit metrics SHALL accumulate per session and SHALL reset the prefix baseline when the provider or model changes.

#### Scenario: Model switch resets baseline

- **WHEN** a session switches to a different provider or model
- **THEN** the prefix baseline is reset for the new provider/model
- **AND** the first request after the switch reports a low prefix hit rate

#### Scenario: Session cumulative rates

- **WHEN** a session has multiple LLM calls
- **THEN** the session cumulative provider and prefix hit rates reflect aggregated cached/new/prefix-hit tokens
