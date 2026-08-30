// Copyright (c) 2026 Mike Grier
//! Contract tests for `IoRing::completion_event` (M11.2).
//!
//! These are written against the *stated* rules -- D-19's edge-trigger
//! contract, D-20's hand-back-a-duplicate shape, and D-21's auto-reset,
//! one-waiter choice -- rather than against whatever the current
//! implementation happens to do. Each test names the rule it pins.
//!
//! The rules, restated once here so a failure is readable without opening
//! `DESIGN-NOTES.md`:
//!
//! 1. The event is signalled when the completion queue transitions from
//!    **empty to non-empty**. Not once per completion, and not
//!    level-triggered.
//! 2. A waiter must **drain to empty before waiting again**, on every pass.
//! 3. A **wake with nothing to pop is normal**; `completion_event`
//!    deliberately produces one at setup so a caller that already submitted
//!    never misses its backlog.
//! 4. The returned handle is an owned **duplicate**; the ring keeps its own,
//!    and repeat calls hand back duplicates of the *same* event rather than
//!    attaching a second one.

#![cfg(windows)]

use std::collections::HashMap;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;

use windows_ioring_sys::{Batch, IoRing, PushOptions, Token, capabilities};
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 512;

/// Generous, because a positive wait must not flake on a loaded machine.
/// Every test that pays it in full is one that would otherwise hang.
const SIGNAL_TIMEOUT_MS: u32 = 5_000;

/// The budget for the one wait that is *asserted* to time out. Short on
/// purpose: by the time it is entered, the completion it would have reported
/// has already been observed to have landed, so any wakeup that was coming
/// would arrive immediately. A generous value here would only lengthen the
/// suite without making the assertion any safer.
const LOST_WAKEUP_MS: u32 = 500;

/// Completions this crate mints all belong to reads whose tokens the test
/// still holds, so a drain that never reaches the expected count is a real
/// failure rather than a slow machine. Bounded so it fails instead of hangs.
const MAX_DRAIN_ATTEMPTS: usize = 512;

/// Read tokens still awaiting their completion, keyed by `UserData`.
type Pending = HashMap<usize, Token<Vec<u8>>>;

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-completion-event-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// A file with `CHUNKS * CHUNK_LEN` bytes of readable content, open for read.
fn fixture(tag: &str) -> File {
    let path = temp_file(tag);
    let mut content = vec![0_u8; CHUNKS * CHUNK_LEN];
    for (chunk_index, chunk) in content.chunks_mut(CHUNK_LEN).enumerate() {
        chunk.fill(chunk_index as u8);
    }
    std::fs::write(&path, &content).expect("write fixture file");
    std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read")
}

/// A fresh auto-reset, initially-unsignalled event standing in for whatever
/// non-ring handle a multiplexed consumer is really waiting on.
fn unrelated_event() -> OwnedHandle {
    // SAFETY: default security, unnamed, auto-reset, initially unsignalled --
    // the documented all-null-defaults form.
    let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    assert!(!raw.is_null(), "CreateEventW failed");
    // SAFETY: `CreateEventW` just produced this handle and nothing else owns
    // it, so `OwnedHandle` becomes its sole owner.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn signal(event: &OwnedHandle) {
    // SAFETY: `event` is a live event handle this test owns.
    assert!(
        unsafe { SetEvent(event.as_raw_handle()) } != 0,
        "SetEvent failed"
    );
}

/// Wait up to `timeout_ms` for `event`. Consumes the signal when it fires,
/// since D-21 makes these events auto-reset.
fn signalled_within(event: &OwnedHandle, timeout_ms: u32) -> bool {
    // SAFETY: `event` is a live event handle this test owns.
    let result = unsafe { WaitForSingleObject(event.as_raw_handle(), timeout_ms) };
    if result == WAIT_OBJECT_0 {
        true
    } else if result == WAIT_TIMEOUT {
        false
    } else {
        panic!("unexpected WaitForSingleObject result 0x{result:08X}");
    }
}

/// Is `event` signalled *at this instant*? Every negative assertion below
/// uses this rather than a timed wait, because each one is made at a point
/// where the ring has already been observed to be quiet -- so a zero
/// timeout is decisive rather than merely impatient.
fn signalled_now(event: &OwnedHandle) -> bool {
    signalled_within(event, 0)
}

/// The multiplexed wait a consumer actually writes: block until *either*
/// handle fires, and report which one did. `None` is a wait that timed out
/// with neither handle signalled.
fn wait_either(first: &OwnedHandle, second: &OwnedHandle, timeout_ms: u32) -> Option<usize> {
    let handles: [HANDLE; 2] = [first.as_raw_handle(), second.as_raw_handle()];
    // SAFETY: both handles are live and owned by this test, and `handles`
    // holds exactly the two entries the count promises. `bWaitAll = FALSE`.
    let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout_ms) };
    if result == WAIT_OBJECT_0 {
        Some(0)
    } else if result == WAIT_OBJECT_0 + 1 {
        Some(1)
    } else if result == WAIT_TIMEOUT {
        None
    } else {
        panic!("unexpected WaitForMultipleObjects result 0x{result:08X}");
    }
}

/// Queue `count` reads and submit them, blocking for `wait_operations`
/// completions (0 to return immediately).
fn submit_reads(
    ring: &mut IoRing,
    file: &File,
    count: usize,
    wait_operations: u32,
    pending: &mut Pending,
) {
    assert!(count <= CHUNKS, "the fixture only has {CHUNKS} chunks");
    let handle = file.as_raw_handle();
    let mut batch = Batch::new(ring);
    for chunk_index in 0..count {
        let buffer = vec![0_u8; CHUNK_LEN];
        let offset = (chunk_index * CHUNK_LEN) as u64;
        // SAFETY: `file` is the caller's and outlives every operation queued
        // here -- each test drains its ring to empty before dropping either.
        let token = unsafe { batch.read_raw(handle, buffer, offset, PushOptions::new()) }
            .expect("queue read");
        pending.insert(token.id(), token);
    }
    batch
        .submit_and_wait(wait_operations, SIGNAL_TIMEOUT_MS)
        .expect("submit");
}

/// Rule 2's drain: `try_pop` until it yields `None`, claiming each token so
/// nothing is leaked, and report how many completions this pass observed.
fn drain_to_empty(ring: &mut IoRing, pending: &mut Pending) -> usize {
    let mut popped = 0;
    while let Some(completion) = ring.try_pop().expect("pop completion") {
        completion.result().expect("read succeeded");
        if let Some(token) = pending.remove(&completion.user_data()) {
            let _buffer = token
                .claim_if(&completion)
                .expect("a token claims its own completion");
        }
        popped += 1;
    }
    popped
}

/// Drain until exactly `want` completions have been observed and the queue
/// is empty, so the ring can be dropped with nothing outstanding.
fn drain_exactly(ring: &mut IoRing, pending: &mut Pending, want: usize) {
    let mut popped = 0;
    let mut attempts = 0;
    while popped < want {
        attempts += 1;
        assert!(
            attempts <= MAX_DRAIN_ATTEMPTS,
            "only {popped} of {want} completions ever arrived"
        );
        popped += drain_to_empty(ring, pending);
    }
    assert_eq!(popped, want, "more completions arrived than were submitted");
}

/// Every test below needs the capability. Skipping silently would let the
/// whole file rot unnoticed, so this states the requirement loudly instead;
/// the capability's *absence* has its own test at the bottom of this file.
fn ring_with_event(submission: u32, completion: u32) -> (IoRing, OwnedHandle) {
    let mut ring = IoRing::new(submission, completion).expect("create ring");
    let event = ring.completion_event().expect(
        "this host must report IORING_FEATURE_SET_COMPLETION_EVENT to run the M11.2 contract tests",
    );
    (ring, event)
}

/// Consume the deliberate setup signal (rule 3) and confirm nothing is left
/// behind, leaving the ring quiet and the event unsignalled.
fn settle(ring: &mut IoRing, event: &OwnedHandle, pending: &mut Pending) {
    assert!(
        signalled_within(event, SIGNAL_TIMEOUT_MS),
        "attaching must signal once (rule 3)"
    );
    drain_to_empty(ring, pending);
    assert!(
        !signalled_now(event),
        "the setup signal must be consumable exactly once"
    );
}

// --- rule 3: the deliberate setup signal ------------------------------------

#[test]
fn attaching_to_a_fresh_ring_signals_once_and_leaves_nothing_to_pop() {
    let (mut ring, event) = ring_with_event(64, 64);

    assert!(
        signalled_within(&event, SIGNAL_TIMEOUT_MS),
        "attaching must signal once even with no work outstanding (rule 3)"
    );
    let mut pending = Pending::new();
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        0,
        "a wake with nothing to pop is normal, not an error (rule 3)"
    );
    assert!(
        !signalled_now(&event),
        "the setup signal is one edge, not a level held high"
    );
}

#[test]
fn attaching_to_a_ring_whose_queue_is_already_non_empty_still_signals() {
    // The case that matters: the kernel will *not* signal an attach into a
    // non-empty queue, and it will never signal afterwards either, because
    // the queue never returns to empty to re-arm the edge. Without the
    // deliberate setup signal this backlog is stranded permanently -- which
    // is exactly the `EventDelivery` bug M11.3 fixes.
    let file = fixture("backlog");
    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut pending = Pending::new();

    // Submit and let the completions land *before* the event exists.
    submit_reads(&mut ring, &file, CHUNKS, CHUNKS as u32, &mut pending);

    let event = ring.completion_event().expect("completion event");
    assert!(
        signalled_within(&event, SIGNAL_TIMEOUT_MS),
        "a caller that submitted before attaching must still be woken for its backlog"
    );

    drain_exactly(&mut ring, &mut pending, CHUNKS);
}

// --- rule 4: the returned handle is a duplicate of one shared event ---------

#[test]
fn a_repeat_call_hands_back_a_duplicate_of_the_same_event() {
    let (mut ring, first) = ring_with_event(64, 64);
    let second = ring.completion_event().expect("repeat completion event");

    // One event, one setup signal: an auto-reset event satisfies exactly one
    // wait, so consuming it through `first` must leave `second` unsignalled.
    // Two separately attached events would each carry their own signal.
    assert!(
        signalled_within(&first, SIGNAL_TIMEOUT_MS),
        "the setup signal must be visible through the first duplicate"
    );
    assert!(
        !signalled_now(&second),
        "a repeat call must return a duplicate of the same event, not attach a second one"
    );
}

#[test]
fn the_ring_still_signals_both_duplicates_after_a_repeat_call() {
    // `SetIoRingCompletionEvent` replaces rather than adds, so a repeat call
    // that attached a second event would silently detach the *first*
    // subsystem's. Round one is what catches that, and it has to come first:
    // asserting only that the second handle signals passes vacuously against
    // a detached first, since a freshly attached event signals perfectly
    // well. Verified by sabotage -- an earlier version of this test asserted
    // the second handle only, and a non-idempotent `completion_event` passed
    // it. Round two then reverses the order, which one shared auto-reset
    // event satisfies and two independent events could not.
    let file = fixture("idempotent");
    let (mut ring, first) = ring_with_event(64, 64);
    let second = ring.completion_event().expect("repeat completion event");
    let mut pending = Pending::new();
    settle(&mut ring, &first, &mut pending);

    submit_reads(&mut ring, &file, 1, 1, &mut pending);
    assert!(
        signalled_within(&first, SIGNAL_TIMEOUT_MS),
        "a repeat call must not detach the event the first call handed out"
    );
    assert!(
        !signalled_now(&second),
        "both handles name one event, so one completion edge satisfies exactly one wait"
    );
    drain_exactly(&mut ring, &mut pending, 1);

    submit_reads(&mut ring, &file, 1, 1, &mut pending);
    assert!(
        signalled_within(&second, SIGNAL_TIMEOUT_MS),
        "the handle from the repeat call names that same event"
    );
    assert!(
        !signalled_now(&first),
        "one edge cannot satisfy a wait on both handles if they are one event"
    );
    drain_exactly(&mut ring, &mut pending, 1);
}

#[test]
fn closing_one_duplicate_does_not_stop_the_ring_signalling_the_other() {
    // What makes the returned handle safe to hold, and to drop, for any
    // length of time: the ring owns its own copy, and the kernel still
    // signals the survivors. The *later* duplicate is the one closed here,
    // so the surviving assertion runs through the handle a non-idempotent
    // implementation would have detached -- closing the second and then
    // asserting on the second would pass against exactly the bug this is
    // meant to exclude.
    let file = fixture("closed-duplicate");
    let (mut ring, first) = ring_with_event(64, 64);
    let second = ring.completion_event().expect("repeat completion event");
    let mut pending = Pending::new();
    settle(&mut ring, &first, &mut pending);

    drop(second);

    submit_reads(&mut ring, &file, 1, 1, &mut pending);
    assert!(
        signalled_within(&first, SIGNAL_TIMEOUT_MS),
        "closing a duplicate must not disturb the ring's own event"
    );
    drain_exactly(&mut ring, &mut pending, 1);
}

#[test]
fn the_returned_handle_stays_valid_after_the_ring_is_dropped() {
    // The ring drops its own copy only after `CloseIoRing`, and the caller's
    // duplicate is independent: it simply never fires again. A handle the
    // ring had somehow closed on the caller's behalf would fail the wait
    // rather than time out, which `signalled_within` panics on.
    let (mut ring, event) = ring_with_event(8, 8);
    let mut pending = Pending::new();
    settle(&mut ring, &event, &mut pending);

    drop(ring);

    assert!(
        !signalled_now(&event),
        "an outlived duplicate must remain a valid, waitable, unsignalled handle"
    );
}

// --- rule 1: the edge -------------------------------------------------------

#[test]
fn a_completion_arriving_into_an_empty_queue_signals() {
    let file = fixture("edge");
    let (mut ring, event) = ring_with_event(64, 64);
    let mut pending = Pending::new();
    settle(&mut ring, &event, &mut pending);

    submit_reads(&mut ring, &file, 1, 1, &mut pending);

    assert!(
        signalled_within(&event, SIGNAL_TIMEOUT_MS),
        "empty -> non-empty is the edge the event reports (rule 1)"
    );
    drain_exactly(&mut ring, &mut pending, 1);
}

#[test]
fn many_completions_arriving_at_once_produce_exactly_one_wakeup() {
    // The half of rule 1 that a level-triggered reading gets wrong: the
    // event is not signalled once per completion, and a single
    // drain-to-empty retrieves the whole batch.
    let file = fixture("batch");
    let (mut ring, event) = ring_with_event(64, 64);
    let mut pending = Pending::new();
    settle(&mut ring, &event, &mut pending);

    submit_reads(&mut ring, &file, CHUNKS, CHUNKS as u32, &mut pending);

    assert!(
        signalled_within(&event, SIGNAL_TIMEOUT_MS),
        "the first completion into an empty queue must signal"
    );
    assert!(
        !signalled_now(&event),
        "completions arriving into an already non-empty queue must not signal again"
    );
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        CHUNKS,
        "one drain-to-empty must retrieve every completion the single wakeup covered"
    );
}

#[test]
fn the_edge_re_arms_after_every_drain_to_empty() {
    // Rule 1's other half: emptying the queue restores the empty state, so
    // the next completion signals again. Three rounds, because a
    // signal-once-and-never-again bug survives a single round.
    let file = fixture("re-arm");
    let (mut ring, event) = ring_with_event(64, 64);
    let mut pending = Pending::new();
    settle(&mut ring, &event, &mut pending);

    for round in 0..3 {
        submit_reads(&mut ring, &file, 1, 1, &mut pending);
        assert!(
            signalled_within(&event, SIGNAL_TIMEOUT_MS),
            "round {round}: the edge must re-arm after the previous drain"
        );
        drain_exactly(&mut ring, &mut pending, 1);
        assert!(
            !signalled_now(&event),
            "round {round}: a full drain must leave no leftover signal"
        );
    }
}

// --- the multiplexed wait: the configuration that makes a violation visible --

#[test]
fn the_ring_still_wakes_after_an_unrelated_handle_fires_in_a_multiplexed_wait() {
    // This is the shape `completion_event` exists for -- ring I/O waited on
    // alongside non-ring I/O -- and it is the configuration in which an
    // edge-trigger violation becomes observable at all.
    //
    // Rounds 0-3 are a conformant loop: drain to empty on *every* pass
    // (rule 2), tolerate a wake with nothing to pop (rule 3). Round 4 then
    // breaks rule 2 on purpose and asserts the resulting lost wakeup, so the
    // deadlock is demonstrated here rather than described and left to hang a
    // real consumer.
    //
    // Sabotage-verified, and the measured result corrected an assumption
    // worth recording: replacing round 1's drain with a single `try_pop`
    // fails this test at *round 2*, not round 3, because round 2's
    // unrelated-wake drain rescues the seven stranded completions. That is
    // precisely why rule 2 says every pass and not merely every ring pass --
    // the drain on the pass the ring did not wake is what keeps the queue
    // recoverable.
    let file = fixture("multiplexed");
    let (mut ring, event) = ring_with_event(64, 64);
    let other = unrelated_event();
    let mut pending = Pending::new();
    settle(&mut ring, &event, &mut pending);

    // Round 0: the unrelated handle wakes a wait the ring has nothing for.
    signal(&other);
    assert_eq!(
        wait_either(&event, &other, SIGNAL_TIMEOUT_MS),
        Some(1),
        "round 0: the unrelated handle must wake the multiplexed wait"
    );
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        0,
        "round 0: a wake with nothing to pop is normal (rule 3)"
    );

    // Round 1: a whole batch arrives under one wakeup, and rule 1 gives no
    // second signal for the rest of it -- draining to empty here is the only
    // thing that re-arms the edge for round 3.
    submit_reads(&mut ring, &file, CHUNKS, CHUNKS as u32, &mut pending);
    assert_eq!(
        wait_either(&event, &other, SIGNAL_TIMEOUT_MS),
        Some(0),
        "round 1: the ring must wake the wait after the unrelated handle already did"
    );
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        CHUNKS,
        "round 1: one drain-to-empty must retrieve the whole batch the single wakeup covered"
    );

    // Round 2: the unrelated handle again, between two ring wakeups.
    signal(&other);
    assert_eq!(
        wait_either(&event, &other, SIGNAL_TIMEOUT_MS),
        Some(1),
        "round 2: the unrelated handle must still wake the wait"
    );
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        0,
        "round 2: rule 2 says drain on every pass, not only the ring's"
    );

    // Round 3: the payoff of the conformant loop. This wakeup exists only
    // because round 1 emptied the queue and so re-armed the edge.
    submit_reads(&mut ring, &file, 1, 1, &mut pending);
    assert_eq!(
        wait_either(&event, &other, SIGNAL_TIMEOUT_MS),
        Some(0),
        "round 3: the ring must still wake the wait after two unrelated wakeups"
    );
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        1,
        "round 3: the final completion must be retrievable"
    );

    // Round 4: the violation itself. Pop one instead of draining to empty,
    // and the queue never returns to empty -- so the next completion, which
    // has demonstrably landed, arrives into a non-empty queue and signals
    // nothing. A conformant consumer never reaches this state; a consumer
    // that assumes a level-triggered event reaches it immediately and then
    // blocks forever.
    submit_reads(&mut ring, &file, CHUNKS, CHUNKS as u32, &mut pending);
    assert_eq!(
        wait_either(&event, &other, SIGNAL_TIMEOUT_MS),
        Some(0),
        "round 4: the batch's first completion must wake the wait"
    );
    let stranded = ring
        .try_pop()
        .expect("pop one")
        .expect("the batch left completions queued");
    let _buffer = pending
        .remove(&stranded.user_data())
        .expect("the popped completion matches a held token")
        .claim_if(&stranded)
        .expect("a token claims its own completion");

    submit_reads(&mut ring, &file, 1, 1, &mut pending);
    assert_eq!(
        wait_either(&event, &other, LOST_WAKEUP_MS),
        None,
        "a completion arriving into a queue that never returned to empty must not signal (rule 1)"
    );

    // Nothing was lost but the wakeup: the entries are still there, and
    // draining recovers every one of them.
    assert_eq!(
        drain_to_empty(&mut ring, &mut pending),
        CHUNKS,
        "the stranded entries must still be poppable once the waiter drains"
    );
    assert!(pending.is_empty(), "every token must have been claimed");
    assert!(
        !signalled_now(&event),
        "the ring must end quiet, with no leftover signal after a full drain"
    );
}

// --- the capability gate ----------------------------------------------------

#[test]
fn completion_event_reports_unsupported_exactly_when_the_capability_is_absent() {
    // Both branches are stated contract, so this asserts whichever one the
    // host is: a supporting host must succeed, and a host without
    // `IORING_FEATURE_SET_COMPLETION_EVENT` must refuse with `Unsupported`
    // rather than silently substituting a polling thread.
    let supported = capabilities()
        .expect("capabilities")
        .supports_completion_event;
    let mut ring = IoRing::new(8, 8).expect("create ring");

    match ring.completion_event() {
        Ok(event) => {
            assert!(
                supported,
                "completion_event succeeded on a host that does not report the capability"
            );
            assert!(
                signalled_within(&event, SIGNAL_TIMEOUT_MS),
                "a successful attach owes the caller its setup signal"
            );
        }
        Err(error) => {
            assert!(
                !supported,
                "completion_event failed on a host that reports the capability: {error}"
            );
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::Unsupported,
                "a missing capability must be reported as Unsupported"
            );
        }
    }
}
