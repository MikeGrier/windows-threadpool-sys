// Copyright (c) 2026 Mike Grier
//! The `fail-fast` feature turns a `Drop` that owes a drain into a panic.
//!
//! Three separate claims, which need three different kinds of assertion:
//!
//! 1. **It fires when a drain is owed.** The panic happens on the dropping
//!    thread, so this is an ordinary unwind and `catch_unwind` can observe it
//!    in-process.
//! 2. **It does not fire when nothing is owed.** The other half of the guard:
//!    a fail-fast that panicked unconditionally would satisfy (1) and be
//!    useless, so the accepting direction is asserted too.
//! 3. **The drain happens before the panic.** The ordering constraint the
//!    feature is built around -- panicking first would unwind past the close
//!    and the context free, which is the abandonment the feature exists to
//!    prevent, reached through the mechanism meant to prevent it. Asserted by
//!    observing that a callback queued at drop time *ran*, which it cannot
//!    have done if the panic came first.
//!
//! A fourth claim -- a fail-fast landing on an already-unwinding path aborts --
//! cannot be asserted in-process, because the abort would take the test runner
//! with it. That one re-executes this binary as a child, the same way
//! `callback_panic_aborts.rs` guards the callback contract.

#![cfg(windows)]
#![cfg(feature = "fail-fast")]

use std::os::windows::io::AsRawHandle;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::pool::ThreadpoolPool;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use windows_threadpool_sys::work::ThreadpoolWork;

use windows_sys::Win32::System::Threading::SetEvent;

/// The variable naming which child scenario to run. Absent in the parent.
const SCENARIO_VAR: &str = "WTPS_FAIL_FAST_SCENARIO";

/// How long a child gets to reach its abort before the parent gives up on it.
const CHILD_TIMEOUT: Duration = Duration::from_secs(60);

/// Exit code a child reports when it fails during *setup*, before it could
/// reach the double-panic path. Any nonzero exit otherwise looks like a pass,
/// so a setup failure must be distinguishable from a real abort.
const SETUP_FAILURE_EXIT_CODE: i32 = 111;

fn event() -> WaitableHandle {
    WaitableHandle::event(true, false).expect("create an event")
}

fn signal(wait: &ThreadpoolWait) {
    // SAFETY: the handle is a live event owned by `wait` for the call's duration.
    let ok = unsafe { SetEvent(wait.handle().as_raw_handle() as _) };
    assert!(ok != 0, "signal the event");
}

/// The panic message, when `f` panicked with a string payload.
fn panic_message<F: FnOnce()>(f: F) -> Option<String> {
    // The default hook would print the expected panic to stderr and make a
    // passing run look like a failing one.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(previous);

    let payload = caught.err()?;
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return Some((*s).to_owned());
    }
    payload.downcast_ref::<String>().cloned()
}

// --- 1. it fires when a drain is owed ---

#[test]
fn dropping_an_armed_wait_panics() {
    let message = panic_message(|| {
        let wait = ThreadpoolWait::new(event(), |_| {}, None).expect("create wait");
        wait.arm(None);
        // Dropped here, with the drain never made.
    })
    .expect("the drop must panic when a drain is owed");

    assert!(
        message.contains("ThreadpoolWait") && message.contains("drain still owed"),
        "the panic must name the type and the obligation, got: {message}"
    );
    assert!(
        message.contains("stop_and_drain"),
        "the panic must name the call that discharges it, got: {message}"
    );
}

#[test]
fn dropping_a_submitted_work_item_panics() {
    let message = panic_message(|| {
        let work = ThreadpoolWork::new(|| {}, None).expect("create work");
        work.submit();
    })
    .expect("the drop must panic when a drain is owed");

    assert!(
        message.contains("ThreadpoolWork"),
        "the panic must name the type, got: {message}"
    );
}

// --- 2. it does not fire when nothing is owed ---

#[test]
fn dropping_a_drained_wait_does_not_panic() {
    // The accepting direction. Without this, a fail-fast that panicked on every
    // drop would pass every assertion above while being useless.
    let outcome = panic_message(|| {
        let wait = ThreadpoolWait::new(event(), |_| {}, None).expect("create wait");
        wait.arm(None);
        wait.stop_and_drain();
    });
    assert_eq!(
        outcome, None,
        "a wait the caller drained owes nothing, so its drop must be silent"
    );
}

#[test]
fn dropping_a_never_armed_wait_does_not_panic() {
    let outcome = panic_message(|| {
        let _wait = ThreadpoolWait::new(event(), |_| {}, None).expect("create wait");
    });
    assert_eq!(
        outcome, None,
        "a wait that was never armed never owed a drain"
    );
}

// --- 3. the drain happens before the panic ---

#[test]
fn the_drain_runs_before_the_panic() {
    // The ordering constraint, asserted through the only thing that can
    // distinguish the two orders from outside: whether the queued callback ran.
    //
    // A panic placed *before* the drain unwinds past it, so the callback is
    // still queued when the object dies and this count stays at zero. That is
    // precisely the abandonment the feature exists to prevent, so a fail-fast
    // that caused it would be worse than none at all.
    let pool = ThreadpoolPool::new().expect("create the private pool");
    pool.set_min_threads(1).expect("one thread minimum");
    pool.set_max_threads(1).expect("one thread maximum");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    // Occupy the pool's only thread, so the wait's callback is *queued* rather
    // than already run by the time the drop happens.
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let gate_for_work = Arc::clone(&gate);
    let entered = Arc::new(AtomicUsize::new(0));
    let entered_for_work = Arc::clone(&entered);
    let occupier = ThreadpoolWork::new(
        move || {
            entered_for_work.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*gate_for_work;
            let mut open = lock.lock().unwrap_or_else(|p| p.into_inner());
            while !*open {
                open = cvar.wait(open).unwrap_or_else(|p| p.into_inner());
            }
        },
        Some(&mut env),
    )
    .expect("create the occupying work item");
    occupier.submit();

    // `submit` queues; it does not dispatch. Waiting for the occupier to be
    // *running* is what makes "the callback is still queued" a fact rather than
    // an assumption -- see `M-T6.10`, where skipping this made a guard pass
    // under the mutation it existed to catch.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while entered.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the occupier never reached the pool's only thread"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    let ran = Arc::new(AtomicUsize::new(0));
    let ran_for_callback = Arc::clone(&ran);

    // Release the occupier from another thread, so the drain inside the drop is
    // what waits for the queued callback rather than this thread having already
    // let it run.
    let gate_for_release = Arc::clone(&gate);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let (lock, cvar) = &*gate_for_release;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
        cvar.notify_all();
    });

    let message = panic_message(|| {
        let wait = ThreadpoolWait::new(
            event(),
            move |_| {
                ran_for_callback.fetch_add(1, Ordering::SeqCst);
            },
            Some(&mut env),
        )
        .expect("create wait");
        wait.arm(None);
        signal(&wait);
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "the pool's only thread is occupied, so the callback cannot have run yet"
        );
        // Dropped here: the drain must run the queued callback, and only then
        // may the fail-fast panic.
    })
    .expect("the drop must still panic -- the drain does not discharge the obligation");

    // Snapshotted *here*, before the releaser is joined, and that timing is the
    // whole assertion. "Did the callback run at all" cannot distinguish the two
    // orders: a panic placed first unwinds past `CloseThreadpoolWait`, so the
    // pool goes on watching a leaked context and the callback still runs, just
    // later and unsupervised. Measured -- an earlier version of this test
    // asserted the count after joining and passed under exactly that mutation.
    //
    // What only the correct order produces is a *blocking* drop: the drain
    // waits for the queued callback, so by the time the panic reaches this
    // thread the count is already 1. The mutated order returns immediately,
    // while the occupier still holds the pool's only thread.
    let ran_when_the_panic_surfaced = ran.load(Ordering::SeqCst);

    // Asserted before the join, not after, and that ordering is also measured.
    // Under the panic-first mutation the wait is never disarmed or closed, so
    // the pool goes on watching a handle that the unwinding field drop has
    // already closed; when the occupier releases, the dispatch into it takes
    // the process down. Joining first would spend the whole 50ms release window
    // inside that race and the crash would pre-empt this assertion -- turning a
    // precise "the drain did not run" into a bare abort, which says only that
    // something went wrong.
    //
    // A *failing* run still ends in an abort after this message is printed,
    // because unwinding out of here drops `occupier` owing a drain and the
    // fail-fast panics again. That is this feature's own double-fault contract
    // operating on an unwinding path, not a defect in the test: the assertion
    // text reaches stderr first, which is what diagnosis needs.
    assert_eq!(
        ran_when_the_panic_surfaced, 1,
        "the drain must have run the queued callback *before* the panic: a panic placed first \
         unwinds past the drain and the close, abandoning the callback to a leaked context, \
         which is the failure this feature exists to prevent"
    );

    releaser.join().expect("the releasing thread finished");

    assert!(
        message.contains("ThreadpoolWait"),
        "the panic must name the type, got: {message}"
    );

    occupier.stop_and_drain();
}

// --- 4. a fail-fast on an unwinding path aborts ---

/// Run `scenario` in a child copy of this binary and assert it aborted.
///
/// "Aborted" is asserted as *did not exit successfully* rather than as a
/// specific status: the code Windows reports for a Rust abort is a toolchain
/// detail that has changed between releases, so pinning it would make this fail
/// on a change that is not a regression here.
fn assert_child_aborts(scenario: &str) {
    let exe = std::env::current_exe().expect("locate the test binary");
    let mut child = Command::new(exe)
        .env(SCENARIO_VAR, scenario)
        .env("RUST_TEST_THREADS", "1")
        // The child's panic messages are expected output, not a failure.
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn the child");

    let deadline = std::time::Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the child") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            // Kill and reap, or it survives as an orphan that can hang the run.
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the {scenario} child neither aborted nor exited within {CHILD_TIMEOUT:?}; \
                 the second panic was probably contained"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        !status.success(),
        "the {scenario} child exited cleanly, so the double panic was contained"
    );
    assert_ne!(
        status.code(),
        Some(SETUP_FAILURE_EXIT_CODE),
        "the {scenario} child failed during setup, before it could reach the double panic -- \
         this proves nothing about what a fail-fast does on an unwinding path"
    );
}

/// Panic with an armed wait still live, so the fail-fast lands mid-unwind.
fn child_fail_fast_during_unwind() -> ! {
    let wait = ThreadpoolWait::new(event(), |_| {}, None).expect("create wait");
    wait.arm(None);
    // This panic begins unwinding; `wait` is then dropped as a local of the
    // unwinding frame, and its fail-fast panics again. A second panic while
    // panicking is a double fault, which Rust resolves by aborting.
    panic!("the first panic, which the fail-fast then compounds");
}

#[test]
fn a_fail_fast_on_an_unwinding_path_aborts_the_process() {
    dispatch_if_child();
    assert_child_aborts("unwind");
}

/// Dispatch to a child scenario when this binary was spawned as one.
///
/// libtest runs every `#[test]`, so the dispatch happens from inside one rather
/// than from a `main`. In a child it never returns.
///
/// The `catch_unwind` exists only for a setup failure on *this* thread, which
/// must report [`SETUP_FAILURE_EXIT_CODE`] rather than being indistinguishable
/// from a real abort. It cannot catch the double panic: that aborts the process
/// outright rather than unwinding to here.
fn dispatch_if_child() {
    let Ok(scenario) = std::env::var(SCENARIO_VAR) else {
        return;
    };
    let caught = catch_unwind(|| match scenario.as_str() {
        "unwind" => child_fail_fast_during_unwind(),
        other => panic!("unknown child scenario {other}"),
    });
    if caught.is_err() {
        std::process::exit(SETUP_FAILURE_EXIT_CODE);
    }
}
