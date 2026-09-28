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
mod hook;

/// Arm the trace, and install any requested hooks, **before `main`**.
///
/// `.CRT$XCU` is the C runtime's static-initialiser table; a function pointer
/// placed in it is called during CRT startup, which for a Rust binary is
/// before `main` and therefore before the test harness has created a single
/// thread.
///
/// This exists because lazy installation was measured to be too late. Hooks
/// were previously installed on the first traced call, which is already inside
/// the first test: in the `M26.13` reproducer they landed between 0.15 s and
/// 0.58 s, while the fault under investigation is established in the first
/// 15.7 ms. An instrument that arrives after the event cannot observe it.
///
/// Installing here is also **cheaper and safer**, not merely earlier. Patching
/// live code requires every other thread to be stopped, and at this point
/// there are none to stop -- so the suspend-and-resume pass finds nothing, the
/// perturbation it would otherwise cause does not happen, and the whole
/// question of suspending a thread that holds a lock does not arise.
///
/// It does nothing unless the environment asks for it: no
/// `WINDOWS_THREADPOOL_TRACE`, no arming, and no
/// `WINDOWS_THREADPOOL_TRACE_HOOKS`, no patching.
#[cfg(feature = "trace")]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static ARM_BEFORE_MAIN: extern "C" fn() = {
    extern "C" fn arm() {
        ARMED_BEFORE_MAIN.store(true, std::sync::atomic::Ordering::Relaxed);
        // `enabled` is the single path that arms the trace, starts its clock,
        // installs the exception observer and requests the hooks. Calling it
        // rather than any of those directly keeps one order of operations
        // rather than two that have to be kept in step.
        let _ = imp::enabled();
    }
    arm
};

/// Whether [`ARM_BEFORE_MAIN`] ran.
///
/// Set only from inside the static initialiser and never anywhere else, so it
/// distinguishes "ran before `main`" from "was arranged and silently dropped".
/// That is the failure this needs a guard for: a static nothing references is
/// exactly what a linker is entitled to discard, `#[used]` is what asks it not
/// to, and the symptom of getting that wrong is not a build error but an
/// instrument that quietly reverts to installing too late.
#[cfg(feature = "trace")]
static ARMED_BEFORE_MAIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the trace's static initialiser ran before `main`.
#[cfg(all(test, feature = "trace"))]
pub(crate) fn armed_before_main() -> bool {
    ARMED_BEFORE_MAIN.load(std::sync::atomic::Ordering::Relaxed)
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
    /// **Set from measurement, and re-measured when the trace grows.** The
    /// buffer is per-process and each test binary is its own process, so the
    /// population that can fill it is one binary's own run. Two measurements,
    /// both under `WINDOWS_THREADPOOL_TRACE` set to everything:
    ///
    /// | | records |
    /// |---|---|
    /// | `windows-ioring-sys`' `event_delivery`, the only binary that captures | 89 |
    /// | this crate's own lib tests, 2026-09-26 | 14061 |
    /// | this crate's own lib tests, after the call-boundary and exception targets | 47997 |
    ///
    /// The third figure is why this is not 65536 any more: adding
    /// `syscall-enter`/`syscall-leave` and `exception` tripled the volume and
    /// left only a third of a buffer spare. The figures, and what the smallest
    /// value cost, are in [the archive entry for
    /// M26.13.2](../../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26132).
    const CAPACITY: usize = 262_144;

    static ARMED: AtomicU8 = AtomicU8::new(ARMED_UNKNOWN);
    const ARMED_UNKNOWN: u8 = 0;
    const ARMED_OFF: u8 = 1;
    const ARMED_ON: u8 = 2;

    static STATE: std::sync::OnceLock<Mutex<Buffer>> = std::sync::OnceLock::new();
    static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    static FILTERS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

    fn state() -> &'static Mutex<Buffer> {
        STATE.get_or_init(|| Mutex::new(Buffer::with_capacity(CAPACITY)))
    }

    fn started() -> Instant {
        *STARTED.get_or_init(Instant::now)
    }

    fn filters() -> &'static Vec<String> {
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
                if on {
                    observe_exceptions();
                }
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
                "  {:>12.6}s t{:<6} {:<22} {:<36} {:>6} {:>6}\n",
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

    /// The target every exception observation is recorded under.
    const EXCEPTION_TARGET: &str = "exception";

    /// Tell the OS this handler did not handle the exception, so the search
    /// continues exactly as it would have without us. Named rather than
    /// written as a bare literal: this value is the whole of the promise that
    /// installing the observer changes no behaviour.
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    /// Ask to be called *before* any previously installed handler, which is
    /// what makes the timestamp the moment of the raise rather than the moment
    /// some other handler declined it.
    const CALL_FIRST: u32 = 1;

    /// Record one observation from a context that must neither block nor
    /// initialise anything.
    ///
    /// A vectored exception handler runs on whatever thread raised, at
    /// whatever point it raised -- including, in principle, a thread that is
    /// inside [`record`] holding the buffer lock. So this takes none of the
    /// paths that could deadlock or allocate:
    ///
    /// - every `OnceLock` is read with `get`, never `get_or_init`, so a first
    ///   exception arriving before the trace has initialised is dropped rather
    ///   than initialising the trace from inside an exception handler;
    /// - the buffer lock is taken with `try_lock`, so an exception raised by a
    ///   thread already holding it drops the record instead of deadlocking;
    /// - the eviction announcement is deliberately ignored, because writing to
    ///   stderr from an exception handler is not something this facility
    ///   should do uninvited.
    ///
    /// The cost of all three is the same: a lost record, which is the right
    /// trade for an observer whose entire purpose is to change nothing.
    fn record_without_blocking(event: &'static str, a: u64, b: u64) {
        if ARMED.load(Ordering::Relaxed) != ARMED_ON {
            return;
        }
        let Some(filters) = FILTERS.get() else {
            return;
        };
        if !filters
            .iter()
            .any(|filter| filter == "*" || EXCEPTION_TARGET.contains(filter.as_str()))
        {
            return;
        }
        let (Some(started), Some(state)) = (STARTED.get(), STATE.get()) else {
            return;
        };
        let at = started.elapsed();
        // SAFETY: no preconditions; returns the calling thread's id.
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let Ok(mut state) = state.try_lock() else {
            return;
        };
        let _ = state.push(
            Record {
                at,
                thread,
                target: EXCEPTION_TARGET,
                event,
                a,
                b,
            },
            CAPACITY,
        );
    }

    /// Note that an exception was raised, and decline to handle it.
    ///
    /// SAFETY: this is the `PVECTORED_EXCEPTION_HANDLER` ABI. `info` is
    /// supplied by the OS and is valid for the duration of the call.
    unsafe extern "system" fn exception_observer(
        info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
    ) -> i32 {
        if !info.is_null() {
            // SAFETY: non-null and OS-supplied for this call.
            let record = unsafe { (*info).ExceptionRecord };
            if !record.is_null() {
                // SAFETY: as above; the record outlives this call.
                let (code, address) =
                    unsafe { ((*record).ExceptionCode, (*record).ExceptionAddress) };
                record_without_blocking("raised", code as u32 as u64, address as u64);
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    /// Start noting every exception raised in this process, once.
    ///
    /// Installed when the trace turns on rather than on demand, so that no
    /// caller has to remember: the window an investigation cares about has
    /// usually opened before anyone would think to ask for this.
    ///
    /// **It observes and does nothing else.** The handler returns
    /// `EXCEPTION_CONTINUE_SEARCH`, so every exception is dispatched exactly
    /// as it would have been -- including first-chance exceptions a later
    /// handler goes on to swallow, which is precisely the population a normal
    /// debugger view hides and an investigation may want.
    pub fn observe_exceptions() {
        use std::sync::atomic::AtomicBool;
        static INSTALLED: AtomicBool = AtomicBool::new(false);
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        // Force the clock and the buffer into existence *here*, in an ordinary
        // context, because the handler deliberately refuses to initialise
        // either. Without this an exception raised before the first ordinary
        // record would be dropped -- which is the window an investigation is
        // most likely to care about, since it is the one before anything has
        // happened yet.
        let _ = started();
        let _ = state();
        // SAFETY: the handler matches the documented ABI, has static lifetime,
        // and is never removed.
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(
                CALL_FIRST,
                Some(exception_observer),
            )
        };
        // Planting code over `ntdll` needs its own opt-in and does nothing
        // without one; see `trace::hook`. It is placed here so that a process
        // that asked for hooks gets them as early as the trace itself, which
        // for the worker factory means before the pool has any thread.
        super::hook::install_requested();
    }

    /// Record what the default pool's worker factory believes about itself.
    ///
    /// Answers the question every outside measurement leaves open: the pool
    /// has parked workers and a queued packet, so does the *factory* think it
    /// has an available worker? Returns whether anything was recorded; it
    /// needs a handle, which only a hooked call can supply, so it reports
    /// `false` when hooks were not installed or have not yet fired.
    pub fn worker_factory_counts() -> bool {
        super::hook::counts()
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
    /// Does nothing in this build: there is no trace to attribute exceptions
    /// to, so no handler is installed.
    pub fn observe_exceptions() {}
    /// Reports that this build cannot read the factory's counters, which is a
    /// different finding from a build that read them and saw nothing.
    pub fn worker_factory_counts() -> bool {
        false
    }
}

pub use imp::{clear, dump, enabled, observe_exceptions, record, wants, worker_factory_counts};

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

/// Bracket a Win32 call that blocks, or that takes a lock inside the pool.
///
/// Records the call's name under `syscall-enter` before it and `syscall-leave`
/// after, so a call that blocked shows up as an *interval* rather than as a
/// record with a later timestamp than you expected. The difference matters:
/// a single record stamped after the call returns cannot distinguish "this was
/// issued late" from "this took four seconds to return".
///
/// Every thread-pool API touches the pool's own synchronisation, and several
/// block outright -- the `WaitForThreadpool*Callbacks` family,
/// `CloseThreadpoolCleanupGroupMembers`, `CloseThreadpool`, and
/// `SetThreadpoolThreadMinimum`, which creates threads. Which of the rest can
/// contend is not documented, so they are bracketed too rather than assumed
/// cheap: the whole point is to catch synchronisation nobody expected.
///
/// **It is its own target**, so it can be switched on without it. A capture
/// narrowed to `wait,delivery` is unchanged by this existing; one narrowed to
/// `syscall` sees only the call boundaries. Argument expressions are evaluated
/// only when the target is being traced, and the whole thing compiles to the
/// call alone without the `trace` feature.
///
/// ```ignore
/// crate::trace_call!("SetThreadpoolWait", wait, handle as usize, {
///     // SAFETY: ...
///     unsafe { SetThreadpoolWait(wait, handle, ptr::null()) }
/// })
/// ```
#[macro_export]
macro_rules! trace_call {
    ($name:literal, $a:expr, $b:expr, $call:block) => {{
        $crate::trace_record!("syscall-enter", $name, $a, $b);
        let result = $call;
        $crate::trace_record!("syscall-leave", $name, $a, $b);
        result
    }};
}

#[cfg(test)]
mod tests;
