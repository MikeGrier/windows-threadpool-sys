// Copyright (c) 2026 Mike Grier
//! Unit tests for the servicing path.
//!
//! What is under test is the machinery's four promises: every request is serviced
//! exactly once, in submission order, never concurrently with another, and
//! teardown converges. The request type is deliberately trivial -- these are
//! properties of the path, not of what travels it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::Servicer;

/// Upper bound for waiting on work the pool really should run.
const SERVICE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a deliberately-parked handler will wait to be released before giving
/// up and returning.
///
/// The bound is not politeness. Teardown waits for a running handler, so a
/// handler that parks unconditionally turns any assertion failure *before* the
/// release into a deadlock rather than a test failure -- the panic unwinds into
/// `Drop`, which waits for a handler nothing will ever free. That wedged a whole
/// nextest run rather than reporting one failing assertion.
const PARK_LIMIT: Duration = Duration::from_secs(10);

/// Park until `gate` is set, or until [`PARK_LIMIT`] elapses.
fn park_until(gate: &std::sync::atomic::AtomicBool) {
    let deadline = Instant::now() + PARK_LIMIT;
    while !gate.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Records what the handler saw, so a test can assert on order and count without
/// running assertions on the servicing path itself.
#[derive(Default)]
struct Log {
    seen: std::sync::Mutex<Vec<u64>>,
    /// How many handler calls are in flight. Peaks above one would mean the path
    /// is not serialising.
    inside: AtomicUsize,
    peak: AtomicUsize,
}

impl Log {
    fn record(&self, item: u64) {
        let now = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.seen.lock().expect("record").push(item);
        self.inside.fetch_sub(1, Ordering::SeqCst);
    }

    fn items(&self) -> Vec<u64> {
        self.seen.lock().expect("read").clone()
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

/// A servicer that appends every request to a shared log.
fn logging() -> (Servicer<u64>, Arc<Log>) {
    let log = Arc::new(Log::default());
    let handler = Arc::clone(&log);
    let servicer =
        Servicer::new(move |item: u64| handler.record(item)).expect("create the servicing path");
    (servicer, log)
}

/// Block until `predicate` holds, failing rather than hanging if it never does.
fn wait_until<F>(what: &str, predicate: F)
where
    F: Fn() -> bool,
{
    let deadline = Instant::now() + SERVICE_TIMEOUT;
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_submitted_request_is_serviced() {
    let (servicer, log) = logging();
    servicer.submit(7).expect("accepted");
    wait_until("the request", || log.items() == [7]);
}

#[test]
fn requests_are_serviced_in_submission_order() {
    let (servicer, log) = logging();
    for item in 0..64 {
        servicer.submit(item).expect("accepted");
    }
    wait_until("every request", || log.items().len() == 64);

    let expected: Vec<u64> = (0..64).collect();
    assert_eq!(log.items(), expected, "the queue must be FIFO");
}

#[test]
fn every_request_is_serviced_exactly_once() {
    let (servicer, log) = logging();
    for item in 0..500 {
        servicer.submit(item).expect("accepted");
    }
    wait_until("every request", || log.items().len() == 500);

    let mut seen = log.items();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 500, "no request may be serviced twice or lost");
}

#[test]
fn handlers_never_run_concurrently() {
    let (servicer, log) = logging();
    for item in 0..500 {
        servicer.submit(item).expect("accepted");
    }
    wait_until("every request", || log.items().len() == 500);

    // The guarantee D-2 actually needs. Coalescing alone would not give it: a
    // drain that has emptied the queue but is still running its last handler
    // leaves the queue observably empty, so an empty/non-empty doorbell would
    // start a second drain alongside the first.
    assert_eq!(
        log.peak(),
        1,
        "resident-state mutations must be serialised, saw {} concurrent handlers",
        log.peak()
    );
}

#[test]
fn many_producers_are_all_serviced_and_never_overlap() {
    let (servicer, log) = logging();
    let servicer = Arc::new(servicer);

    let producers: Vec<_> = (0..8)
        .map(|producer| {
            let servicer = Arc::clone(&servicer);
            std::thread::spawn(move || {
                for index in 0..100 {
                    servicer.submit(producer * 100 + index).expect("accepted");
                }
            })
        })
        .collect();
    for producer in producers {
        producer.join().expect("producer");
    }

    wait_until("every request", || log.items().len() == 800);
    assert_eq!(log.peak(), 1, "concurrent producers, one servicer");

    let mut seen = log.items();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 800);
}

#[test]
fn the_doorbell_coalesces_a_burst_of_submissions() {
    // The concrete failure this exists to prevent: subscribing to 500 paths must
    // not queue 500 drains, 499 of which find the queue already emptied.
    //
    // Parking the handler is what makes the count exact rather than a bound. A
    // burst against an idle servicer would ring an unpredictable number of times
    // -- legitimately, since a drain that outpaces the producer finds nothing to
    // coalesce -- so asserting a small number there would be asserting the
    // machine's speed. With a handler held mid-flight the backlog is guaranteed,
    // and the flag is set at *submission* time rather than when the drain runs, so
    // the answer is one regardless of how the pool schedules anything.
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate = Arc::clone(&released);
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);

    let servicer = Servicer::new(move |_: u64| {
        park_until(&gate);
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .expect("create the servicing path");

    for item in 0..500 {
        servicer.submit(item).expect("accepted");
    }
    assert_eq!(
        servicer.rings(),
        1,
        "500 submissions behind one running handler must queue exactly one drain"
    );

    released.store(true, Ordering::SeqCst);
    wait_until("every request", || count.load(Ordering::SeqCst) == 500);

    // Servicing the backlog rings nothing further: the drain loops until empty
    // rather than needing a ring per request.
    assert_eq!(servicer.rings(), 1);
}

#[test]
fn an_idle_servicer_has_not_rung() {
    let (servicer, log) = logging();
    assert_eq!(servicer.rings(), 0);
    assert_eq!(servicer.pending(), 0);
    assert!(log.items().is_empty());
}

#[test]
fn a_request_submitted_from_a_handler_is_serviced() {
    // Re-entrant submission is what M3.5's cancel-during-subscribe path will do,
    // and it must not deadlock: `submit` holds the queue lock only for the
    // enqueue, and the drain holds it only to pop.
    let log = Arc::new(Log::default());
    let handler = Arc::clone(&log);
    let inner: Arc<std::sync::OnceLock<Servicer<u64>>> = Arc::new(std::sync::OnceLock::new());
    // Weak, not a second `Arc`: the strong form would be a cycle -- the servicer
    // owns the work object, which owns this closure -- so the servicer would never
    // drop and its teardown would never be exercised.
    let self_ref = Arc::downgrade(&inner);

    let servicer = Servicer::new(move |item: u64| {
        handler.record(item);
        if item == 1 {
            let inner = self_ref
                .upgrade()
                .expect("the servicer outlives its own handler");
            let servicer = inner
                .get()
                .expect("the servicer is set before any submission");
            servicer.submit(2).expect("accepted");
        }
    })
    .expect("create the servicing path");
    inner
        .set(servicer)
        .unwrap_or_else(|_| unreachable!("set exactly once"));

    inner.get().expect("set").submit(1).expect("accepted");
    wait_until("both requests", || log.items() == [1, 2]);
}

#[test]
fn a_slow_handler_does_not_block_producers() {
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate = Arc::clone(&released);
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);

    let servicer = Servicer::new(move |_: u64| {
        // The first request parks, so every later submission happens while a
        // handler is genuinely mid-flight.
        park_until(&gate);
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .expect("create the servicing path");

    servicer.submit(0).expect("accepted");
    // Wait until that request has actually been taken off the queue, so the count
    // asserted below is exact rather than a race against the drain starting.
    wait_until("the handler to start", || servicer.pending() == 0);

    // If `submit` waited on servicing, this would not return until the handler
    // was released -- so completing it at all is the assertion.
    let started = Instant::now();
    for item in 1..200 {
        servicer.submit(item).expect("accepted");
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "submitting blocked behind a running handler"
    );
    assert_eq!(servicer.pending(), 199);

    released.store(true, Ordering::SeqCst);
    wait_until("every request", || count.load(Ordering::SeqCst) == 200);
}

#[test]
fn shutdown_refuses_further_requests_and_hands_them_back() {
    let (servicer, _log) = logging();
    servicer.shut_down();

    assert!(!servicer.is_open());
    let rejected = servicer
        .submit(99)
        .expect_err("a shut-down path must refuse");
    assert_eq!(
        rejected.0, 99,
        "the request comes back so it can be reported"
    );
}

#[test]
fn shutdown_is_idempotent_and_drop_after_it_is_safe() {
    let (servicer, _log) = logging();
    servicer.submit(1).expect("accepted");
    servicer.shut_down();
    servicer.shut_down();
    servicer.shut_down();
    assert!(!servicer.is_open());
    drop(servicer);
}

#[test]
fn shutdown_discards_pending_requests_rather_than_servicing_them() {
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate = Arc::clone(&released);
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);

    let servicer = Servicer::new(move |_: u64| {
        park_until(&gate);
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .expect("create the servicing path");

    servicer.submit(0).expect("accepted");
    wait_until("the handler to start", || servicer.pending() == 0);
    for item in 1..50 {
        servicer.submit(item).expect("accepted");
    }
    assert_eq!(servicer.pending(), 49);

    // Release before shutting down, or shutdown would wait on a handler that
    // never returns -- which is a property of this test's handler, not of the
    // path.
    released.store(true, Ordering::SeqCst);
    servicer.shut_down();

    assert_eq!(servicer.pending(), 0);
    assert!(
        servicer.discarded() > 0,
        "pending requests must be discarded, not serviced, at teardown"
    );
    assert!(
        count.load(Ordering::SeqCst) < 50,
        "teardown serviced the whole backlog instead of discarding it"
    );
}

#[test]
fn shutdown_waits_for_a_running_handler() {
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&finished);

    let servicer = Servicer::new(move |_: u64| {
        std::thread::sleep(Duration::from_millis(50));
        flag.store(true, Ordering::SeqCst);
    })
    .expect("create the servicing path");

    servicer.submit(0).expect("accepted");
    // Wait until the handler is genuinely running, so the shutdown below has
    // something in flight to wait for rather than racing the submission.
    wait_until("the handler to start", || servicer.pending() == 0);
    servicer.shut_down();

    assert!(
        finished.load(Ordering::SeqCst),
        "shutdown returned while a handler was still running"
    );
}

#[test]
fn dropping_an_idle_servicer_is_prompt() {
    let (servicer, _log) = logging();
    let started = Instant::now();
    drop(servicer);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "dropping an idle servicing path must not wait for anything"
    );
}

#[test]
fn dropping_waits_for_a_running_handler() {
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&finished);

    let servicer = Servicer::new(move |_: u64| {
        std::thread::sleep(Duration::from_millis(50));
        flag.store(true, Ordering::SeqCst);
    })
    .expect("create the servicing path");

    servicer.submit(0).expect("accepted");
    wait_until("the handler to start", || servicer.pending() == 0);
    drop(servicer);

    // The context the handler borrows lives in the work object, so returning from
    // `drop` before the handler finished would be a use-after-free rather than
    // merely untidy.
    assert!(finished.load(Ordering::SeqCst));
}

#[test]
fn many_servicers_tear_down_concurrently_without_wedging() {
    let workers: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(|| {
                let (servicer, log) = logging();
                for item in 0..50 {
                    servicer.submit(item).expect("accepted");
                }
                servicer.shut_down();
                drop(servicer);
                log.peak()
            })
        })
        .collect();

    let started = Instant::now();
    for worker in workers {
        let peak = worker.join().expect("worker");
        assert!(peak <= 1, "each path serialises independently");
    }
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "concurrent teardown wedged"
    );
}

#[test]
fn teardown_from_a_thread_other_than_the_creator_is_safe() {
    let (servicer, log) = logging();
    servicer.submit(1).expect("accepted");
    wait_until("the request", || log.items() == [1]);

    // The shape the monitor uses when it is dropped on a thread that did not
    // create it, which is the ordinary case for a monitor handed to a worker.
    std::thread::spawn(move || drop(servicer))
        .join()
        .expect("teardown thread");
}
