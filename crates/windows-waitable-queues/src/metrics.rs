// Copyright (c) Mike Grier.

//! Counters a queue keeps about itself.
//!
//! # What is here, and what is deliberately not
//!
//! Three numbers, and each is here because it answers a question the queue's
//! own state cannot:
//!
//! - **Refusals**, so backpressure is *measured* rather than inferred from a
//!   caller's error handling.
//! - **Doorbell rings**, so the skip rule is measurable rather than assumed.
//!   Counted by [`Doorbell`](crate::doorbell::Doorbell) rather than by
//!   [`Metrics`], for the reason given below; a reader after the ring count
//!   will not find it on this type.
//! - **Peak depth**, so a bound can be chosen from evidence.
//!
//! **Depth itself is not here**, and its absence is a decision. `Bounded::len`
//! already reports it, computed on demand from positions the queue keeps
//! anyway, so restating it as a metric would give one number two names and two
//! places to drift. What belongs here is only what has to be *accumulated*.
//!
//! # Why two of the three are free and one is not
//!
//! A counter on a hot path is a shared line every thread writes, which is the
//! same false-sharing cost the queues are carefully padded to avoid. So each
//! counter is placed where it is already paid for:
//!
//! - **Refusals** increment only when a push is *refused*, which is off the
//!   success path entirely.
//! - **Rings** increment only when the doorbell actually calls `SetEvent`,
//!   which is a syscall measured at ~81 ns against ~7 ns for an uncontended
//!   atomic. That increment happens inside the doorbell, so the counter lives
//!   on [`Doorbell`](crate::doorbell::Doorbell) rather than on [`Metrics`]:
//!   keeping it here would mean reaching across to a line this type does not
//!   own. The skipped signals -- the hot ones -- are deliberately *not*
//!   counted, because that increment would land on exactly the path the skip
//!   exists to cheapen.
//! - **Peak depth** cannot be placed that way, because it must observe every
//!   change. It is therefore **opt-in**, and off by default; see
//!   [`Metrics::record_depth`] and
//!   [D-23](../DESIGN-NOTES.md#d-23).

use core::fmt;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The counters one queue keeps.
pub(crate) struct Metrics {
    /// Pushes refused for want of room.
    ///
    /// Written only on the failure path, so it costs a successful push nothing.
    refused: AtomicU64,
    /// The deepest the queue has been observed to get, if it is being tracked.
    ///
    /// `Option` rather than a sentinel because "not tracked" and "never got
    /// past empty" are different answers, and a caller acting on a `0` that
    /// meant the former would be reading a number nobody recorded.
    high_water: Option<AtomicUsize>,
}

impl Metrics {
    /// Counters with peak depth left untracked, which is the default.
    pub(crate) const fn new(track_high_water: bool) -> Self {
        Self {
            refused: AtomicU64::new(0),
            high_water: if track_high_water {
                Some(AtomicUsize::new(0))
            } else {
                None
            },
        }
    }

    /// Whether peak depth is being tracked.
    ///
    /// Read on the push path by shapes whose producer does not otherwise know
    /// the depth, so that they only pay for the load that computes it when
    /// somebody asked for the answer. The field is written once at construction
    /// and never again, so the line is shared but read-only -- which is the
    /// cheap kind.
    pub(crate) fn tracks_high_water(&self) -> bool {
        self.high_water.is_some()
    }

    /// Record that the queue reached `depth`.
    ///
    /// # Why this loads before it modifies
    ///
    /// The obvious spelling is an unconditional [`AtomicUsize::fetch_max`], and
    /// it would be a read-modify-write on a shared line for **every push** --
    /// the cost this crate pads its positions apart to avoid.
    ///
    /// A new maximum is rare: it happens while a queue is filling and then
    /// almost never again. So the common case is turned into a plain load of a
    /// line that is written rarely and read often, and the read-modify-write is
    /// reached only when the value is actually about to change. The load can be
    /// stale, and the `fetch_max` that follows is what makes the result correct
    /// anyway -- a racing pair of producers may both see an old maximum, but
    /// `fetch_max` keeps the larger of the two regardless of which lands first.
    pub(crate) fn record_depth(&self, depth: usize) {
        let Some(high_water) = self.high_water.as_ref() else {
            return;
        };
        // `>` rather than `>=`, and a mutation run will report the two as
        // indistinguishable -- correctly. `fetch_max(depth)` when `depth`
        // already equals the maximum stores the value it read, so the weaker
        // test only buys an extra read-modify-write on the shared line in the
        // one case it admits. That is the cost this guard exists to avoid, so
        // the difference is real; it is just not a difference in any answer,
        // and no test can be written for it. Left documented rather than
        // chased.
        if depth > high_water.load(Ordering::Relaxed) {
            high_water.fetch_max(depth, Ordering::Relaxed);
        }
    }

    /// Record that a push was refused for want of room.
    pub(crate) fn record_refusal(&self) {
        self.refused.fetch_add(1, Ordering::Relaxed);
    }

    /// How many pushes have been refused for want of room.
    pub(crate) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// The deepest the queue has been observed to get, if tracked.
    pub(crate) fn high_water(&self) -> Option<usize> {
        Some(self.high_water.as_ref()?.load(Ordering::Relaxed))
    }
}

impl fmt::Debug for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metrics")
            .field("refused", &self.refused())
            .field("high_water", &self.high_water())
            .finish()
    }
}

#[cfg(test)]
mod tests;
