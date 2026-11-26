//! Stall watchdog — per-actor handler-duration instrumentation.
//!
//! Tokio is cooperative: a synchronous / CPU-bound actor `handle()` that never `.await`s blocks its
//! worker and can starve other actors, and (per the 2026-11-25 stress research) this case is
//! **invisible to tokio-console and the coop budget** — it only shows as worker busy-time skew. The
//! [`StallMonitor`] closes that gap by timing each handler and flagging any that exceed a threshold,
//! so a blocking handler is **detected and attributable to an actor** rather than silently starving
//! the runtime. The daemon wraps each actor's `handle()` with [`StallMonitor::record`]; a metrics /
//! supervisor surface reads [`StallMonitor::stalls`] / [`StallMonitor::max_micros`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Records handler execution times and counts those over the stall threshold.
pub struct StallMonitor {
    threshold_micros: u64,
    max_micros: AtomicU64,
    stalls: AtomicU64,
}

impl StallMonitor {
    /// A monitor that flags any handler taking at least `threshold`.
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold_micros: threshold.as_micros() as u64,
            max_micros: AtomicU64::new(0),
            stalls: AtomicU64::new(0),
        }
    }

    /// Record one handler's elapsed time; returns `true` if it was a stall (>= threshold).
    pub fn record(&self, elapsed: Duration) -> bool {
        let us = elapsed.as_micros() as u64;
        self.max_micros.fetch_max(us, Ordering::Relaxed);
        if us >= self.threshold_micros {
            self.stalls.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// How many handlers have stalled (>= threshold).
    pub fn stalls(&self) -> u64 {
        self.stalls.load(Ordering::Relaxed)
    }

    /// The longest handler observed, in microseconds.
    pub fn max_micros(&self) -> u64 {
        self.max_micros.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_handler_is_not_a_stall_but_a_slow_one_is() {
        let mon = StallMonitor::new(Duration::from_millis(100));
        assert!(!mon.record(Duration::from_millis(1)));
        assert_eq!(mon.stalls(), 0);
        assert!(mon.record(Duration::from_millis(200)));
        assert_eq!(mon.stalls(), 1);
        assert!(mon.max_micros() >= 200_000);
    }
}
