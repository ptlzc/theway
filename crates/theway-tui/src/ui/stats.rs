//! Throughput statistics for the composer busy band (issue #38).
//!
//! [`CpsMeter`] measures characters-per-second over a 1-second sliding
//! window from cumulative streamed-byte counters; [`busy_stats_text`]
//! formats the busy-band stats line (`char/s · input · output`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Sliding-window size for the char/s measurement.
pub const WINDOW: Duration = Duration::from_secs(1);

/// Char/s meter over a 1-second sliding window.
///
/// The streaming path records the *cumulative* streamed-byte count at the
/// end of each frame ([`CpsMeter::record`]); [`CpsMeter::cps`] returns the
/// rate over the trailing [`WINDOW`]. With no in-window activity (idle, or
/// the stream stalled for more than a window) the meter falls back to 0.
#[derive(Clone, Debug, Default)]
pub struct CpsMeter {
    /// `(timestamp, cumulative bytes)` samples, oldest first; kept trimmed
    /// to [`WINDOW`] on record.
    window: VecDeque<(Instant, usize)>,
}

impl CpsMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the cumulative byte count now.
    pub fn record(&mut self, total: usize) {
        self.record_at(Instant::now(), total);
    }

    /// Record the cumulative byte count at `now` (test seam).
    pub fn record_at(&mut self, now: Instant, total: usize) {
        // A reset counter (new session) restarts the window instead of
        // reporting a negative delta.
        if let Some(&(_, last)) = self.window.back()
            && total < last
        {
            self.window.clear();
        }
        self.trim(now);
        self.window.push_back((now, total));
    }

    /// Chars per second over the trailing [`WINDOW`]; 0.0 with no
    /// in-window activity.
    #[must_use]
    pub fn cps(&self) -> f64 {
        self.cps_at(Instant::now())
    }

    /// [`CpsMeter::cps`] at an explicit `now` (test seam).
    #[must_use]
    pub fn cps_at(&self, now: Instant) -> f64 {
        let Some(cutoff) = now.checked_sub(WINDOW) else {
            return 0.0;
        };
        let mut in_window = self.window.iter().skip_while(|(t, _)| *t < cutoff);
        let Some(&(t0, b0)) = in_window.next() else {
            return 0.0;
        };
        let &(t1, b1) = self.window.back().expect("non-empty window checked above");
        if t1 <= t0 || b1 <= b0 {
            return 0.0;
        }
        let secs = t1.duration_since(t0).as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        (b1 - b0) as f64 / secs
    }

    fn trim(&mut self, now: Instant) {
        let Some(cutoff) = now.checked_sub(WINDOW) else {
            return;
        };
        while let Some(&(t, _)) = self.window.front() {
            if t >= cutoff {
                break;
            }
            self.window.pop_front();
        }
    }
}

/// Compact human count for the busy-band stats line (`512`, `57.1k`,
/// `1.2M`) — one decimal, matching the feed's human number style.
#[must_use]
pub fn human_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

/// Busy-band stats line: `tps: {tps} · in: {in} · out: {out} · cache {hit}`.
/// `tps` is the live token-per-second estimate from the streamed feed; `in`
/// / `out` are the session-cumulative input/output token counts; `cache` is
/// the provider cache hit rate (falling back to the prefix estimate when the
/// provider does not report one, `-` when neither is known). Missing usage
/// slots are skipped.
#[must_use]
pub fn busy_stats_text(tps: f64, input_tokens: Option<u64>, output_tokens: Option<u64>) -> String {
    let mut text = format!("tps: {}", tps.round() as u64);
    if let Some(tokens) = input_tokens {
        text.push_str(&format!(" · in: {}", human_count(tokens)));
    }
    if let Some(tokens) = output_tokens {
        text.push_str(&format!(" · out: {}", human_count(tokens)));
    }
    text
}

/// Format a cache hit ratio for display (`72.3%` or `-` when unknown).
fn format_hit_rate(rate: Option<f64>) -> String {
    match rate {
        Some(rate) => {
            let pct = rate * 100.0;
            let rounded = (pct * 10.0).round() / 10.0;
            if (rounded - rounded.round()).abs() < f64::EPSILON {
                format!("{}%", rounded.round() as u64)
            } else {
                format!("{rounded:.1}%")
            }
        }
        None => "-".to_string(),
    }
}

/// Busy-band stats line backed by session-cumulative usage: `tps: {tps} ·
/// in: {in} · out: {out} · cache {hit}`. The cache hit rate prefers the
/// provider-reported ratio and falls back to the client-side prefix estimate.
#[must_use]
pub fn busy_stats_text_with_session(
    tps: f64,
    total_input_tokens: u64,
    output_tokens: u64,
    provider_cache_hit_rate: Option<f64>,
    prefix_cache_hit_rate: Option<f64>,
) -> String {
    let hit = provider_cache_hit_rate.or(prefix_cache_hit_rate);
    let hit = format_hit_rate(hit);
    format!(
        "tps: {} · in: {} · out: {} · cache {hit}",
        tps.round() as u64,
        human_count(total_input_tokens),
        human_count(output_tokens),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arbitrary anchor; only relative offsets matter.
    fn t0() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn empty_meter_reads_zero() {
        let meter = CpsMeter::new();
        assert_eq!(meter.cps_at(t0()), 0.0);
    }

    #[test]
    fn single_sample_reads_zero() {
        let mut meter = CpsMeter::new();
        let t = t0();
        meter.record_at(t, 100);
        assert_eq!(meter.cps_at(t), 0.0);
    }

    #[test]
    fn sliding_window_rate() {
        let mut meter = CpsMeter::new();
        let t = t0();
        meter.record_at(t, 0);
        meter.record_at(t + Duration::from_millis(500), 500);
        // 500 bytes over 0.5 s.
        assert!((meter.cps_at(t + Duration::from_millis(500)) - 1000.0).abs() < 1e-6);

        meter.record_at(t + Duration::from_secs(1), 2000);
        // Full 1 s window: 2000 bytes over 1.0 s.
        assert!((meter.cps_at(t + Duration::from_secs(1)) - 2000.0).abs() < 1e-6);
    }

    #[test]
    fn window_drops_samples_older_than_one_second() {
        let mut meter = CpsMeter::new();
        let t = t0();
        meter.record_at(t, 0);
        meter.record_at(t + Duration::from_millis(500), 500);
        meter.record_at(t + Duration::from_secs(1), 2000);
        // Sample at t0 (bytes 0) fell out of the window by t+1.6s; the rate
        // comes from (t+1s, 2000) → (t+1.6s, 2600).
        meter.record_at(t + Duration::from_millis(1600), 2600);
        let cps = meter.cps_at(t + Duration::from_millis(1600));
        assert!((cps - 1000.0).abs() < 1e-6, "rate after trim: {cps}");
    }

    #[test]
    fn stalled_stream_falls_back_to_zero() {
        let mut meter = CpsMeter::new();
        let t = t0();
        meter.record_at(t, 0);
        meter.record_at(t + Duration::from_millis(200), 400);
        // No records for more than a window: nothing left in-window.
        assert_eq!(meter.cps_at(t + Duration::from_millis(1500)), 0.0);
    }

    #[test]
    fn counter_reset_restarts_window() {
        let mut meter = CpsMeter::new();
        let t = t0();
        meter.record_at(t, 10_000);
        meter.record_at(t + Duration::from_millis(100), 100); // reset
        meter.record_at(t + Duration::from_millis(300), 500);
        // 400 bytes over 0.2 s since the reset, not a negative delta.
        let cps = meter.cps_at(t + Duration::from_millis(300));
        assert!((cps - 2000.0).abs() < 1e-6, "rate after reset: {cps}");
    }

    #[test]
    fn human_count_formats() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(1_200), "1.2k");
        assert_eq!(human_count(57_100), "57.1k");
        assert_eq!(human_count(1_234_567), "1.2M");
    }

    #[test]
    fn busy_stats_text_full_and_tps_only() {
        assert_eq!(
            busy_stats_text(84.0, Some(57_100), Some(1_200)),
            "tps: 84 · in: 57.1k · out: 1.2k"
        );
        // No usage data → tps only; fractional tps rounds.
        assert_eq!(busy_stats_text(83.6, None, None), "tps: 84");
        // Partial usage data renders only the present slots.
        assert_eq!(busy_stats_text(0.0, Some(500), None), "tps: 0 · in: 500");
    }
}
