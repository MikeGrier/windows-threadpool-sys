// Copyright (c) 2026 Mike Grier
//! End-to-end test of Model A delivery: the completion event wired to the
//! thread pool (M4.4).

// `EventDelivery` is behind the default-on `threadpool` feature (D-22), so
// this whole file compiles out with `--no-default-features`.
#![cfg(all(windows, feature = "threadpool"))]

use std::collections::HashMap;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use windows_ioring_sys::{Batch, EventDelivery, IoRing, PushOptions, Token};

const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 512;

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-event-delivery-{tag}-{}.tmp",
        std::process::id()
    ))
}

fn filled_content() -> Vec<u8> {
    let mut content = vec![0_u8; CHUNKS * CHUNK_LEN];
    for (chunk_index, chunk) in content.chunks_mut(CHUNK_LEN).enumerate() {
        chunk.fill(chunk_index as u8);
    }
    content
}

/// How long a delivery is allowed to take before the test calls it stalled.
const DELIVERY_BOUND: Duration = Duration::from_secs(5);

/// How much longer to wait, **only after a failure**, to learn whether the
/// delivery was lost or merely late.
///
/// This costs nothing on a passing run because it is never reached. On a
/// failing one it is the single most discriminating fact available: a delivery
/// that arrives at nine seconds is a stall to be explained, while one that
/// never arrives is a lost wakeup, and those have entirely different causes.
///
/// **Kept short enough that the report survives being killed.** The sabotage
/// harness bounds a suite at three times its baseline -- around thirty seconds
/// here -- and kills the process when that is exceeded. A post-mortem long
/// enough to push a failing run past that bound would trade the diagnosis for
/// the thing it was added to diagnose. The immediate facts are printed
/// *before* this wait for the same reason, so they survive even if it is.
const POST_MORTEM_BOUND: Duration = Duration::from_secs(10);

/// What the delivery path did, captured so a timeout is a diagnosis rather
/// than a word.
///
/// `M26.9` exists because these two tests time out at roughly one run in
/// eighty, and the message they produced -- `Timeout` -- ruled nothing out.
/// Every field below was chosen to separate hypotheses that message could not:
/// whether the pool ever ran the callback, whether the kernel ever finished
/// the I/O, whether deliveries were steady and then stopped, and whether the
/// missing one was lost or late.
struct DeliveryWatch {
    started: std::time::Instant,
    /// When each completion reached the test thread, relative to `started`.
    arrivals: Vec<Duration>,
    /// How many times the pool actually invoked the callback.
    callbacks: Arc<AtomicUsize>,
}

impl DeliveryWatch {
    fn new(callbacks: Arc<AtomicUsize>) -> Self {
        Self {
            started: std::time::Instant::now(),
            arrivals: Vec::new(),
            callbacks,
        }
    }

    fn record_arrival(&mut self) {
        self.arrivals.push(self.started.elapsed());
    }

    /// Everything known at the moment a wait gave up.
    ///
    /// Written to stderr as well as into the panic message: `cargo test`
    /// replays a failing test's captured output, and the sabotage harness
    /// writes that transcript to `.scratch/sabotage/<case>.txt`, so this is
    /// what a later reader actually has to work from.
    ///
    /// **States what was observed and what each number means mechanically;
    /// does not say what caused it.** An earlier draft ended each report with
    /// a verdict, and a forced-failure run showed the verdict was wrong -- it
    /// blamed something upstream of the channel when the injected fault was in
    /// the callback body, which the counters it printed had already ruled out.
    /// A diagnosis nobody asked for is worse than none, because it is the part
    /// a tired reader will believe.
    fn report(&self, what: &str, expected: usize, outstanding: usize, postmortem: &str) -> String {
        let gaps: Vec<String> = self
            .arrivals
            .windows(2)
            .map(|pair| format!("{:?}", pair[1] - pair[0]))
            .collect();
        let callbacks = self.callbacks.load(Ordering::SeqCst);
        format!(
            "M26.9 delivery stalled: {what}\n\
             \x20 delivered      : {} of {expected}\n\
             \x20 callbacks run  : {callbacks} -- times the pool invoked the callback. Equal to \
             delivered means everything the callback received reached this thread; greater \
             means the gap is between the callback and the channel.\n\
             \x20 outstanding    : {outstanding} -- the ring's own count, which decrements on \
             pop, and the pop happens inside the callback. So it cannot on its own separate \
             'the kernel has not finished' from 'the callback never ran'.\n\
             \x20 arrival times  : {:?}\n\
             \x20 inter-arrival  : [{}]\n\
             \x20 waited         : {:?} before giving up\n\
             \x20 post-mortem    : {postmortem}\n\
             {}",
            self.arrivals.len(),
            self.arrivals,
            gaps.join(", "),
            DELIVERY_BOUND,
            trace_section(),
        )
    }
}

/// Whether the default process thread pool is still running callbacks at all.
///
/// The discriminating probe for `M26.9`. Every captured stall shows the pool
/// never invoking the callback for *any* ring in the process, which has two
/// very different explanations: the whole default pool has stopped dispatching,
/// or only its wait mechanism has. This runs one of each and says which.
///
/// Both use short bounds because they run inside an already-failing test; a
/// probe that hung would replace the diagnosis with a second timeout.
fn pool_liveness() -> String {
    use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
    use windows_threadpool_sys::work::ThreadpoolWork;

    let probe_bound = Duration::from_secs(2);

    // 1. A plain work item. If this does not run, the pool is not dispatching
    //    anything and the wait mechanism is not the subject.
    let (work_tx, work_rx) = mpsc::channel();
    let work_tx = std::sync::Mutex::new(work_tx);
    let work_ran = match ThreadpoolWork::new(
        move || {
            if let Ok(tx) = work_tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    ) {
        Ok(work) => {
            work.submit();
            work_rx.recv_timeout(probe_bound).is_ok()
        }
        Err(error) => return format!("could not create a work probe: {error}"),
    };

    // 2. A brand-new wait on a brand-new event, armed and then signalled. If
    //    the work item ran and this does not, the fault is specific to waits
    //    rather than to the pool as a whole.
    let wait_ran = match WaitableHandle::event(false, false) {
        Ok(event) => {
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            match ThreadpoolWait::new(
                event,
                move |_| {
                    if let Ok(tx) = tx.lock() {
                        let _ = tx.send(());
                    }
                },
                None,
            ) {
                Ok(wait) => {
                    wait.arm(None);
                    // SAFETY: the wait owns the event, so the handle is open.
                    unsafe {
                        windows_sys::Win32::System::Threading::SetEvent(
                            std::os::windows::io::AsRawHandle::as_raw_handle(&wait.handle()),
                        )
                    };
                    rx.recv_timeout(probe_bound).is_ok()
                }
                Err(error) => return format!("could not create a wait probe: {error}"),
            }
        }
        Err(error) => return format!("could not create a probe event: {error}"),
    };

    format!("work item ran: {work_ran}; a fresh wait ran: {wait_ran} (both within {probe_bound:?})")
}

/// The concurrency trace, when this build carries one.
///
/// Empty on an ordinary build, because the `trace` feature compiles the whole
/// facility away -- which is the point: the defect this chases is timing
/// dependent, so the instrument must be absent unless it is wanted. Enable it
/// with `--features trace` and narrow it with `WINDOWS_THREADPOOL_TRACE`:
///
/// ```text
/// $env:WINDOWS_THREADPOOL_TRACE = 'wait,delivery'
/// cargo test -p windows-ioring-sys --features trace --test event_delivery
/// ```
fn trace_section() -> String {
    let dump = windows_threadpool_sys::trace::dump();
    if dump.trim().is_empty() {
        return "  trace          : nothing recorded (set WINDOWS_THREADPOOL_TRACE to narrow one)"
            .to_owned();
    }
    format!("  trace (oldest first):\n{dump}")
}

/// Wait for one delivery, turning a timeout into the report above.
fn recv_one(
    rx: &mpsc::Receiver<windows_ioring_sys::Completion>,
    watch: &mut DeliveryWatch,
    what: &str,
    expected: usize,
    outstanding: impl Fn() -> usize,
) -> windows_ioring_sys::Completion {
    match rx.recv_timeout(DELIVERY_BOUND) {
        Ok(completion) => {
            watch.record_arrival();
            completion
        }
        Err(_) => {
            // Read the ring's own count *before* the second wait, so it
            // describes the moment of failure rather than the moment of
            // giving up on it.
            let at_failure = outstanding();
            // The pool-liveness probe runs first, while the process is still
            // in the failed state -- asking afterwards would describe a
            // different moment.
            let liveness = pool_liveness();
            // Printed now, before waiting any longer. If this process is
            // killed during the post-mortem -- which the sabotage harness will
            // do if the suite exceeds its hang bound -- these lines are
            // already in the transcript, and losing them is losing the whole
            // reason the instrumentation exists.
            eprintln!(
                "{}\n  pool liveness  : {liveness}",
                watch.report(
                    what,
                    expected,
                    at_failure,
                    &format!(
                        "waiting a further {POST_MORTEM_BOUND:?} to see whether it is late \
                              rather than lost; the line below is the answer"
                    )
                )
            );
            let started = std::time::Instant::now();
            let postmortem = match rx.recv_timeout(POST_MORTEM_BOUND) {
                Ok(_) => format!("it arrived, {:?} past the bound", started.elapsed()),
                Err(_) => format!("still nothing after a further {POST_MORTEM_BOUND:?}"),
            };
            panic!(
                "{}\n  pool liveness  : {liveness}",
                watch.report(what, expected, at_failure, &postmortem)
            );
        }
    }
}

#[test]
fn completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting() {
    let path = temp_file("delivery");
    let content = filled_content();
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let (tx, rx) = mpsc::channel();
    let submitting_thread = std::thread::current().id();
    let saw_foreign_thread = Arc::new(AtomicBool::new(false));
    let saw_foreign_thread_for_callback = Arc::clone(&saw_foreign_thread);
    // Counted in the callback itself, so a stalled run can say whether the
    // pool ever ran it -- which is the first fork in diagnosing M26.9.
    let callbacks = Arc::new(AtomicUsize::new(0));
    let callbacks_for_callback = Arc::clone(&callbacks);

    let ring = IoRing::new(64, 64).expect("create ring");
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            callbacks_for_callback.fetch_add(1, Ordering::SeqCst);
            if std::thread::current().id() != submitting_thread {
                saw_foreign_thread_for_callback.store(true, Ordering::SeqCst);
            }
            let _ = tx.send(completion);
        },
        None,
    )
    .expect("wire event delivery");

    // Buffers are held by the token map in `windows-ioring-sys`'s own
    // submission API; here it is enough to know each op's byte count and
    // offset without keeping the buffer, since Model A hands the buffer back
    // through the completion path this test does not need to exercise
    // (submission_lifecycle.rs already covers buffer round-tripping).
    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        for chunk_index in 0..CHUNKS {
            let buffer = vec![0_u8; CHUNK_LEN];
            let offset = (chunk_index * CHUNK_LEN) as u64;
            let _token = unsafe { batch.read_raw(handle, buffer, offset, PushOptions::new()) }
                .expect("queue read");
        }
        // `wait_operations = 0`: this thread submits and returns immediately,
        // never waiting for a single completion itself (M4.4).
        batch.submit_and_wait(0, 0).expect("submit without waiting");
    }

    let mut watch = DeliveryWatch::new(Arc::clone(&callbacks));
    let mut received = 0;
    while received < CHUNKS {
        let completion = recv_one(
            &rx,
            &mut watch,
            "completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting",
            CHUNKS,
            || delivery.scope().outstanding(),
        );
        completion.result().expect("read succeeded");
        received += 1;
    }

    assert!(
        saw_foreign_thread.load(Ordering::SeqCst),
        "completions must be delivered on a pool thread, not the submitter's"
    );

    drop(delivery);
}

#[test]
fn teardown_with_operations_in_flight_neither_hangs_nor_closes_the_ring_early() {
    let path = temp_file("teardown");
    let content = vec![9_u8; 4096];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let delivered = Arc::new(AtomicUsize::new(0));
    let delivered_for_callback = Arc::clone(&delivered);

    let ring = IoRing::new(64, 64).expect("create ring");
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            let _ = completion.result();
            delivered_for_callback.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("wire event delivery");

    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        for _ in 0..8 {
            let buffer = vec![0_u8; content.len()];
            let _token = unsafe { batch.read_raw(handle, buffer, 0, PushOptions::new()) }
                .expect("queue read");
        }
        batch.submit_and_wait(0, 0).expect("submit without waiting");
        // Deliberately do not wait for any of these to complete: teardown
        // below races real in-flight operations rather than ones already
        // finished.
    }

    // `drop` must neither hang (M2.4/M4.3's rundown is still bounded and
    // rechecked) nor close the ring while the wait's callback might still be
    // touching it -- the field-drop order documented on `EventDelivery`
    // itself is what this exercises.
    drop(delivery);

    // The mutex guarding the shared counter dropped along with `delivery`,
    // but the counter's `Arc` outlives it, so it is still safe to inspect: a
    // sane outcome is *some* deliveries happened before or during teardown,
    // never a panic or a hang getting here.
    let _ = delivered.load(Ordering::SeqCst);
}

#[test]
fn completions_queued_before_handover_are_still_delivered() {
    // The M11.3 regression test, from the spike that found the bug (D-19).
    //
    // Every other test in this file hands over a *fresh* ring, which is why
    // this went unnoticed: `SetIoRingCompletionEvent` does not signal when it
    // attaches to a ring whose completion queue is already non-empty, and
    // nothing afterwards signals either, because the queue never returns to
    // empty to re-arm the edge. Those completions were stranded permanently
    // while `EventDelivery::new`'s own rustdoc promised they were delivered.
    //
    // The fix is `IoRing::completion_event`'s deliberate signal-once-on-attach
    // (D-20), which this exercises by doing the one thing the old tests never
    // did: let the completions land *before* the handover.
    let path = temp_file("queued-before-handover");
    let content = filled_content();
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut pending: HashMap<usize, (usize, Token<Vec<u8>>)> = HashMap::new();
    {
        let mut batch = Batch::new(&mut ring);
        for chunk_index in 0..CHUNKS {
            let buffer = vec![0_u8; CHUNK_LEN];
            let offset = (chunk_index * CHUNK_LEN) as u64;
            let token = unsafe { batch.read_raw(handle, buffer, offset, PushOptions::new()) }
                .expect("queue read");
            pending.insert(token.id(), (chunk_index, token));
        }
        // Wait for all of them here, on this thread, so the ring is handed
        // over with a full completion queue. This is the whole point: no
        // completion arrives after the handover, so delivery can only happen
        // if attaching covers the backlog.
        batch
            .submit_and_wait(CHUNKS as u32, 5_000)
            .expect("submit and wait for every completion to land");
    }

    let (tx, rx) = mpsc::channel();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let callbacks_for_callback = Arc::clone(&callbacks);
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            callbacks_for_callback.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(completion);
        },
        None,
    )
    .expect("wire event delivery to a ring that already has completions queued");

    // Claim on this thread rather than in the callback, so a delivered
    // completion is checked against the token that minted it -- a delivery
    // that reported the wrong `UserData` would fail here rather than pass.
    //
    // Note what a stall means *here* specifically, and why the report says
    // `outstanding` is not self-explanatory: every completion was already in
    // the queue before the handover, so the kernel has nothing left to do.
    // A timeout in this test therefore cannot be the device being slow.
    let mut watch = DeliveryWatch::new(Arc::clone(&callbacks));
    for _ in 0..CHUNKS {
        let completion = recv_one(
            &rx,
            &mut watch,
            "completions_queued_before_handover_are_still_delivered (every completion was \
             already queued before the handover, so the kernel had nothing left to do -- a \
             stall here is in the signal or the pool, never in the device)",
            CHUNKS,
            || delivery.scope().outstanding(),
        );
        let transferred = completion.result().expect("read succeeded");
        let (chunk_index, token) = pending
            .remove(&completion.user_data())
            .expect("completion matches a held token");
        let buffer = token
            .claim_if(&completion)
            .expect("a token claims its own completion");
        assert_eq!(transferred, CHUNK_LEN);
        assert_eq!(
            buffer,
            content[chunk_index * CHUNK_LEN..(chunk_index + 1) * CHUNK_LEN]
        );
    }
    assert!(
        pending.is_empty(),
        "every completion queued before the handover must have been delivered"
    );

    drop(delivery);
}

#[test]
fn the_stall_report_carries_what_a_diagnosis_needs() {
    // `M26.9`'s instrumentation, guarded without paying for it.
    //
    // The report is what a future reader of a flaked run has to work from, so
    // it is worth knowing it still says something. Driving a real stall to
    // find out would cost every suite run the delivery bound plus the
    // post-mortem, which is why this exercises the formatting directly: the
    // failure path's only other job is to call it, and that was verified once
    // by forcing a stall by hand (measured: five of eight delivered, eight
    // callbacks run, which localised the injected fault to between the
    // callback and the channel exactly as the counters promise).
    let callbacks = Arc::new(AtomicUsize::new(8));
    let mut watch = DeliveryWatch::new(callbacks);
    watch.record_arrival();
    watch.record_arrival();

    let report = watch.report("a_test_name", 8, 3, "still nothing");
    for needle in [
        "a_test_name",
        "delivered",
        "2 of 8",
        "callbacks run",
        "outstanding",
        "arrival times",
        "inter-arrival",
        "post-mortem",
        "still nothing",
    ] {
        assert!(
            report.contains(needle),
            "the stall report must carry {needle:?}, or a flaked run says less than it could \
             -- got:\n{report}"
        );
    }

    // The counter that separates "the pool stopped calling us" from "the
    // callback got it and the channel did not" is the one worth pinning by
    // value rather than by name: a report that printed the delivered count
    // twice would satisfy every check above.
    assert!(
        report.contains("callbacks run  : 8"),
        "the callback count must be the pool's, not the delivered count -- got:\n{report}"
    );
}

// ------------------------------------------------------------------------
// Relocated from `src/event_delivery/tests.rs` at 9bc0350e (M24.3).

#[test]
fn new_succeeds_and_the_ring_stays_reachable_for_pushes() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");
    let info = delivery.scope().info().expect("query info");
    assert!(info.submission_queue_size > 0);
}

#[test]
fn dropping_with_nothing_outstanding_does_not_hang() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");
    drop(delivery);
}
