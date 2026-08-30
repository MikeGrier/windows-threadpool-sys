// Copyright (c) 2026 Mike Grier
//! The handover precondition: what state the ring is in at the instant a
//! completion event is attached to it (M17.1).
//!
//! [completion_event.rs](completion_event.rs) pins the two *settled* handover
//! states -- a fresh ring, and one whose backlog has already landed -- and
//! [event_delivery.rs](event_delivery.rs) pins the same two through the pool.
//! Two cells were left open, and this file closes them.
//!
//! **The mixed queue.** The backlog and the re-arm are each covered alone.
//! What was never covered is one attach that has to serve *both*: completions
//! already queued when the event is attached (covered by `completion_event`'s
//! deliberate signal-once-on-attach, D-20) together with completions arriving
//! afterwards (covered by the empty-to-non-empty edge, D-19). A seam between
//! those two mechanisms would strand work in exactly the way
//! [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) did.
//!
//! **Why the in-flight case uses unbuffered I/O.** The original plan was to
//! race the attach against operations still in flight by submitting without
//! waiting and sweeping a delay across the window. That does not work for
//! *buffered* reads, which complete **synchronously inside**
//! `submit_and_wait`: "submitted but not yet complete" never exists to be
//! sampled. Measured before being believed -- 512 reads of 64 KiB, and four
//! smaller shapes, produced a full queue at attach time in 80 of 80 attempts,
//! with no partial split at any size -- so a sweep only looks like coverage
//! while restating the already-queued case. Unbuffered reads are genuinely
//! asynchronous, so the state is reached that way instead, and the test that
//! does it *checks it actually got there* rather than assuming. See
//! [DESIGN-NOTES.md](DESIGN-NOTES.md) `D-40`.//!
//! Every test here is bound to [`RingContract`], so a completion that is lost,
//! duplicated, or never claimed is reported as a contract violation rather
//! than inferred from a count.

#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_ioring_sys::contract::RingContract;
use windows_ioring_sys::{Batch, IoBuf, IoBufMut, IoRing, PushOptions, Token};
use windows_sys::Win32::Foundation::{
    GENERIC_READ, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

#[cfg(feature = "threadpool")]
use std::sync::mpsc;
#[cfg(feature = "threadpool")]
use std::time::Duration;
#[cfg(feature = "threadpool")]
use windows_ioring_sys::EventDelivery;

/// Reads per wave. The fixture holds one chunk per read of every wave.
const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 512;
const WAVES: usize = 3;

/// Generous, because a positive wait must not flake on a loaded machine.
/// Every test that pays it in full is one that would otherwise hang.
const SIGNAL_TIMEOUT_MS: u32 = 5_000;

/// Bounded so a lost wakeup fails instead of looping forever. A wake with
/// nothing to pop is legal (D-20), so the loop cannot simply count wakes.
const MAX_WAITS: usize = 512;

#[cfg(feature = "threadpool")]
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Read tokens still awaiting their completion, keyed by `UserData`.
type Pending<B = Vec<u8>> = HashMap<usize, Token<B>>;

/// Reads issued against the unbuffered fixture, and their size. Large enough
/// that a device read cannot finish inside the microseconds an attach takes.
const DIRECT_OPS: usize = 8;
const DIRECT_LEN: usize = 1024 * 1024;

/// Attempts at catching the unbuffered reads mid-flight. A handful, because
/// each one issues `DIRECT_OPS * DIRECT_LEN` of real device I/O; the test
/// asserts that *at least one* attempt reached the in-flight state rather than
/// requiring every attempt to, so an unlucky one cannot make it flake.
const DIRECT_ATTEMPTS: usize = 4;

/// `FILE_FLAG_NO_BUFFERING` requires the buffer address, the file offset, and
/// the length to be sector-aligned. 4096 satisfies both 512e and 4Kn devices.
const ALIGN: usize = 4096;

/// A heap buffer whose first byte is `ALIGN`-aligned.
///
/// `Vec<u8>` gives no alignment guarantee beyond `u8`'s -- and cannot be made
/// to, since a hand-aligned allocation would then be freed under the wrong
/// layout -- so this over-allocates and offsets into the allocation. The heap
/// block does not move when the `Aligned` value moves, which is what [`IoBuf`]
/// requires. Same shape as [flush_barrier.rs](flush_barrier.rs)'s, extended to
/// [`IoBufMut`] because these are reads rather than writes.
struct Aligned {
    storage: Vec<u8>,
    offset: usize,
    len: usize,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let storage = vec![0_u8; len + ALIGN];
        let base = storage.as_ptr() as usize;
        let offset = (ALIGN - (base % ALIGN)) % ALIGN;
        Self {
            storage,
            offset,
            len,
        }
    }
}

// SAFETY: `storage` is a heap allocation that is never reallocated or resized
// while this value exists, so the address it reports is stable across moves and
// for the value's whole life; `offset` is within the over-allocation by
// construction, and `len` bytes from there are initialized and stay allocated.
// `len` is fixed at construction.
unsafe impl IoBuf for Aligned {
    fn stable_ptr(&self) -> *const u8 {
        // SAFETY: `offset <= ALIGN` and the allocation is `len + ALIGN` bytes.
        unsafe { self.storage.as_ptr().add(self.offset) }
    }

    fn bytes_len(&self) -> usize {
        self.len
    }
}

// SAFETY: the same allocation and offset `IoBuf` reports, so the two addresses
// are equal and equally stable, as the trait requires.
unsafe impl IoBufMut for Aligned {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        // SAFETY: `offset <= ALIGN` and the allocation is `len + ALIGN` bytes.
        unsafe { self.storage.as_mut_ptr().add(self.offset) }
    }
}

/// Open `path` for unbuffered, overlapped reading, so its completions are
/// genuinely asynchronous rather than resolved inside submit (D-40).
fn open_unbuffered(path: &Path) -> OwnedHandle {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a null-terminated wide path that outlives the call; the
    // security-attributes and template arguments are the documented null
    // defaults.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            std::ptr::null_mut(),
        )
    };
    assert!(
        !raw.is_null() && raw != INVALID_HANDLE_VALUE,
        "CreateFileW failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: `CreateFileW` just returned a fresh handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw.cast::<c_void>()) }
}

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-handover-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// A file with `WAVES * CHUNKS * CHUNK_LEN` bytes of readable content, open
/// for read, so every wave can read a disjoint span.
fn fixture(tag: &str) -> File {
    let path = temp_file(tag);
    let mut content = vec![0_u8; WAVES * CHUNKS * CHUNK_LEN];
    for (chunk_index, chunk) in content.chunks_mut(CHUNK_LEN).enumerate() {
        chunk.fill(chunk_index as u8);
    }
    std::fs::write(&path, &content).expect("write fixture file");
    std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read")
}

/// Wait up to `timeout_ms` for `event`, consuming the signal when it fires
/// (D-21 makes these events auto-reset).
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

/// Queue wave `wave`'s `CHUNKS` reads into `batch`, without submitting.
///
/// Split from `submit_wave` so the same queueing serves both a plain ring and
/// an [`EventDelivery`] scope, which hands out a [`Batch`] rather than a
/// `&mut IoRing` (M18.6). Submitting stays with the caller because
/// `Batch::submit_and_wait` consumes the batch.
///
/// Each wave reads a disjoint span of the fixture, so a completion carrying
/// the wrong `UserData` is caught by the claim rather than passing unnoticed.
fn queue_wave(
    batch: &mut Batch<'_>,
    file: &File,
    wave: usize,
    contract: &mut RingContract,
    pending: &mut Pending,
) {
    let handle = file.as_raw_handle();
    for chunk_index in 0..CHUNKS {
        let buffer = vec![0_u8; CHUNK_LEN];
        let offset = ((wave * CHUNKS + chunk_index) * CHUNK_LEN) as u64;
        // SAFETY: `file` is the caller's and outlives every operation queued
        // here -- each test observes every completion before it drops.
        let token = unsafe { batch.read_raw(handle, buffer, offset, PushOptions::new()) }
            .expect("queue read");
        contract.observe_push(token.id());
        pending.insert(token.id(), token);
    }
}

/// [`queue_wave`] against a ring the test owns outright, then submit.
fn submit_wave(
    ring: &mut IoRing,
    file: &File,
    wave: usize,
    contract: &mut RingContract,
    pending: &mut Pending,
) {
    let mut batch = Batch::new(ring);
    queue_wave(&mut batch, file, wave, contract, pending);
    batch.submit_and_wait(0, 0).expect("submit without waiting");
}

/// Pop until the queue is empty, reporting each completion and claim to the
/// contract, and return how many this pass observed.
fn drain_to_empty<B: Send + 'static>(
    ring: &mut IoRing,
    contract: &mut RingContract,
    pending: &mut Pending<B>,
) -> usize {
    let mut popped = 0;
    while let Some(completion) = ring.try_pop().expect("pop completion") {
        contract.observe_completion(completion.user_data());
        completion.result().expect("read succeeded");
        let token = pending
            .remove(&completion.user_data())
            .expect("completion matches a held token");
        let _buffer = token
            .claim_if(&completion)
            .expect("a token claims its own completion");
        contract.observe_claim(completion.user_data());
        popped += 1;
    }
    popped
}

/// Wait and drain until `want` completions have been seen in total, obeying
/// the drain-to-empty-before-waiting-again rule (D-19).
fn wait_and_drain<B: Send + 'static>(
    ring: &mut IoRing,
    event: &OwnedHandle,
    contract: &mut RingContract,
    pending: &mut Pending<B>,
    want: usize,
    context: &str,
) {
    let mut popped = 0;
    let mut waits = 0;
    while popped < want {
        waits += 1;
        assert!(
            waits <= MAX_WAITS,
            "{context}: woken {waits} times with only {popped} of {want} completions drained"
        );
        assert!(
            signalled_within(event, SIGNAL_TIMEOUT_MS),
            "{context}: {popped} of {want} completions drained and the ring never signalled \
             again -- a wakeup was lost"
        );
        popped += drain_to_empty(ring, contract, pending);
    }
    assert_eq!(
        popped, want,
        "{context}: more completions arrived than were submitted"
    );
}

// --- cell (a): one attach serving both a backlog and later arrivals ---------

#[test]
fn an_attach_serves_both_the_backlog_and_the_wave_that_follows_it() {
    let file = fixture("mixed-queue");
    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut contract = RingContract::new();
    let mut pending = Pending::new();

    // Wave 0 lands *before* the event exists, so only the deliberate setup
    // signal can account for it.
    submit_wave(&mut ring, &file, 0, &mut contract, &mut pending);

    let event = ring.completion_event().expect(
        "this host must report IORING_FEATURE_SET_COMPLETION_EVENT to run the handover tests",
    );

    // Wave 1 lands *after* the attach, into a queue wave 0 already made
    // non-empty -- so it raises no edge of its own and is only ever seen by a
    // waiter that drains to empty rather than counting wakeups.
    submit_wave(&mut ring, &file, 1, &mut contract, &mut pending);

    wait_and_drain(
        &mut ring,
        &event,
        &mut contract,
        &mut pending,
        2 * CHUNKS,
        "mixed queue",
    );

    // With the queue now empty the edge must arm again, so a third wave is
    // delivered on its own signal rather than on the setup one.
    submit_wave(&mut ring, &file, 2, &mut contract, &mut pending);
    wait_and_drain(
        &mut ring,
        &event,
        &mut contract,
        &mut pending,
        CHUNKS,
        "wave after re-arm",
    );

    assert!(pending.is_empty(), "a token was never claimed");
    contract.assert_quiescent();
}

#[cfg(feature = "threadpool")]
#[test]
fn a_handover_serves_both_the_backlog_and_the_wave_that_follows_it() {
    let file = fixture("delivery-mixed-queue");
    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut contract = RingContract::new();
    let mut pending = Pending::new();

    // Queued before the handover: `EventDelivery::new` attaches the event, so
    // this is the backlog #47 stranded.
    submit_wave(&mut ring, &file, 0, &mut contract, &mut pending);

    let (tx, rx) = mpsc::channel();
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            let _ = tx.send(completion);
        },
        None,
    )
    .expect("wire event delivery to a ring that already has completions queued");

    // Submitted after the handover, without waiting for the backlog to be
    // delivered first -- so the pool sees one queue holding both waves.
    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        queue_wave(&mut batch, &file, 1, &mut contract, &mut pending);
        batch.submit_and_wait(0, 0).expect("submit without waiting");
    }

    // Claim on this thread rather than in the callback, so a delivery that
    // reported the wrong `UserData` fails here instead of passing.
    for delivered in 0..(2 * CHUNKS) {
        let completion = rx.recv_timeout(DELIVERY_TIMEOUT).unwrap_or_else(|_| {
            panic!(
                "only {delivered} of {} completions were delivered -- one attach did not \
                 serve both the backlog and the wave after it",
                2 * CHUNKS
            )
        });
        contract.observe_completion(completion.user_data());
        completion.result().expect("read succeeded");
        let token = pending
            .remove(&completion.user_data())
            .expect("completion matches a held token");
        let _buffer = token
            .claim_if(&completion)
            .expect("a token claims its own completion");
        contract.observe_claim(completion.user_data());
    }

    assert!(pending.is_empty(), "a token was never claimed");
    contract.assert_quiescent();
    drop(delivery);
}

#[test]
fn attaching_while_unbuffered_reads_are_still_in_flight_strands_nothing() {
    // The state D-40 found unreachable with *buffered* reads, reached the way
    // that does work: unbuffered I/O, which the device cannot resolve inside
    // `submit_and_wait`.
    //
    // The precondition is *checked, not assumed*. Counting what is already
    // queued at the instant of attach is what distinguishes this from the
    // already-landed cell, and the test fails if no attempt ever caught a read
    // in flight -- so it cannot quietly decay into a duplicate of
    // `an_attach_serves_both_the_backlog_and_the_wave_that_follows_it` on
    // faster hardware.
    let path = temp_file("unbuffered");
    std::fs::write(&path, vec![0_u8; DIRECT_OPS * DIRECT_LEN]).expect("write fixture file");
    let file = open_unbuffered(&path);
    let handle = file.as_raw_handle();

    let mut caught_in_flight = false;

    for attempt in 0..DIRECT_ATTEMPTS {
        let mut ring = IoRing::new(64, 64).expect("create ring");
        let mut contract = RingContract::new();
        let mut pending: Pending<Aligned> = Pending::new();

        {
            let mut batch = Batch::new(&mut ring);
            for index in 0..DIRECT_OPS {
                let buffer = Aligned::new(DIRECT_LEN);
                let offset = (index * DIRECT_LEN) as u64;
                // SAFETY: `file` outlives every operation queued here -- this
                // attempt drains to completion before the next one starts, and
                // the handle lives for the whole test.
                let token = unsafe { batch.read_raw(handle, buffer, offset, PushOptions::new()) }
                    .expect("queue unbuffered read");
                contract.observe_push(token.id());
                pending.insert(token.id(), token);
            }
            batch.submit_and_wait(0, 0).expect("submit without waiting");
        }

        let event = ring.completion_event().expect(
            "this host must report IORING_FEATURE_SET_COMPLETION_EVENT to run the handover tests",
        );

        // Non-blocking, so this measures what the attach actually found rather
        // than waiting for a state to develop.
        let already_queued = drain_to_empty(&mut ring, &mut contract, &mut pending);
        if already_queued < DIRECT_OPS {
            caught_in_flight = true;
        }

        wait_and_drain(
            &mut ring,
            &event,
            &mut contract,
            &mut pending,
            DIRECT_OPS - already_queued,
            &format!("attempt {attempt}, {already_queued} already queued at attach"),
        );

        assert!(
            pending.is_empty(),
            "attempt {attempt}: a token was never claimed"
        );
        contract.assert_quiescent();
    }

    assert!(
        caught_in_flight,
        "no attempt caught a read in flight: every unbuffered read had already completed by the \
         time the event was attached, so this test degenerated into the already-queued case and \
         is no longer covering what it claims"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

// --- cell (b): re-arming after the pool drains the queue --------------------

#[cfg(feature = "threadpool")]
#[test]
fn a_wave_submitted_after_the_pool_drained_the_queue_is_still_delivered() {
    // `the_edge_re_arms_after_every_drain_to_empty` pins re-arming for a ring
    // the test drains itself. `EventDelivery` drains on a pool thread, and
    // every other delivery test submits exactly one wave, so nothing
    // established that the edge arms again once the *pool* has emptied it.
    let file = fixture("waves");
    let ring = IoRing::new(64, 64).expect("create ring");
    let mut contract = RingContract::new();

    let (tx, rx) = mpsc::channel();
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            let _ = tx.send(completion);
        },
        None,
    )
    .expect("wire event delivery");

    for wave in 0..WAVES {
        let mut pending = Pending::new();
        {
            let mut scope = delivery.scope();
            let mut batch = scope.batch();
            queue_wave(&mut batch, &file, wave, &mut contract, &mut pending);
            batch.submit_and_wait(0, 0).expect("submit without waiting");
        }

        // Draining this wave completely before submitting the next is what
        // makes the next one a re-arm test: the pool empties the queue, so the
        // following wave can only arrive if the edge armed again.
        for delivered in 0..CHUNKS {
            let completion = rx.recv_timeout(DELIVERY_TIMEOUT).unwrap_or_else(|_| {
                panic!(
                    "wave {wave}: only {delivered} of {CHUNKS} completions were delivered -- \
                     the edge did not re-arm after the pool drained the queue"
                )
            });
            contract.observe_completion(completion.user_data());
            completion.result().expect("read succeeded");
            let token = pending
                .remove(&completion.user_data())
                .expect("completion matches a held token");
            let _buffer = token
                .claim_if(&completion)
                .expect("a token claims its own completion");
            contract.observe_claim(completion.user_data());
        }

        assert!(pending.is_empty(), "wave {wave}: a token was never claimed");
    }

    contract.assert_quiescent();
    drop(delivery);
}
