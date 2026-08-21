// Copyright (c) Mike Grier.

//! Opt-in stress tests for the timer APIs.
//!
//! These apply load to the contracts the timer types make: a one-shot's
//! self-re-arm never overlaps itself, a periodic's ticks explicitly may, and
//! both gate re-arming against teardown. The unit tests establish each contract
//! once and deterministically; this suite pushes on it.
//!
//! # Running
//!
//! Nothing here runs unless `WINDOWS_THREADPOOL_STRESS` is set to `1`, `true`,
//! `yes`, or `on`. Every test still compiles and lints, so the suite cannot rot,
//! but a plain `cargo test` -- including the one CI runs -- skips the load.
//!
//! ```text
//! $env:WINDOWS_THREADPOOL_STRESS = "1"
//! cargo test -p windows-threadpool-sys --test timer_stress -- --nocapture
//! ```
//!
//! `WINDOWS_THREADPOOL_STRESS_SCALE` multiplies every load count, so the same
//! scenarios can be run harder without editing them:
//!
//! ```text
//! $env:WINDOWS_THREADPOOL_STRESS_SCALE = "10"
//! ```
//!
//! # Timer resolution shapes these scenarios
//!
//! Pool timers fire on the system timer tick, which is ~15.6ms by default. A
//! re-arm with a zero delay therefore does not fire "immediately" -- it fires on
//! the next tick. A self-re-arming chain runs at roughly 64 iterations a second
//! however little the callback does, so chain lengths here are deliberately
//! modest; raising them buys wall-clock time, not additional coverage.
//!
//! The same tick explains why a scenario that arms and disarms in a tight loop
//! records few firings or none: the pool never reaches a tick with the timer
//! still armed. Those scenarios stress the arming path and the object's state
//! integrity, and report their firing counts rather than asserting them.
//!
//! # What is asserted
//!
//! Only what is genuinely invariant under load: non-overlap where the type
//! guarantees it, quiescence after a drain, and the absence of a hang or a
//! crash. Rates, latencies, and exact fire counts are *reported* rather than
//! asserted -- under load those are properties of the machine, not of the code,
//! and asserting them would produce a suite that fails for the wrong reasons.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use windows_threadpool_sys::cleanup_group::CleanupGroup;
use windows_threadpool_sys::timer::{ThreadpoolPeriodicTimer, ThreadpoolTimer};

// --- gating ---

/// Set this to `1` to run the suite. Absent, every scenario returns immediately.
const ENABLE_VAR: &str = "WINDOWS_THREADPOOL_STRESS";

/// Multiplies every load count, so the suite can be run harder unedited.
const SCALE_VAR: &str = "WINDOWS_THREADPOOL_STRESS_SCALE";

fn enabled() -> bool {
    std::env::var_os(ENABLE_VAR).is_some_and(|raw| {
        matches!(
            raw.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn scale() -> usize {
    std::env::var(SCALE_VAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1)
}

/// A load count, scaled by [`SCALE_VAR`].
fn load(base: usize) -> usize {
    base.saturating_mul(scale())
}

/// Heavy scenarios run one at a time.
///
/// Cargo runs the tests in a binary on parallel threads, and every scenario here
/// drives the same process-wide thread pool. Left to overlap they would contend
/// for pool threads and for the CPU, which makes the timing-dependent parts of
/// each scenario a measurement of the other scenarios rather than of the code.
static LANE: Mutex<()> = Mutex::new(());

fn enter_lane(name: &str) -> Option<MutexGuard<'static, ()>> {
    if !enabled() {
        eprintln!("stress: skipping {name} -- set {ENABLE_VAR}=1 to run");
        return None;
    }
    let lane = LANE.lock().unwrap_or_else(|poison| poison.into_inner());
    eprintln!("stress: running {name} at scale {}", scale());
    Some(lane)
}

/// Define a stress scenario, gated on [`ENABLE_VAR`].
///
/// The gate is a macro rather than a line inside each test on purpose: a gate
/// that must be remembered per test is one that will eventually be forgotten in
/// exactly one of them, and that test is then a load test running in CI.
macro_rules! stress {
    ($(#[$meta:meta])* $name:ident $body:block) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let Some(_lane) = enter_lane(stringify!($name)) else {
                return;
            };
            let started = Instant::now();
            $body
            eprintln!("stress: {} finished in {:?}", stringify!($name), started.elapsed());
        }
    };
}

// --- observation helpers ---

/// A counter a callback can bump and a test can wait on.
struct Tally {
    count: Mutex<usize>,
    changed: Condvar,
}

impl Tally {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            count: Mutex::new(0),
            changed: Condvar::new(),
        })
    }

    fn record(&self) -> usize {
        let mut count = self.count.lock().unwrap_or_else(|p| p.into_inner());
        *count += 1;
        let now = *count;
        self.changed.notify_all();
        now
    }

    fn count(&self) -> usize {
        *self.count.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait until the count reaches `target`, or `timeout` elapses.
    ///
    /// Returns the count actually reached, so a caller can report a shortfall
    /// rather than deadlocking on one.
    fn wait_for(&self, target: usize, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        let mut count = self.count.lock().unwrap_or_else(|p| p.into_inner());
        while *count < target {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let (next, _) = self
                .changed
                .wait_timeout(count, remaining)
                .unwrap_or_else(|p| p.into_inner());
            count = next;
        }
        *count
    }
}

/// Detects whether two callbacks of the same object are ever inside at once.
///
/// A one-shot that re-arms itself is documented never to overlap, because the
/// re-arm is applied after the callback returns. This is how that is checked
/// under load: any entry that finds the flag already set is a violation.
struct Overlap {
    inside: AtomicBool,
    violations: AtomicUsize,
    peak_concurrent: AtomicUsize,
    concurrent: AtomicUsize,
}

impl Overlap {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inside: AtomicBool::new(false),
            violations: AtomicUsize::new(0),
            peak_concurrent: AtomicUsize::new(0),
            concurrent: AtomicUsize::new(0),
        })
    }

    fn enter(self: &Arc<Self>) -> OverlapGuard<'_> {
        if self.inside.swap(true, Ordering::SeqCst) {
            self.violations.fetch_add(1, Ordering::SeqCst);
        }
        let now = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_concurrent.fetch_max(now, Ordering::SeqCst);
        OverlapGuard(self)
    }

    fn violations(&self) -> usize {
        self.violations.load(Ordering::SeqCst)
    }

    fn peak_concurrent(&self) -> usize {
        self.peak_concurrent.load(Ordering::SeqCst)
    }
}

struct OverlapGuard<'a>(&'a Arc<Overlap>);

impl Drop for OverlapGuard<'_> {
    fn drop(&mut self) {
        self.0.concurrent.fetch_sub(1, Ordering::SeqCst);
        self.0.inside.store(false, Ordering::SeqCst);
    }
}

/// Burn a little time inside a callback, varying deterministically with `step`.
///
/// Deterministic rather than random so a failure is reproducible; the point is
/// only that callbacks do not all take exactly the same time, which would make
/// them line up and hide interleavings.
fn work_for(step: usize) {
    let micros = u64::try_from(step % 5).unwrap_or(0) * 200;
    if micros > 0 {
        std::thread::sleep(Duration::from_micros(micros));
    }
}

/// Assert that nothing more fires once the caller has drained the object.
///
/// A drained timer is quiescent, and that *is* invariant under load: it is the
/// property teardown and cancellation exist to provide.
fn assert_quiescent(tally: &Tally, label: &str) {
    let settled = tally.count();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        tally.count(),
        settled,
        "{label}: a callback ran after the timer was drained"
    );
}

// --- one-shot: self-re-arming chains ---

stress! {
    /// A one-shot that re-arms itself must never be entered twice at once. The
    /// re-arm is applied after the callback returns, so the delay runs from the
    /// end of the firing; that is the guarantee this hammers.
    stress_one_shot_self_rearm_never_overlaps {
        // Each link in the chain costs a timer tick (~15.6ms), so this is sized
        // for wall-clock time rather than for a round number of iterations.
        let target = load(300);
        let overlap = Overlap::new();
        let tally = Tally::new();

        let seen = Arc::clone(&overlap);
        let counter = Arc::clone(&tally);
        let timer = ThreadpoolTimer::new(
            move |firing| {
                let _inside = seen.enter();
                let n = counter.record();
                work_for(n);
                if n < target {
                    // Zero delay: re-arm as aggressively as the API allows.
                    firing.rearm_after(Duration::ZERO);
                }
            },
            None,
        )
        .expect("create timer");

        let chain_started = Instant::now();
        timer.set_after(Duration::ZERO);
        let reached = tally.wait_for(target, Duration::from_secs(300));
        let chain_elapsed = chain_started.elapsed();
        timer.disarm();
        timer.cancel_pending();

        assert_eq!(
            overlap.violations(),
            0,
            "a self-re-arming one-shot overlapped itself"
        );
        assert_eq!(reached, target, "the re-arm chain stalled");
        assert_quiescent(&tally, "self-re-arm chain");

        // Reported so the tick granularity is visible to whoever runs this: a
        // mean near 15.6ms is the timer, not the callback.
        eprintln!(
            "stress:   {reached} links in {chain_elapsed:?} ({:?} mean per link)",
            chain_elapsed / u32::try_from(reached.max(1)).unwrap_or(1)
        );
    }
}

stress! {
    /// The same chain driven through `rearm_at` with instants already in the
    /// past, which the type documents as firing immediately. This is the path
    /// that converts an absolute time to a due time, rather than a relative one.
    stress_one_shot_rearm_at_past_instants {
        let target = load(200);
        let overlap = Overlap::new();
        let tally = Tally::new();

        let seen = Arc::clone(&overlap);
        let counter = Arc::clone(&tally);
        let timer = ThreadpoolTimer::new(
            move |firing| {
                let _inside = seen.enter();
                let n = counter.record();
                if n < target {
                    let past = SystemTime::now() - Duration::from_secs(1);
                    firing.rearm_at(past);
                }
            },
            None,
        )
        .expect("create timer");

        timer.set_at(SystemTime::now() - Duration::from_secs(1));
        let reached = tally.wait_for(target, Duration::from_secs(300));
        timer.disarm();
        timer.cancel_pending();

        assert_eq!(overlap.violations(), 0, "a past-instant chain overlapped");
        assert_eq!(reached, target, "the past-instant chain stalled");
        assert_quiescent(&tally, "past-instant chain");
    }
}

// --- one-shot: external arming under contention ---

stress! {
    /// Many threads arming, disarming, and querying one timer while it fires.
    ///
    /// Non-overlap is deliberately *not* asserted here: it is guaranteed for a
    /// callback's own re-arm, not for an external `set_after` landing while a
    /// callback runs, which the type documents. What must hold is that the churn
    /// neither hangs nor leaves the timer live after it is drained.
    stress_one_shot_external_arming_churn {
        let threads = 8;
        // Sized against the pause below rather than raw op count: without pauses
        // longer than a timer tick, the loop outruns the pool and the callback
        // never runs at all, which would make this a test of arming only.
        let per_thread = load(400);
        let tally = Tally::new();
        let overlap = Overlap::new();

        let counter = Arc::clone(&tally);
        let seen = Arc::clone(&overlap);
        let timer = Arc::new(
            ThreadpoolTimer::new(
                move |_firing| {
                    let _inside = seen.enter();
                    let n = counter.record();
                    work_for(n);
                },
                None,
            )
            .expect("create timer"),
        );

        let workers: Vec<_> = (0..threads)
            .map(|t| {
                let timer = Arc::clone(&timer);
                std::thread::spawn(move || {
                    for i in 0..per_thread {
                        // Arming dominates the mix deliberately. With disarms at
                        // a quarter of the operations across eight threads the
                        // timer is almost never left armed when a tick arrives,
                        // and the scenario degenerates into a test of the arming
                        // calls with the callback path barely reached.
                        match (t + i) % 8 {
                            0..=2 => timer.set_after(Duration::ZERO),
                            3 => timer.set_after(Duration::from_micros(50)),
                            4 => timer.disarm(),
                            _ => {
                                let _ = timer.is_set();
                            }
                        }
                        // Long enough for the pool to reach a tick with the
                        // timer still armed, so callbacks genuinely interleave
                        // with the arming rather than being outrun by it.
                        if i % 4 == 0 {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().expect("arming thread");
        }

        timer.disarm();
        timer.cancel_pending();

        // The overlap counts are reported, not asserted, in either direction: an
        // external arming that lands while a callback runs is allowed to overlap
        // it, so a non-zero count is correct behaviour, and a zero count only
        // means this run did not happen to hit the window.
        let fired = tally.count();
        eprintln!(
            "stress:   fired {fired} times under arming churn; {} overlapping entries, peak {} concurrent",
            overlap.violations(),
            overlap.peak_concurrent()
        );
        // This one *is* asserted: each thread leaves the timer armed across many
        // pauses longer than a timer tick, so a run that never fires means the
        // scenario silently degraded into a test of the arming calls alone.
        assert!(
            fired > 0,
            "the timer never fired under arming churn -- the loop is outrunning the pool"
        );
        assert!(!timer.is_set(), "the timer is still armed after disarm");
        assert_quiescent(&tally, "arming churn");
    }
}

stress! {
    /// Two threads racing `set_after(0)` against `disarm`, which is the tightest
    /// arm/disarm window the API exposes. The final state must follow the last
    /// call, not the race.
    stress_one_shot_arm_disarm_race {
        let rounds = load(20_000);
        let tally = Tally::new();

        let counter = Arc::clone(&tally);
        let timer = Arc::new(
            ThreadpoolTimer::new(
                move |_firing| {
                    counter.record();
                },
                None,
            )
            .expect("create timer"),
        );

        let armer = {
            let timer = Arc::clone(&timer);
            std::thread::spawn(move || {
                for _ in 0..rounds {
                    timer.set_after(Duration::ZERO);
                }
            })
        };
        let disarmer = {
            let timer = Arc::clone(&timer);
            std::thread::spawn(move || {
                for _ in 0..rounds {
                    timer.disarm();
                }
            })
        };

        armer.join().expect("arming thread");
        disarmer.join().expect("disarming thread");

        timer.disarm();
        timer.cancel_pending();

        eprintln!("stress:   fired {} times across {rounds} races", tally.count());
        assert!(!timer.is_set(), "the timer is still armed after disarm");
        assert_quiescent(&tally, "arm/disarm race");
    }
}

stress! {
    /// Many threads each driving their own timer through a full arm-fire-rearm
    /// cycle, waiting for each firing before arming again.
    ///
    /// Unlike the churn scenarios, this one controls its own timing: it does not
    /// proceed until the firing it is waiting for has happened, so every round
    /// is guaranteed to exercise the dispatch path and the exact firing count is
    /// a safe assertion rather than a timing measurement.
    ///
    /// The count is recorded *after* the overlap guard is released, which is
    /// what makes the driving loop genuinely serial. Recording it while still
    /// inside the guard -- as an earlier version did -- released the waiting
    /// thread mid-callback, so it armed again while the previous callback was
    /// still running and the two overlapped. That is permitted behaviour for an
    /// external arming, so the resulting failure was the test contradicting
    /// itself rather than the timer misbehaving.
    stress_one_shot_arm_and_await_fire {
        let threads = 8;
        let rounds = load(40);

        let workers: Vec<_> = (0..threads)
            .map(|_| {
                std::thread::spawn(move || {
                    let tally = Tally::new();
                    let overlap = Overlap::new();
                    let counter = Arc::clone(&tally);
                    let seen = Arc::clone(&overlap);
                    let timer = ThreadpoolTimer::new(
                        move |_firing| {
                            {
                                let _inside = seen.enter();
                                work_for(counter.count());
                            }
                            // Only now is this firing over, so a thread waiting
                            // on the count cannot arm into an overlap.
                            counter.record();
                        },
                        None,
                    )
                    .expect("create timer");

                    for round in 1..=rounds {
                        timer.set_after(Duration::ZERO);
                        let reached = tally.wait_for(round, Duration::from_secs(30));
                        assert_eq!(reached, round, "a timer stopped firing mid-cycle");
                    }

                    timer.disarm();
                    timer.cancel_pending();
                    assert_eq!(
                        overlap.violations(),
                        0,
                        "a timer overlapped itself across arm-fire cycles"
                    );
                    tally.count()
                })
            })
            .collect();

        let total: usize = workers
            .into_iter()
            .map(|worker| worker.join().expect("cycle thread"))
            .sum();

        assert_eq!(total, threads * rounds, "not every arm-fire cycle completed");
        eprintln!("stress:   completed {total} arm-fire cycles across {threads} threads");
    }
}

// --- one-shot: many objects at once ---

stress! {
    /// A large population of independent one-shots armed together, which puts
    /// the pool's own timer queue under pressure rather than any one object.
    stress_one_shot_many_timers_fire {
        let count = load(2_000);
        let tally = Tally::new();

        let timers: Vec<_> = (0..count)
            .map(|i| {
                let counter = Arc::clone(&tally);
                let timer = ThreadpoolTimer::new(
                    move |_firing| {
                        counter.record();
                    },
                    None,
                )
                .expect("create timer");
                // Spread the due times so they do not all land in one burst.
                timer.set_after(Duration::from_micros(u64::try_from(i % 500).unwrap_or(0) * 20));
                timer
            })
            .collect();

        let fired = tally.wait_for(count, Duration::from_secs(120));
        for timer in &timers {
            timer.disarm();
            timer.cancel_pending();
        }

        assert_eq!(fired, count, "not every timer fired");
        assert_quiescent(&tally, "many timers");
    }
}

stress! {
    /// The coalescing path under load. A window lets the pool move a due time
    /// later to batch timers together, so the only invariant is that every timer
    /// still fires -- not when.
    stress_one_shot_coalescing_windows {
        let count = load(1_000);
        let tally = Tally::new();

        let timers: Vec<_> = (0..count)
            .map(|i| {
                let counter = Arc::clone(&tally);
                let timer = ThreadpoolTimer::new(
                    move |_firing| {
                        counter.record();
                    },
                    None,
                )
                .expect("create timer");
                timer.set_after_with_window(
                    Duration::from_millis(u64::try_from(i % 10).unwrap_or(0)),
                    Duration::from_millis(50),
                );
                timer
            })
            .collect();

        let fired = tally.wait_for(count, Duration::from_secs(120));
        for timer in &timers {
            timer.disarm();
            timer.cancel_pending();
        }

        assert_eq!(fired, count, "a coalesced timer never fired");
        assert_quiescent(&tally, "coalescing windows");
    }
}

// --- one-shot: teardown ---

stress! {
    /// Create, arm, and drop as fast as the loop can go.
    ///
    /// The drop always beats the tick here, so no callback runs -- measured, not
    /// assumed. What this exercises is object churn and the teardown of a timer
    /// that is armed but has never fired, which is the common shape when a
    /// component is torn down while idle.
    ///
    /// A regression in the teardown path shows up as a hang or a crash rather
    /// than a failed assertion, which is why the scenario is worth having: no
    /// assertion can observe a use-after-free directly.
    stress_one_shot_rapid_create_arm_drop {
        let cycles = load(5_000);
        let tally = Tally::new();

        for i in 0..cycles {
            let counter = Arc::clone(&tally);
            let timer = ThreadpoolTimer::new(
                move |_firing| {
                    counter.record();
                },
                None,
            )
            .expect("create timer");

            match i % 3 {
                0 => timer.set_after(Duration::ZERO),
                1 => timer.set_after(Duration::from_millis(16)),
                _ => timer.set_after(Duration::from_millis(50)),
            }
            drop(timer);
        }

        eprintln!(
            "stress:   {cycles} create/arm/drop cycles, {} callbacks ran",
            tally.count()
        );
    }
}

stress! {
    /// Drop deliberately straddling the due time, so some drops land before the
    /// firing, some around it, and some after it.
    ///
    /// The rapid loop above cannot do this: it drops microseconds after arming,
    /// so the disarm always wins and no firing is ever raced. Here the timer is
    /// armed a tick out and the drop is delayed by a walking offset that spans
    /// the tick, which is what puts drops on both sides of the firing.
    stress_one_shot_drop_racing_the_due_time {
        let cycles = load(200);
        let tally = Tally::new();

        for i in 0..cycles {
            let counter = Arc::clone(&tally);
            let timer = ThreadpoolTimer::new(
                move |_firing| {
                    counter.record();
                },
                None,
            )
            .expect("create timer");

            timer.set_after(Duration::from_millis(16));
            // Walks 0..28ms, spanning the ~15.6ms tick in both directions.
            let offset = u64::try_from(i % 8).unwrap_or(0) * 4;
            std::thread::sleep(Duration::from_millis(offset));
            drop(timer);
        }

        let fired = tally.count();
        eprintln!("stress:   {cycles} drops straddling the due time, {fired} callbacks ran");
        // Offsets past the tick guarantee some drops land after the firing; if
        // none did, the scenario has degenerated into the rapid loop above.
        assert!(
            fired > 0,
            "no drop landed after a firing -- the offsets no longer span the tick"
        );
    }
}

stress! {
    /// Drop while a callback is *inside* the timer with a deferred re-arm
    /// pending, which is the exact window the teardown gate exists to close.
    ///
    /// The callback signals that it has been entered and then sleeps, so the
    /// drop is guaranteed to land mid-callback rather than merely near one. Each
    /// cycle therefore exercises the gate rather than hoping to.
    stress_one_shot_drop_during_rearming_callback {
        let cycles = load(300);
        let mut entered_total = 0usize;

        for _ in 0..cycles {
            let tally = Tally::new();
            let counter = Arc::clone(&tally);
            let timer = ThreadpoolTimer::new(
                move |firing| {
                    counter.record();
                    // Ask to re-arm, then stay inside long enough for the drop
                    // to reach its disarm before the request is applied.
                    firing.rearm_after(Duration::ZERO);
                    std::thread::sleep(Duration::from_millis(5));
                },
                None,
            )
            .expect("create timer");

            timer.set_after(Duration::ZERO);
            let entered = tally.wait_for(1, Duration::from_secs(30));
            assert_eq!(entered, 1, "the callback never ran");
            entered_total += entered;

            // Drop here, with the callback still inside and a re-arm pending.
            drop(timer);

            // The re-arm must not have survived the teardown. Nothing may run
            // after Drop returns, because Drop has already freed the context.
            let after = tally.count();
            std::thread::sleep(Duration::from_millis(25));
            assert_eq!(
                tally.count(),
                after,
                "a callback ran after the timer was dropped"
            );
        }

        eprintln!("stress:   {cycles} drops landed mid-callback ({entered_total} callbacks entered)");
    }
}

stress! {
    /// Drops racing from many threads at once, each thread churning its own
    /// timers, so teardown runs concurrently with other teardowns and with the
    /// pool dispatching for unrelated objects.
    stress_one_shot_concurrent_teardown {
        let threads = 8;
        let per_thread = load(300);
        let tally = Tally::new();

        let workers: Vec<_> = (0..threads)
            .map(|t| {
                let tally = Arc::clone(&tally);
                std::thread::spawn(move || {
                    for i in 0..per_thread {
                        let counter = Arc::clone(&tally);
                        let timer = ThreadpoolTimer::new(
                            move |_firing| {
                                counter.record();
                            },
                            None,
                        )
                        .expect("create timer");

                        timer.set_after(Duration::ZERO);
                        // A third of the cycles drain explicitly first, so the
                        // drained and undrained teardown paths both run, and a
                        // third outlive a tick so teardown races a real firing
                        // rather than only an armed-but-never-fired timer.
                        match (t + i) % 3 {
                            0 => timer.cancel_pending(),
                            1 => {
                                std::thread::sleep(Duration::from_millis(20));
                                timer.disarm();
                                timer.wait();
                            }
                            _ => {}
                        }
                        drop(timer);
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().expect("teardown thread");
        }

        let fired = tally.count();
        eprintln!(
            "stress:   {} concurrent teardowns, {fired} callbacks ran",
            threads * per_thread
        );
        assert!(
            fired > 0,
            "no teardown raced a firing -- every cycle outran the pool"
        );
    }
}

// --- periodic: ticking under load ---

stress! {
    /// Sustained ticking: a period far shorter than the system tick, run until a
    /// tick budget is met.
    ///
    /// A 1ms period does not produce 1000 ticks a second -- the measured mean is
    /// ~15.6ms, the tick. That is the pool's resolution rather than a defect, and
    /// it is why the rate is reported rather than asserted. What is asserted is
    /// that ticking happens at all and that `stop_and_drain` leaves the timer
    /// quiescent.
    stress_periodic_sustained_ticking {
        let target = load(500);
        let tally = Tally::new();

        let counter = Arc::clone(&tally);
        let timer = ThreadpoolPeriodicTimer::new(
            Duration::from_millis(1),
            move |_tick| {
                counter.record();
            },
            None,
        )
        .expect("create periodic timer");

        let started = Instant::now();
        timer.start_after(Duration::ZERO);
        let reached = tally.wait_for(target, Duration::from_secs(300));
        let elapsed = started.elapsed();
        timer.stop_and_drain();

        assert_eq!(reached, target, "the timer stopped ticking");
        assert!(!timer.is_running(), "the timer still reports running");
        assert_quiescent(&tally, "high-frequency ticks");
        eprintln!(
            "stress:   {reached} ticks in {elapsed:?} ({:?} mean per tick)",
            elapsed / u32::try_from(reached.max(1)).unwrap_or(1)
        );
    }
}

stress! {
    /// A callback slower than the period, which is how overlapping ticks are
    /// produced on purpose.
    ///
    /// The periodic type documents that ticks may run concurrently, so overlap
    /// here is correct behaviour rather than a defect -- the opposite of the
    /// one-shot's guarantee. The peak concurrency reached is reported; only the
    /// drain is asserted, because how much overlap occurs depends on how many
    /// pool threads the machine has.
    stress_periodic_overlapping_ticks_are_tolerated {
        let target = load(200);
        let tally = Tally::new();
        let overlap = Overlap::new();

        let counter = Arc::clone(&tally);
        let seen = Arc::clone(&overlap);
        let timer = ThreadpoolPeriodicTimer::new(
            Duration::from_millis(1),
            move |_tick| {
                let _inside = seen.enter();
                counter.record();
                // Far longer than the period, so the next tick is due while
                // this one is still running.
                std::thread::sleep(Duration::from_millis(20));
            },
            None,
        )
        .expect("create periodic timer");

        timer.start_after(Duration::ZERO);
        let reached = tally.wait_for(target, Duration::from_secs(300));
        timer.stop_and_drain();

        assert_eq!(reached, target, "the timer stopped ticking");
        assert_quiescent(&tally, "overlapping ticks");
        eprintln!(
            "stress:   {reached} ticks, {} overlapping entries, peak {} concurrent",
            overlap.violations(),
            overlap.peak_concurrent()
        );
    }
}

stress! {
    /// A callback that stops its own timer once a threshold is reached.
    ///
    /// `PeriodicTick::stop` stops future ticks being queued; it does not retract
    /// queued ticks and does not affect running ones, so the callback is
    /// expected to run again afterwards. The assertion is therefore that ticking
    /// *ends*, not that it ends immediately.
    stress_periodic_self_stop {
        let rounds = load(200);
        let mut overshoot_total = 0usize;

        for _ in 0..rounds {
            let threshold = 5;
            let tally = Tally::new();
            let counter = Arc::clone(&tally);
            let timer = ThreadpoolPeriodicTimer::new(
                Duration::from_millis(1),
                move |tick| {
                    if counter.record() >= threshold {
                        tick.stop();
                    }
                },
                None,
            )
            .expect("create periodic timer");

            timer.start_after(Duration::ZERO);
            let reached = tally.wait_for(threshold, Duration::from_secs(30));
            assert!(
                reached >= threshold,
                "the timer stopped before reaching its threshold"
            );

            timer.stop_and_drain();
            let settled = tally.count();
            overshoot_total += settled.saturating_sub(threshold);

            std::thread::sleep(Duration::from_millis(5));
            assert_eq!(
                tally.count(),
                settled,
                "the timer kept ticking after stopping itself and draining"
            );
        }

        eprintln!(
            "stress:   {rounds} self-stopping timers, {overshoot_total} ticks past the threshold"
        );
    }
}

stress! {
    /// Many threads starting, stopping, and querying one running timer.
    ///
    /// `start` and `stop` are both `&self`, so this is a supported usage; the
    /// invariant is that whichever call lands last decides the state, and that a
    /// drain afterwards is final.
    stress_periodic_start_stop_churn {
        let threads = 8;
        let per_thread = load(200);
        let tally = Tally::new();

        let counter = Arc::clone(&tally);
        let timer = Arc::new(
            ThreadpoolPeriodicTimer::new(
                Duration::from_millis(2),
                move |_tick| {
                    counter.record();
                },
                None,
            )
            .expect("create periodic timer"),
        );

        let workers: Vec<_> = (0..threads)
            .map(|t| {
                let timer = Arc::clone(&timer);
                std::thread::spawn(move || {
                    for i in 0..per_thread {
                        match (t + i) % 8 {
                            0..=2 => timer.start_after(Duration::ZERO),
                            3 => timer.start(),
                            4 => timer.stop(),
                            _ => {
                                let _ = timer.is_running();
                            }
                        }
                        // Longer than a timer tick. Every `start_after` resets
                        // the due time, so without a pause that outlasts a tick
                        // the threads keep pushing the next tick into the future
                        // and the timer never actually fires.
                        if i % 4 == 0 {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().expect("churn thread");
        }

        timer.stop_and_drain();

        let fired = tally.count();
        eprintln!("stress:   {fired} ticks under start/stop churn");
        assert!(
            fired > 0,
            "the timer never ticked under churn -- the loop is outrunning the pool"
        );
        assert!(!timer.is_running(), "the timer still reports running");
        assert_quiescent(&tally, "start/stop churn");
    }
}

// --- periodic: teardown ---

stress! {
    /// Create, start, and drop while ticking, which is the periodic equivalent
    /// of the one-shot teardown race.
    ///
    /// The timer is left running long enough to tick before the drop, so `Drop`
    /// runs against a live tick stream rather than a timer that never started.
    stress_periodic_drop_while_ticking {
        let cycles = load(300);
        let tally = Tally::new();

        for i in 0..cycles {
            let counter = Arc::clone(&tally);
            let timer = ThreadpoolPeriodicTimer::new(
                Duration::from_millis(1),
                move |_tick| {
                    counter.record();
                },
                None,
            )
            .expect("create periodic timer");

            timer.start_after(Duration::ZERO);
            // Walks 0..14ms so drops land before, around, and after ticks.
            let offset = u64::try_from(i % 8).unwrap_or(0) * 2;
            std::thread::sleep(Duration::from_millis(offset));
            drop(timer);
        }

        let fired = tally.count();
        eprintln!("stress:   {cycles} periodic timers dropped while ticking, {fired} ticks ran");
        assert!(
            fired > 0,
            "no drop raced a tick -- the offsets no longer span the tick"
        );
    }
}

stress! {
    /// Drop landing while a tick is inside the callback.
    ///
    /// The callback signals entry and then stays inside, so the drop is
    /// guaranteed to land mid-tick. Nothing may run after `Drop` returns,
    /// because it has already freed the context by then.
    stress_periodic_drop_during_tick {
        let cycles = load(200);

        for _ in 0..cycles {
            let tally = Tally::new();
            let counter = Arc::clone(&tally);
            let timer = ThreadpoolPeriodicTimer::new(
                Duration::from_millis(1),
                move |_tick| {
                    counter.record();
                    std::thread::sleep(Duration::from_millis(5));
                },
                None,
            )
            .expect("create periodic timer");

            timer.start_after(Duration::ZERO);
            let entered = tally.wait_for(1, Duration::from_secs(30));
            assert!(entered >= 1, "the tick never ran");

            // Drop here, with a tick still inside the callback.
            drop(timer);

            let after = tally.count();
            std::thread::sleep(Duration::from_millis(15));
            assert_eq!(
                tally.count(),
                after,
                "a tick ran after the timer was dropped"
            );
        }

        eprintln!("stress:   {cycles} drops landed mid-tick");
    }
}

stress! {
    /// A large population of periodic timers ticking at once, which puts the
    /// pool's timer queue under sustained pressure rather than any one object.
    stress_periodic_many_timers_tick {
        let count = load(300);
        let per_timer = 3;
        let tally = Tally::new();

        let timers: Vec<_> = (0..count)
            .map(|i| {
                let counter = Arc::clone(&tally);
                let timer = ThreadpoolPeriodicTimer::new(
                    Duration::from_millis(2),
                    move |_tick| {
                        counter.record();
                    },
                    None,
                )
                .expect("create periodic timer");
                // Stagger the first ticks so they do not all land together.
                timer.start_after(Duration::from_millis(u64::try_from(i % 10).unwrap_or(0)));
                timer
            })
            .collect();

        let reached = tally.wait_for(count * per_timer, Duration::from_secs(300));
        for timer in &timers {
            timer.stop_and_drain();
        }

        assert_eq!(
            reached,
            count * per_timer,
            "the timer population stopped ticking"
        );
        for timer in &timers {
            assert!(!timer.is_running(), "a timer still reports running");
        }
        assert_quiescent(&tally, "many periodic timers");
        eprintln!("stress:   {reached} ticks across {count} periodic timers");
    }
}

stress! {
    /// Periods below the minimum are rejected, and rejection under load must not
    /// leave anything behind: the object is never created, so there is nothing
    /// to drop, and a valid timer must still be creatable afterwards.
    stress_periodic_short_period_is_rejected {
        let attempts = load(2_000);
        let too_short = [
            Duration::ZERO,
            Duration::from_micros(1),
            Duration::from_micros(500),
            Duration::from_micros(999),
        ];

        for i in 0..attempts {
            let period = too_short[i % too_short.len()];
            let rejected = ThreadpoolPeriodicTimer::new(period, |_tick| {}, None);
            assert!(rejected.is_err(), "a period of {period:?} was accepted");
        }

        let tally = Tally::new();
        let counter = Arc::clone(&tally);
        let timer = ThreadpoolPeriodicTimer::new(
            Duration::from_millis(1),
            move |_tick| {
                counter.record();
            },
            None,
        )
        .expect("create periodic timer after rejections");
        timer.start_after(Duration::ZERO);
        assert!(
            tally.wait_for(1, Duration::from_secs(30)) >= 1,
            "a valid timer would not tick after repeated rejections"
        );
        timer.stop_and_drain();

        eprintln!("stress:   {attempts} short-period rejections");
    }
}

// --- cleanup groups holding timer members ---

stress! {
    /// A group holding a large population of armed one-shot and ticking
    /// periodic members, released as a unit.
    ///
    /// A group releases its members' contexts itself, so this is a different
    /// teardown path from dropping the timers individually: one release drains
    /// every member at once. Both dispositions are exercised, because cancelling
    /// pending callbacks and running them first are different paths through it.
    stress_cleanup_group_timer_members {
        let rounds = load(60);
        let per_round = 40;
        let tally = Tally::new();
        let mut released = 0usize;

        for round in 0..rounds {
            let mut group = CleanupGroup::new().expect("create cleanup group");

            {
                let one_shots: Vec<_> = (0..per_round)
                    .map(|_| {
                        let counter = Arc::clone(&tally);
                        group
                            .create_timer(move |_firing| {
                                counter.record();
                            }, None)
                            .expect("create timer member")
                    })
                    .collect();
                let periodics: Vec<_> = (0..per_round)
                    .map(|_| {
                        let counter = Arc::clone(&tally);
                        group
                            .create_periodic_timer(
                                Duration::from_millis(1),
                                move |_tick| {
                                    counter.record();
                                },
                                None,
                            )
                            .expect("create periodic member")
                    })
                    .collect();

                for member in &one_shots {
                    member.set_after(Duration::ZERO);
                }
                for member in &periodics {
                    member.start_after(Duration::ZERO);
                }

                assert_eq!(
                    group.owned_resources(),
                    per_round * 2,
                    "the group is not holding every member's context"
                );

                // Let some of the rounds tick before release, so the group is
                // torn down both while idle and while its members are firing.
                if round % 2 == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }

            // Alternate cancelling pending callbacks and letting them run.
            group.close_members(round % 2 == 0);
            assert_eq!(
                group.owned_resources(),
                0,
                "the group still holds member resources after release"
            );
            released += per_round * 2;

            // Nothing may run once the group has released its members: it has
            // freed their contexts by then.
            let settled = tally.count();
            std::thread::sleep(Duration::from_millis(10));
            assert_eq!(
                tally.count(),
                settled,
                "a member callback ran after the group released it"
            );
        }

        eprintln!(
            "stress:   {released} timer members across {rounds} groups, {} callbacks ran",
            tally.count()
        );
    }
}

stress! {
    /// Groups created, populated, and released concurrently from many threads,
    /// so one group's release runs against other groups' live callbacks.
    stress_cleanup_group_concurrent_release {
        let threads = 8;
        let per_thread = load(40);
        let tally = Tally::new();

        let workers: Vec<_> = (0..threads)
            .map(|t| {
                let tally = Arc::clone(&tally);
                std::thread::spawn(move || {
                    for i in 0..per_thread {
                        let mut group = CleanupGroup::new().expect("create cleanup group");
                        {
                            let counter = Arc::clone(&tally);
                            let one_shot = group
                                .create_timer(move |_firing| {
                                    counter.record();
                                }, None)
                                .expect("create timer member");
                            let counter = Arc::clone(&tally);
                            let periodic = group
                                .create_periodic_timer(
                                    Duration::from_millis(1),
                                    move |_tick| {
                                        counter.record();
                                    },
                                    None,
                                )
                                .expect("create periodic member");

                            one_shot.set_after(Duration::ZERO);
                            periodic.start_after(Duration::ZERO);

                            // A third of the groups outlive a tick, so their
                            // release races callbacks that are actually running.
                            if (t + i) % 3 == 0 {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                        }
                        group.close_members((t + i) % 2 == 0);
                        assert_eq!(
                            group.owned_resources(),
                            0,
                            "a concurrently released group still holds resources"
                        );
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().expect("group thread");
        }

        let fired = tally.count();
        eprintln!(
            "stress:   {} groups released concurrently, {fired} callbacks ran",
            threads * per_thread
        );
        assert!(
            fired > 0,
            "no group release raced a callback -- every round outran the pool"
        );
    }
}

stress! {
    /// A group left to drop rather than closed explicitly, which must release
    /// its members just the same.
    stress_cleanup_group_drop_without_close {
        let rounds = load(200);
        let tally = Tally::new();

        for round in 0..rounds {
            let group = CleanupGroup::new().expect("create cleanup group");
            {
                let counter = Arc::clone(&tally);
                let one_shot = group
                    .create_timer(move |_firing| {
                        counter.record();
                    }, None)
                    .expect("create timer member");
                let counter = Arc::clone(&tally);
                let periodic = group
                    .create_periodic_timer(
                        Duration::from_millis(1),
                        move |_tick| {
                            counter.record();
                        },
                        None,
                    )
                    .expect("create periodic member");

                one_shot.set_after(Duration::ZERO);
                periodic.start_after(Duration::ZERO);

                if round % 3 == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            // Dropped here without close_members, with members possibly firing.
            drop(group);
        }

        let fired = tally.count();
        eprintln!("stress:   {rounds} groups dropped without closing, {fired} callbacks ran");
        assert!(
            fired > 0,
            "no group drop raced a callback -- every round outran the pool"
        );
    }
}

// --- everything at once ---

stress! {
    /// One-shots, periodics, cleanup groups, and object churn all running
    /// together, which is the closest this suite gets to how the crate would be
    /// driven under real load.
    ///
    /// Each participant asserts its own invariant, so a failure names the shape
    /// of work that broke rather than merely reporting that the mix did.
    stress_mixed_timer_load {
        let duration = Duration::from_secs(u64::try_from(load(3)).unwrap_or(3).min(60));
        let deadline = Instant::now() + duration;

        let chain_tally = Tally::new();
        let chain_overlap = Overlap::new();
        let tick_tally = Tally::new();
        let churn_tally = Tally::new();

        // A self-re-arming one-shot, which must still never overlap itself
        // however loaded the pool is around it.
        let counter = Arc::clone(&chain_tally);
        let seen = Arc::clone(&chain_overlap);
        let chain = ThreadpoolTimer::new(
            move |firing| {
                let _inside = seen.enter();
                let n = counter.record();
                work_for(n);
                firing.rearm_after(Duration::ZERO);
            },
            None,
        )
        .expect("create chain timer");
        chain.set_after(Duration::ZERO);

        // A population of periodics ticking throughout.
        let periodics: Vec<_> = (0..16)
            .map(|i| {
                let counter = Arc::clone(&tick_tally);
                let timer = ThreadpoolPeriodicTimer::new(
                    Duration::from_millis(2),
                    move |_tick| {
                        counter.record();
                    },
                    None,
                )
                .expect("create periodic timer");
                timer.start_after(Duration::from_millis(i));
                timer
            })
            .collect();

        // Threads churning short-lived timers and groups underneath all of it.
        let workers: Vec<_> = (0..4)
            .map(|t| {
                let churn_tally = Arc::clone(&churn_tally);
                std::thread::spawn(move || {
                    let mut cycles = 0usize;
                    while Instant::now() < deadline {
                        let counter = Arc::clone(&churn_tally);
                        if t % 2 == 0 {
                            let timer = ThreadpoolTimer::new(
                                move |_firing| {
                                    counter.record();
                                },
                                None,
                            )
                            .expect("create churn timer");
                            timer.set_after(Duration::ZERO);
                            std::thread::sleep(Duration::from_millis(20));
                            drop(timer);
                        } else {
                            let mut group = CleanupGroup::new().expect("create churn group");
                            {
                                let member = group
                                    .create_timer(move |_firing| {
                                        counter.record();
                                    }, None)
                                    .expect("create churn member");
                                member.set_after(Duration::ZERO);
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            group.close_members(false);
                        }
                        cycles += 1;
                    }
                    cycles
                })
            })
            .collect();

        let churn_cycles: usize = workers
            .into_iter()
            .map(|worker| worker.join().expect("churn thread"))
            .sum();

        chain.disarm();
        chain.cancel_pending();
        for timer in &periodics {
            timer.stop_and_drain();
        }

        assert_eq!(
            chain_overlap.violations(),
            0,
            "the self-re-arming one-shot overlapped itself under mixed load"
        );
        assert!(
            chain_tally.count() > 0,
            "the self-re-arming chain never advanced under mixed load"
        );
        assert!(
            tick_tally.count() > 0,
            "the periodic population never ticked under mixed load"
        );
        assert!(
            churn_tally.count() > 0,
            "no churned timer ever fired under mixed load"
        );
        assert_quiescent(&chain_tally, "mixed load chain");
        assert_quiescent(&tick_tally, "mixed load periodics");

        eprintln!(
            "stress:   {duration:?} of mixed load: {} chain links, {} ticks, {churn_cycles} churn cycles, {} churn firings",
            chain_tally.count(),
            tick_tally.count(),
            churn_tally.count()
        );
    }
}
