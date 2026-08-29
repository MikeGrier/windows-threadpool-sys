// Copyright (c) Mike Grier.

//! How promptly a private thread pool replaces a blocked worker.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! The saturation-response design rests on an assumption the pool API cannot be
//! asked about directly: when callbacks are blocked, work is queued, and the
//! maximum allows, the pool **promptly** creates another thread. "Promptly"
//! also feeds a stall threshold -- a pool that took seconds to grow would make
//! a short threshold meaningless.
//!
//! There is no getter for a pool's thread count, so growth is measured by
//! observation: every callback blocks on one manual-reset event, so a blocked
//! worker is genuinely parked in the kernel rather than spinning, and each
//! callback records its thread id and how long after submission it started.
//!
//! Migrated from the throwaway `pool-probe` spike (Probe P).
//!
//! # Tier: ignored
//!
//! Correct to assert, but it parks real threads and waits on real clocks. It is
//! `#[ignore]`d rather than deleted so it can be run deliberately -- notably on
//! a new architecture, where the growth timing is exactly what might differ.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentThreadId, INFINITE, SetEvent, WaitForSingleObject,
};
use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::pool::ThreadpoolPool;
use windows_threadpool_sys::work::ThreadpoolWork;

/// A manual-reset event every callback parks on, so "blocked" means blocked.
struct Gate(HANDLE);

// SAFETY: a Windows event handle is process-wide rather than thread-affine, and
// every use here either waits on it or signals it, both of which Windows
// serialises internally.
unsafe impl Send for Gate {}
// SAFETY: as above.
unsafe impl Sync for Gate {}

impl Gate {
    /// Creates a manual-reset event, initially unsignalled.
    ///
    /// # Panics
    ///
    /// Panics if the event cannot be created, since a probe that measured
    /// nothing would be worse than one that stopped.
    fn new() -> Self {
        // SAFETY: null attributes and name are valid; TRUE selects manual
        // reset, FALSE leaves it unsignalled.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        assert!(!handle.is_null(), "create the gate event");
        Self(handle)
    }

    /// Blocks until the gate is opened.
    fn wait(&self) {
        // SAFETY: the handle is live for this value's lifetime.
        unsafe { WaitForSingleObject(self.0, INFINITE) };
    }

    /// Releases every waiter, and every future waiter.
    fn open(&self) {
        // SAFETY: as above.
        unsafe { SetEvent(self.0) };
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        // SAFETY: the handle is live and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// What one saturation run observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrowthObservation {
    /// The pool maximum the run was configured with.
    pub maximum: u32,
    /// How many callbacks were submitted.
    pub submitted: usize,
    /// How many callbacks had started before the gate was opened.
    ///
    /// This is the concurrency the pool actually reached while every started
    /// callback was parked.
    pub started_while_blocked: usize,
    /// How many distinct threads those callbacks ran on.
    pub distinct_threads: usize,
    /// Microseconds from submission to start, in arrival order.
    pub arrivals_us: Vec<u128>,
}

impl GrowthObservation {
    /// The pool reached its configured maximum concurrency.
    #[must_use]
    pub fn saturated(&self) -> bool {
        self.started_while_blocked >= self.maximum as usize
    }

    /// Every started callback ran on its own thread.
    ///
    /// Blocked callbacks cannot share a thread, so anything else would mean the
    /// probe measured something other than it thinks.
    #[must_use]
    pub fn one_thread_each(&self) -> bool {
        self.distinct_threads == self.started_while_blocked
    }

    /// The slowest thread to arrive, which is what a stall threshold must
    /// tolerate.
    #[must_use]
    pub fn slowest_arrival(&self) -> Duration {
        Duration::from_micros(
            u64::try_from(self.arrivals_us.iter().copied().max().unwrap_or(0)).unwrap_or(u64::MAX),
        )
    }

    /// The largest gap between consecutive arrivals.
    ///
    /// This is the number a stall threshold actually has to tolerate, and it is
    /// **not** the same as the slowest arrival: growth is not uniform. See
    /// [`throttles_after`](Self::throttles_after).
    #[must_use]
    pub fn largest_gap(&self) -> Duration {
        let largest = self
            .arrivals_us
            .windows(2)
            .map(|pair| pair[1].saturating_sub(pair[0]))
            .max()
            .unwrap_or(0);

        Duration::from_micros(u64::try_from(largest).unwrap_or(u64::MAX))
    }

    /// How many workers arrived before growth visibly throttled.
    ///
    /// **Measured, and the most useful thing this probe found.** Growth is not
    /// uniform: an initial burst arrives essentially immediately, and beyond
    /// that the pool adds roughly one thread per throttle interval. A design
    /// that sized a stall threshold from the burst would be badly wrong about
    /// the tail.
    ///
    /// Returns the number of arrivals that came in faster than `threshold`
    /// after their predecessor. `None` when growth never throttled, which is
    /// what a small enough run reports.
    #[must_use]
    pub fn throttles_after(&self, threshold: Duration) -> Option<usize> {
        let threshold_us = u128::from(threshold.as_micros() as u64);

        self.arrivals_us
            .windows(2)
            .position(|pair| pair[1].saturating_sub(pair[0]) >= threshold_us)
            .map(|index| index + 1)
    }
}

/// Opens the gate when it goes out of scope, including while unwinding.
///
/// # Why this exists and where it must be declared
///
/// `ThreadpoolWork`'s `Drop` waits for its callback to return rather than
/// cancelling it, and every callback here parks on the gate. So a panic
/// anywhere between the first submission and the explicit `gate.open()` would
/// deadlock the process **permanently** -- not fail the test, and not time
/// out.
///
/// Opening the gate from `Gate`'s own `Drop` would not fix it. Locals are
/// dropped in reverse declaration order, and the work items are declared
/// before the gate, so the items would be dropped -- and block -- while the
/// gate was still alive.
///
/// This guard must therefore be declared **after** the work items, so that it
/// is dropped **before** them. `Gate::open` sets a manual-reset event, so
/// calling it here as well as on the normal path is harmless.
struct GateGuard(Arc<Gate>);

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// Records what each callback saw.
#[derive(Default)]
struct Log {
    entries: Mutex<Vec<(u32, u128)>>,
    started: AtomicUsize,
    /// When submission began, which is the baseline every arrival is measured
    /// from.
    ///
    /// Set once, immediately before the first submission, rather than captured
    /// when the items were created -- creation is now a separate earlier pass,
    /// and charging its cost to the arrival offsets would inflate exactly the
    /// microsecond figures this probe reports.
    submitted_at: OnceLock<Instant>,
}

/// Saturates a pool of `maximum` threads with `submissions` blocking callbacks
/// and reports how it grew.
///
/// `runs_long` selects `SetThreadpoolCallbackRunsLong`, which is documented to
/// make the pool create threads more eagerly when callbacks block -- so the two
/// settings are worth comparing rather than assuming.
///
/// # Panics
///
/// Panics if the pool or its work items cannot be created.
///
/// It does **not** panic when the pool fails to reach its maximum: that is a
/// finding, reported through [`GrowthObservation::saturated`], and the tests
/// assert it. An earlier version of this comment claimed the panic, which was
/// worth correcting rather than implementing -- a probe that aborts on an
/// unexpected platform result cannot report it.
#[must_use]
pub fn measure_growth(maximum: u32, submissions: usize, runs_long: bool) -> GrowthObservation {
    /// Long enough for a healthy pool to grow well inside it, short enough that
    /// a pathological one is reported rather than waited on.
    const SETTLE: Duration = Duration::from_secs(2);

    let pool = ThreadpoolPool::new().expect("create a private pool");
    pool.set_max_threads(maximum).expect("set the pool maximum");
    pool.set_min_threads(1).expect("set the pool minimum");

    let mut environment = CallbackEnviron::new();
    environment.set_pool(&pool);
    if runs_long {
        environment.set_runs_long();
    }

    let gate = Arc::new(Gate::new());
    let log = Arc::new(Log::default());

    // Creation is a separate pass from submission: nothing is parked on the
    // gate until the loop below, so a failure here needs no guard.
    let items: Vec<_> = (0..submissions)
        .map(|_| {
            let gate = Arc::clone(&gate);
            let log = Arc::clone(&log);

            ThreadpoolWork::new(
                move || {
                    // SAFETY: GetCurrentThreadId has no preconditions.
                    let thread = unsafe { GetCurrentThreadId() };
                    let elapsed = log
                        .submitted_at
                        .get()
                        .map_or(0, |start| start.elapsed().as_micros());
                    log.entries
                        .lock()
                        .expect("the log is not poisoned")
                        .push((thread, elapsed));
                    log.started.fetch_add(1, Ordering::SeqCst);
                    gate.wait();
                },
                Some(&mut environment),
            )
            .expect("create a work item")
        })
        .collect();

    // Declared after `items` so it is dropped before them. See GateGuard.
    let _gate_guard = GateGuard(Arc::clone(&gate));

    log.submitted_at
        .set(Instant::now())
        .expect("the submission time is set exactly once");
    for item in &items {
        item.submit();
    }

    // Let the pool grow. Stop early once it has plainly saturated, so a healthy
    // pool does not pay the whole settle window.
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline && log.started.load(Ordering::SeqCst) < maximum as usize {
        std::thread::sleep(Duration::from_millis(5));
    }

    let started_while_blocked = log.started.load(Ordering::SeqCst);
    let entries = log.entries.lock().expect("the log is not poisoned").clone();

    // Release everything before the pool drops: a callback still parked at
    // teardown would block Drop by design.
    gate.open();
    for item in &items {
        item.wait();
    }

    let mut threads: Vec<u32> = entries.iter().map(|(thread, _)| *thread).collect();
    threads.sort_unstable();
    threads.dedup();

    let mut arrivals_us: Vec<u128> = entries.iter().map(|(_, at)| *at).collect();
    arrivals_us.sort_unstable();

    GrowthObservation {
        maximum,
        submitted: submissions,
        started_while_blocked,
        distinct_threads: threads.len(),
        arrivals_us,
    }
}

/// What the raise-while-saturated probe observed.
///
/// The delay is only meaningful when [`saturated_before_raise`] holds. If the
/// pool had not actually reached `base_max` when the maximum was raised, the
/// delay times ordinary growth *toward the base maximum* rather than the
/// effect of the raise -- a small number that looks like a pass while
/// measuring the wrong thing entirely.
///
/// [`saturated_before_raise`]: Self::saturated_before_raise
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaiseObservation {
    /// The maximum the pool was held at before the raise.
    pub base_max: u32,
    /// Callbacks started at the moment the maximum was raised.
    pub started_before_raise: usize,
    /// How long after the raise the first extra callback started.
    ///
    /// The settle window elapsed when [`took_effect`](Self::took_effect) is
    /// false, so compare that first.
    pub delay: Duration,
    /// An extra callback started within the settle window.
    pub took_effect: bool,
}

impl RaiseObservation {
    /// The pool really was saturated at its base maximum when the raise
    /// happened, so [`delay`](Self::delay) measures the raise.
    #[must_use]
    pub fn saturated_before_raise(self) -> bool {
        self.started_before_raise == self.base_max as usize
    }
}

/// Saturates at `base_max`, raises the maximum to `raised_max`, and reports how
/// long the extra callbacks took to start.
///
/// This is the exact mechanism of "raise the pool size to compensate for a
/// blocked worker", so the delay is the number that matters.
///
/// # Panics
///
/// As [`measure_growth`]: failing to saturate is reported through
/// [`RaiseObservation::saturated_before_raise`], not raised.
#[must_use]
pub fn measure_raise_while_saturated(
    base_max: u32,
    raised_max: u32,
    submissions: usize,
) -> RaiseObservation {
    /// How long to let the raise take effect before giving up on it.
    const SETTLE: Duration = Duration::from_secs(2);

    let pool = ThreadpoolPool::new().expect("create a private pool");
    pool.set_max_threads(base_max).expect("set the maximum");
    pool.set_min_threads(1).expect("set the minimum");

    let mut environment = CallbackEnviron::new();
    environment.set_pool(&pool);

    let gate = Arc::new(Gate::new());
    let log = Arc::new(Log::default());

    // Created first, submitted second, for the reason given in measure_growth.
    let items: Vec<_> = (0..submissions)
        .map(|_| {
            let gate = Arc::clone(&gate);
            let log = Arc::clone(&log);
            ThreadpoolWork::new(
                move || {
                    log.started.fetch_add(1, Ordering::SeqCst);
                    gate.wait();
                },
                Some(&mut environment),
            )
            .expect("create a work item")
        })
        .collect();

    // Declared after `items` so it is dropped before them. See GateGuard.
    let _gate_guard = GateGuard(Arc::clone(&gate));

    for item in &items {
        item.submit();
    }

    // Wait for saturation at the base maximum.
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline && log.started.load(Ordering::SeqCst) < base_max as usize {
        std::thread::sleep(Duration::from_millis(5));
    }

    // Whatever the settle loop above reached -- which is not necessarily
    // `base_max`, since that loop also exits on timeout. Recorded rather than
    // assumed, because the delay measured below only means what it claims when
    // this equals `base_max`.
    let before = log.started.load(Ordering::SeqCst);
    let raised_at = Instant::now();
    pool.set_max_threads(raised_max).expect("raise the maximum");

    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline && log.started.load(Ordering::SeqCst) <= before {
        std::thread::sleep(Duration::from_millis(1));
    }
    let took_effect = log.started.load(Ordering::SeqCst) > before;
    let delay = raised_at.elapsed();

    gate.open();
    for item in &items {
        item.wait();
    }

    RaiseObservation {
        base_max,
        started_before_raise: before,
        delay,
        took_effect,
    }
}
