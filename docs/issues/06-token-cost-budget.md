# Token & cost tracking, budget cap, fallback model

> Parent: master roadmap issue.
> Tier: 2 (session/state).

## Goal

Provider responses already carry `Usage` (input/output tokens) and `ModelCost`. The agent never
surfaces it. Add:

- Per-session running totals: tokens (in/out/cached/thinking) and USD cost.
- Live display in the REPL (status line shows last-turn + session-total).
- `/cost` command for the breakdown by model + by turn.
- Budget cap config (`THEWAY_MAX_USD_PER_SESSION=1.50` or `~/.theway/config.toml`); on hit, prompt
  before continuing.
- Fallback-model rule: on a provider failure (5xx, rate-limited after retries, timeout), retry
  the *same* turn on a configured fallback model (e.g. claude → openai). Default off.

## Architecture

```
theway-core/src/cost/
  tracker.rs    CostTracker — atomic counters, breakdown by (provider, model, turn)
  budget.rs     BudgetPolicy — pre-turn + post-turn checks
  fallback.rs   FallbackRouter — alternative model on terminal errors
```

`CostTracker` lives on `AgentHarness`. After every assistant turn, `Agent` emits a
`UsageEvent`; the tracker accumulates and the session jsonl gets a `usage` line. On `--resume`,
the tracker is rebuilt by replaying jsonl entries.

Budget enforcement runs in two places:

1. Pre-turn: if the next request's estimated max would push above cap, prompt
   `[continue/stop]`.
2. Post-turn: if the actual usage hit cap, warn + ask before next prompt.

`FallbackRouter` plugs into `AgentSession` (already has retry policy). When the existing TS-regex
auto-retry exhausts, it consults the router. If a fallback model is configured *and* hasn't
already been tried this turn, swap and rerun.

## Stability

- Counters are `AtomicU64` so a panic mid-turn doesn't corrupt them.
- Cost data is informational, never load-bearing: a missing `ModelCost` for an exotic model
  records zero cost rather than failing the turn.
- Budget prompts default to "stop" on EOF (CI safety).
- Fallback model must not loop — track which models we've tried per-turn, never the same one
  twice.

## Extensibility

- `BudgetPolicy` is a trait: built-ins for `per-session-USD`, `per-day-USD` (across sessions
  via a sidecar), `per-turn-tokens`. Composable.
- `FallbackRouter` rules are a small DSL in config:
  `[[fallback]] when_provider = "anthropic" on_status = "529" use = "openai:gpt-4o"`.

## Performance

- Counter updates are lock-free.
- Cost calculation uses precomputed per-token rates from the model catalog; no FP division per
  delta event.

## Testing

| Layer | What |
|---|---|
| **unit** | Tracker accumulates from a synthetic usage stream; budget evaluator hits the right branch at exact-cap; fallback router never picks the same model twice. |
| **integration** | A run with two assistant turns: jsonl includes `usage` lines; replay rebuilds the tracker to identical totals. |
| **e2e** | Faux provider configured to return increasing usage per turn until cap; pre-turn prompt appears; user says no → REPL stays alive; user says yes → continues. Faux provider configured to error 5xx → fallback fires → completion comes from the secondary. |

## Acceptance criteria

- `/cost` shows running totals broken down by model.
- A budget-capped session prompts before each over-cap turn.
- A configured fallback model takes over after the primary's retries exhaust.
- Resume rebuilds totals exactly (re-running the same e2e produces the same `/cost` output).

## Out of scope

- Per-user / per-team aggregate cost dashboards.
- Pre-purchasing credits / billing integration.
