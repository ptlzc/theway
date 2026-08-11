//! Token + cost accumulator. Subscribes to assistant `MessageEnd` events and aggregates the
//! `Usage` already attached to each assistant message by the provider. No provider-specific
//! logic — the providers populate `Usage::cost` per-message via the catalog's pricing table,
//! so the tracker is a sum-only fold.
//!
//! Issue #7 part 1: in-memory totals, snapshot accessor, the `/cost` slash command on the
//! coding-agent side. Budget caps + fallback model land in a follow-up.

use std::sync::Arc;

use parking_lot::Mutex;
use theway_llm_provider::{Message as PiMessage, Usage};

use crate::types::{AgentMessage, LoopEvent};

/// Snapshot of the running totals. Cheap to clone — plain `Copy`-able fields.
#[derive(Clone, Debug, Default)]
pub struct CostSnapshot {
    pub tokens: Usage,
    pub turn_count: u64,
}

impl CostSnapshot {
    /// Total USD (input + output + cache). Convenience for the `/cost` summary line.
    pub fn total_cost(&self) -> f64 {
        self.tokens.cost.total
    }
}

/// Thread-safe accumulator. Cloning the tracker shares state via `Arc`.
#[derive(Clone, Debug)]
pub struct CostTracker {
    inner: Arc<Mutex<CostSnapshot>>,
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CostSnapshot::default())),
        }
    }

    pub fn snapshot(&self) -> CostSnapshot {
        self.inner.lock().clone()
    }

    /// Reset counters. Used by `/cost reset` and on session-switch.
    pub fn reset(&self) {
        *self.inner.lock() = CostSnapshot::default();
    }

    /// Apply a single assistant usage record. The provider has already computed costs against
    /// the catalog's pricing table — we just sum.
    pub fn record(&self, usage: &Usage) {
        let mut g = self.inner.lock();
        g.tokens.input = g.tokens.input.saturating_add(usage.input);
        g.tokens.output = g.tokens.output.saturating_add(usage.output);
        g.tokens.cache_read = g.tokens.cache_read.saturating_add(usage.cache_read);
        g.tokens.cache_write = g.tokens.cache_write.saturating_add(usage.cache_write);
        g.tokens.total_tokens = g.tokens.total_tokens.saturating_add(usage.total_tokens);
        let c = &mut g.tokens.cost;
        c.input += usage.cost.input;
        c.output += usage.cost.output;
        c.cache_read += usage.cost.cache_read;
        c.cache_write += usage.cost.cache_write;
        c.total += usage.cost.total;
        g.turn_count += 1;
    }

    /// Build an [`crate::agent::LoopListener`] that records every assistant `MessageEnd`. The
    /// listener clones the tracker (cheap — `Arc` bump) so the harness can keep its handle.
    pub fn as_listener(&self) -> crate::agent::LoopListener {
        let tracker = self.clone();
        Arc::new(move |event, _cancel| {
            let tracker = tracker.clone();
            Box::pin(async move {
                if let LoopEvent::MessageEnd {
                    message: AgentMessage::Llm(PiMessage::Assistant(a)),
                } = event
                {
                    tracker.record(&a.usage);
                }
            })
        })
    }
}

/// Render a one-line summary for the REPL status bar / banner. Format keeps the long
/// breakdown for `/cost` itself.
pub fn one_line_summary(snap: &CostSnapshot) -> String {
    format!(
        "tokens: in={} out={} cached={} total={} | cost ${:.4}",
        snap.tokens.input,
        snap.tokens.output,
        snap.tokens.cache_read + snap.tokens.cache_write,
        snap.tokens.total_tokens,
        snap.total_cost()
    )
}

/// Render the full breakdown — used by the `/cost` slash command.
pub fn full_breakdown(snap: &CostSnapshot) -> String {
    let c = &snap.tokens.cost;
    format!(
        "  turns:        {turns}\n\
         \n\
         Tokens:\n\
         \n  input         {input}\n  output        {output}\n  cache read    {cache_read}\n  cache write   {cache_write}\n  total         {total}\n\n\
         Cost (USD):\n\
         \n  input         ${ci:.4}\n  output        ${co:.4}\n  cache read    ${cr:.4}\n  cache write   ${cw:.4}\n  total         ${ct:.4}\n",
        turns = snap.turn_count,
        input = snap.tokens.input,
        output = snap.tokens.output,
        cache_read = snap.tokens.cache_read,
        cache_write = snap.tokens.cache_write,
        total = snap.tokens.total_tokens,
        ci = c.input,
        co = c.output,
        cr = c.cache_read,
        cw = c.cache_write,
        ct = c.total,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_llm_provider::UsageCost;

    #[test]
    fn accumulates_usage_and_costs() {
        let t = CostTracker::new();
        let u1 = Usage {
            input: 100,
            output: 50,
            cache_read: 10,
            cache_write: 5,
            total_tokens: 165,
            cost: UsageCost {
                input: 0.001,
                output: 0.0005,
                cache_read: 0.0001,
                cache_write: 0.00005,
                total: 0.00165,
            },
        };
        t.record(&u1);
        t.record(&u1);
        let s = t.snapshot();
        assert_eq!(s.tokens.input, 200);
        assert_eq!(s.tokens.output, 100);
        assert_eq!(s.tokens.total_tokens, 330);
        assert_eq!(s.turn_count, 2);
        assert!((s.total_cost() - 0.0033).abs() < 1e-9);
    }

    #[test]
    fn reset_clears_all_counters() {
        let t = CostTracker::new();
        let mut u = Usage::default();
        u.input = 10;
        t.record(&u);
        assert_eq!(t.snapshot().tokens.input, 10);
        t.reset();
        assert_eq!(t.snapshot().tokens.input, 0);
        assert_eq!(t.snapshot().turn_count, 0);
    }
}
