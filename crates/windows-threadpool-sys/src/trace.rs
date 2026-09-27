// Copyright (c) Mike Grier
//! A trace that is cheap enough to use on a timing-sensitive defect.
//!
//! # Why this is not `eprintln!`
//!
//! It exists for one class of problem: a fault that only appears under
//! concurrency, where the obvious instrument destroys the thing it is
//! measuring. Formatting and writing a line takes microseconds and a lock on
//! stderr; the window being investigated may be shorter than that, so an
//! `eprintln!` in the wrong place does not observe a race, it prevents one.
//!
//! Three properties follow, and each is a deliberate cost:
//!
//! 1. **It compiles to nothing unless the `trace` feature is on.** Every entry
//!    point below is `#[inline(always)]` and has an empty body without the
//!    feature, so a published build carries no branch, no atomic, and no
//!    storage. This is what makes it safe to leave the call sites in place.
//! 2. **Recording does not format and does not allocate.** An entry is a
//!    timestamp, a thread id, two `&'static str` labels and two `u64` slots.
//!    Text is produced only when a dump is asked for, which happens after the
//!    interesting moment has passed.
//! 3. **It is off at runtime even when compiled in**, and when on it is
//!    *narrowed* rather than global -- see [`crate::trace::enabled`]. A trace that records
//!    everything is a trace that changes the schedule of everything.
//!
//! # Narrowing to one scenario
//!
//! Set `WINDOWS_THREADPOOL_TRACE` to a comma-separated list of target
//! substrings. Only records whose target contains one of them are kept:
//!
//! ```text
//! $env:WINDOWS_THREADPOOL_TRACE = 'wait'            # just the wait object
//! $env:WINDOWS_THREADPOOL_TRACE = 'wait,delivery'   # two subsystems
//! $env:WINDOWS_THREADPOOL_TRACE = '*'               # everything
//! ```
//!
//! The targets this crate records under are `wait`, `work`, `io`, `timer`, and
//! `timer-periodic` -- one per pool object, and matched by substring, so
//! `timer` selects the periodic timer as well. Dependent crates add their own;
//! `windows-ioring-sys` records under `delivery` and, from its own tests,
//! `postmortem`.
//!
//! Unset, empty, or matching nothing means no record is kept and the cost is
//! one relaxed atomic load per call site.

/// One observation. Deliberately `Copy` and free of owned data, so recording
/// is a push and never an allocation.
///
/// Lives outside the feature gate so that [`Buffer`]'s eviction policy is
/// built and tested in every build, not only in one nobody's `cargo test`
/// selects. Without the feature the formatting that reads these fields is
/// compiled out, which is what the `dead_code` allowance is for.
#[cfg(any(feature = "trace", test))]
#[cfg_attr(not(feature = "trace"), allow(dead_code))]
#[derive(Clone, Copy)]
struct Record {
    at: std::time::Duration,
    thread: u32,
    target: &'static str,
    event: &'static str,
    a: u64,
    b: u64,
}

/// The bounded record store and, with it, the whole of what happens when a
/// trace outgrows the room it was given.
///
/// Separated from the global it backs so that the policy can be exercised at a
/// capacity of four rather than of eight thousand. Filling the real buffer in a
/// test would evict every record every other test had just recorded, which is
/// the one thing a shared trace cannot tolerate.
#[cfg(any(feature = "trace", test))]
struct Buffer {
    records: Vec<Record>,
    /// How many records were evicted to make room for later ones.
    dropped: usize,
    /// Whether the first eviction has already been reported.
    announced: bool,
}

#[cfg(any(feature = "trace", test))]
impl Buffer {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(capacity),
            dropped: 0,
            announced: false,
        }
    }

    /// Append `record`, evicting the oldest first if the buffer is already at
    /// `capacity`.
    ///
    /// Returns whether this call was the **first** eviction, so a caller can
    /// report the loss once rather than on every subsequent push. The report
    /// matters because the alternative is silence: a run that never dumps
    /// never reads [`dump`]'s dropped-record line, so an overflow that
    /// truncated the start of a capture would be discovered only by whoever
    /// later wondered why the trace began in the middle.
    ///
    /// A `capacity` of zero stores nothing and counts every record as dropped.
    /// Degenerate, and defined rather than left to `remove(0)` on an empty
    /// vector, which panics.
    fn push(&mut self, record: Record, capacity: usize) -> bool {
        if self.records.len() >= capacity {
            if capacity > 0 {
                self.records.remove(0);
            }
            self.dropped += 1;
            let first = !self.announced;
            self.announced = true;
            if capacity == 0 {
                return first;
            }
            self.records.push(record);
            return first;
        }
        self.records.push(record);
        false
    }

    fn clear(&mut self) {
        self.records.clear();
        self.dropped = 0;
        self.announced = false;
    }
}

#[cfg(feature = "trace")]
mod imp {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::Instant;

    use super::{Buffer, Record};

    /// How many records are retained. Fixed and pre-allocated: growing a
    /// buffer mid-trace would allocate on the path being measured, which is
    /// the one thing this module exists to avoid. Nothing is allocated until
    /// something is actually recorded, so a build carrying the feature with
    /// the environment unset pays nothing for this.
    ///
    /// Oldest records are dropped first. The failures this was built for show
    /// up in the first moments of a run, so keeping the most recent entries is
    /// the wrong bias -- but keeping a bounded window is what stops a long run
    /// consuming the machine, and an overflow now reports itself twice over:
    /// once to stderr as it happens, and again at the head of every dump.
    ///
    /// **Set from measurement.** The buffer is per-process and each test
    /// binary is its own process, so the population that can fill it is one
    /// binary's own run. Both were measured under `WINDOWS_THREADPOOL_TRACE`
    /// set to everything, and they are four orders of magnitude apart: the
    /// figures, and what the larger one cost at the previous value, are in
    /// [the archive entry for
    /// M26.13.2](../../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26132).
    const CAPACITY: usize = 65536;

    static ARMED: AtomicU8 = AtomicU8::new(ARMED_UNKNOWN);
    const ARMED_UNKNOWN: u8 = 0;
    const ARMED_OFF: u8 = 1;
    const ARMED_ON: u8 = 2;

    fn state() -> &'static Mutex<Buffer> {
        static STATE: std::sync::OnceLock<Mutex<Buffer>> = std::sync::OnceLock::new();
        STATE.get_or_init(|| Mutex::new(Buffer::with_capacity(CAPACITY)))
    }

    fn started() -> Instant {
        static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *STARTED.get_or_init(Instant::now)
    }

    fn filters() -> &'static Vec<String> {
        static FILTERS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        FILTERS.get_or_init(|| {
            std::env::var("WINDOWS_THREADPOOL_TRACE")
                .unwrap_or_default()
                .split(',')
                .map(|piece| piece.trim().to_owned())
                .filter(|piece| !piece.is_empty())
                .collect()
        })
    }

    /// Whether anything is being traced at all.
    ///
    /// Cached in a relaxed atomic after the first call, so the steady-state
    /// cost at a disabled call site is one load and a branch. The environment
    /// is read once: re-reading it per call would put a lock and a lookup on
    /// the measured path.
    #[inline(always)]
    pub fn enabled() -> bool {
        match ARMED.load(Ordering::Relaxed) {
            ARMED_ON => true,
            ARMED_OFF => false,
            _ => {
                let on = !filters().is_empty();
                ARMED.store(if on { ARMED_ON } else { ARMED_OFF }, Ordering::Relaxed);
                on
            }
        }
    }

    /// Whether this specific target is being traced.
    #[inline(always)]
    pub fn wants(target: &str) -> bool {
        enabled()
            && filters()
                .iter()
                .any(|filter| filter == "*" || target.contains(filter.as_str()))
    }

    /// Record one observation. Cheap by construction: a clock read, a thread
    /// id, and a push under a lock that is never held across anything slow.
    #[inline]
    pub fn record(target: &'static str, event: &'static str, a: u64, b: u64) {
        if !wants(target) {
            return;
        }
        let at = started().elapsed();
        // SAFETY: no preconditions; returns the calling thread's id.
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let first_eviction = {
            let Ok(mut state) = state().lock() else {
                return;
            };
            state.push(
                Record {
                    at,
                    thread,
                    target,
                    event,
                    a,
                    b,
                },
                CAPACITY,
            )
        };
        // Outside the lock, and only ever once: a capture that has begun
        // discarding its start is a different artifact from one that has not,
        // and a run that never dumps would otherwise never be told. This is
        // the one place the module formats on the traced path, and it is
        // reached only after `CAPACITY` records have already been taken.
        if first_eviction {
            eprintln!(
                "windows-threadpool-sys trace: the {CAPACITY}-record buffer is full and is now \
                 evicting the oldest records, so this capture no longer reaches back to the \
                 start of the run. Narrow WINDOWS_THREADPOOL_TRACE, or raise CAPACITY."
            );
        }
    }

    /// Every record so far, oldest first, formatted for reading.
    pub fn dump() -> String {
        let Ok(state) = state().lock() else {
            return "<trace lock poisoned>".to_owned();
        };
        let mut out = String::with_capacity(state.records.len() * 64);
        if state.dropped > 0 {
            out.push_str(&format!(
                "  ... {} earlier record(s) dropped; raise CAPACITY to keep them\n",
                state.dropped
            ));
        }
        for record in &state.records {
            out.push_str(&format!(
                "  {:>12.6}s t{:<6} {:<22} {:<28} {:>6} {:>6}\n",
                record.at.as_secs_f64(),
                record.thread,
                record.target,
                record.event,
                record.a,
                record.b
            ));
        }
        out
    }

    /// Discard everything recorded so far.
    pub fn clear() {
        if let Ok(mut state) = state().lock() {
            state.clear();
        }
    }
}

#[cfg(not(feature = "trace"))]
mod imp {
    /// Always false in this build: the feature is off.
    #[inline(always)]
    pub fn enabled() -> bool {
        false
    }
    /// Always false in this build: the feature is off.
    #[inline(always)]
    pub fn wants(_target: &str) -> bool {
        false
    }
    /// Does nothing in this build, and is expected to compile away entirely.
    #[inline(always)]
    pub fn record(_target: &'static str, _event: &'static str, _a: u64, _b: u64) {}
    /// Reports that the build carries no trace, which is a different finding
    /// from a build that traced and saw nothing.
    pub fn dump() -> String {
        "<built without the `trace` feature>".to_owned()
    }
    /// Does nothing in this build.
    pub fn clear() {}
}

pub use imp::{clear, dump, enabled, record, wants};

/// How many records so far carry this target and this event.
///
/// Reads [`dump`] rather than the buffer behind it, so it sees exactly what a
/// reader of a capture sees: a call site that records under a target other
/// than the one it means to would be invisible to a check that went behind the
/// formatting, and is the kind of mistake this is used to catch.
#[cfg(test)]
pub(crate) fn counted(target: &str, event: &str) -> usize {
    dump()
        .lines()
        .filter(|line| {
            let mut fields = line.split_whitespace();
            // Elapsed time, then thread id, then the two labels.
            fields.next();
            fields.next();
            fields.next() == Some(target) && fields.next() == Some(event)
        })
        .count()
}

/// Record one observation, evaluating its arguments only when the target is
/// being traced.
///
/// Prefer this to calling [`record`] directly: the macro keeps argument
/// evaluation behind the filter check, so a call site can compute a value that
/// would itself be too expensive for the measured path.
#[macro_export]
macro_rules! trace_record {
    ($target:expr, $event:expr) => {
        $crate::trace::record($target, $event, 0, 0)
    };
    ($target:expr, $event:expr, $a:expr) => {
        if $crate::trace::wants($target) {
            $crate::trace::record($target, $event, $a as u64, 0);
        }
    };
    ($target:expr, $event:expr, $a:expr, $b:expr) => {
        if $crate::trace::wants($target) {
            $crate::trace::record($target, $event, $a as u64, $b as u64);
        }
    };
}

#[cfg(test)]
mod tests;
