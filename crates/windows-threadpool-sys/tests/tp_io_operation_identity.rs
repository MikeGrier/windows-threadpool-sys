// Copyright (c) 2026 Mike Grier
//! Integration tests for operation-identity durability on the `TP_IO` backend.
//!
//! `ThreadpoolIo::cancel` is a safe function that acts on an operation named by
//! an [`OperationId`]. Because an operation's storage address returns to the
//! allocator when it is reclaimed, a later operation can be handed that address;
//! the generation stamped at submission is what stops an identity retained past
//! its operation's completion from cancelling whichever operation now occupies
//! it. The triggering pattern -- a timeout firing while a completion is already
//! in flight -- is the ordinary use of cancellation, so these tests cover the
//! race rather than an exotic corner.

#![cfg(windows)]

use std::collections::HashSet;
use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows_overlapped_io_sys::{Issued, Operation, OperationId, Submitted, UnassociatedEndpoint};
use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_OPERATION_ABORTED};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};

/// `PIPE_ACCESS_DUPLEX`. Changing this value is a breaking change.
const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
/// `PIPE_TYPE_BYTE`. Changing this value is a breaking change.
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;

/// How long a test waits for the pool to deliver expected callbacks.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);

// --- harness ---

/// Records completions and lets the test thread wait for a specific number.
struct Recorder {
    seen: Mutex<Vec<(Option<OperationId>, u32)>>,
    arrived: Condvar,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            arrived: Condvar::new(),
        })
    }

    fn push(&self, entry: (Option<OperationId>, u32)) {
        let mut seen = self.seen.lock().expect("record");
        seen.push(entry);
        self.arrived.notify_all();
    }

    fn len(&self) -> usize {
        self.seen.lock().expect("read").len()
    }

    fn records(&self) -> Vec<(Option<OperationId>, u32)> {
        self.seen.lock().expect("read").clone()
    }

    fn wait_for(&self, count: usize) -> Vec<(Option<OperationId>, u32)> {
        let seen = self.seen.lock().expect("await");
        let (seen, timeout) = self
            .arrived
            .wait_timeout_while(seen, CALLBACK_TIMEOUT, |seen| seen.len() < count)
            .expect("await");
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {count} completion(s); saw {}",
            seen.len()
        );
        seen.clone()
    }
}

/// A buffer base pointer shared across submitting threads.
#[derive(Clone, Copy)]
struct SharedSlots(*mut u8);

// SAFETY: the pointer is only offset into disjoint per-operation slots of a
// buffer that outlives all of them; no two operations touch the same byte.
unsafe impl Send for SharedSlots {}
unsafe impl Sync for SharedSlots {}

impl SharedSlots {
    /// The one-byte slot reserved for `index`.
    ///
    /// A method rather than a field access, so a closure captures the `Send`
    /// wrapper instead of the bare pointer under disjoint closure capture.
    ///
    /// # Safety
    ///
    /// `index` must be within the buffer this was built from.
    unsafe fn at(self, index: usize) -> *mut u8 {
        // SAFETY: forwarded from this function's own contract.
        unsafe { self.0.add(index) }
    }
}

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-threadpool-sys-tp-identity-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

fn read_endpoint(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open overlapped endpoint")
}

/// A connected server pipe end carrying no data, so a read on it stays pending
/// until it is cancelled. The client end must be kept alive by the caller.
fn pending_pipe(tag: &str) -> (UnassociatedEndpoint, std::fs::File) {
    // Built from an escaped separator rather than a literal UNC prefix, so the
    // name survives any tooling that mangles adjacent backslashes.
    let sep = '\u{5c}';
    let name = format!(
        "{sep}{sep}.{sep}pipe{sep}windows-threadpool-sys-tp-identity-{tag}-{}",
        std::process::id()
    );
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: creates a fresh overlapped named pipe from a valid wide name.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE,
            1,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    assert!(
        !handle.is_null() && handle as isize != -1,
        "CreateNamedPipeW failed: {}",
        io::Error::last_os_error()
    );

    // SAFETY: fresh, exclusively owned, and opened with FILE_FLAG_OVERLAPPED.
    let endpoint =
        unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(handle)) };
    let client = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect the client pipe end");
    (endpoint, client)
}

/// Assert that the registry refused an identity *before* any native call.
///
/// A registry rejection is constructed in Rust and carries no OS error code,
/// whereas `CancelIoEx` reporting `ERROR_NOT_FOUND` always does. Only the former
/// proves a recycled address was never handed to the kernel.
fn assert_rejected_without_a_native_call(error: &io::Error) {
    assert_eq!(
        error.kind(),
        io::ErrorKind::NotFound,
        "a stale identity must be reported as NotFound"
    );
    assert!(
        error.raw_os_error().is_none(),
        "the identity must be rejected by the registry before CancelIoEx runs, but this error \
         came from the kernel: {error:?}"
    );
}

/// Issue a one-byte overlapped read into `slot`.
///
/// SAFETY: `slot` must point to at least one writable byte that stays valid
/// until the operation completes.
unsafe fn issue_read(
    handle: BorrowedHandle<'_>,
    overlapped: *mut OVERLAPPED,
    slot: *mut u8,
) -> io::Result<Issued> {
    // SAFETY: forwarded from this function's own contract.
    let ok = unsafe { ReadFile(handle.as_raw_handle(), slot, 1, ptr::null_mut(), overlapped) };
    if ok != 0 {
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        return Ok(Issued::Pending);
    }
    Err(error)
}

// --- identities stay distinct across recycled storage ---

/// Submitting and draining repeatedly reuses storage addresses; the identity
/// must differ every time regardless.
#[test]
fn recycled_addresses_still_produce_distinct_identities() {
    const CYCLES: usize = 64;

    let path = temp_file_with(b"tp identity durability", "recycle");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let mut identities = HashSet::new();
    let mut addresses = HashSet::new();
    for cycle in 0..CYCLES {
        let operation = Operation::new(cycle);
        // SAFETY: one 1-byte overlapped ReadFile into `landed`, which outlives
        // every operation because each is drained before the next is submitted.
        let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
        let id = match submitted {
            Submitted::Pending(id) => id,
            other => panic!("cycle {cycle}: expected pending, got {other:?}"),
        };
        assert!(
            identities.insert(id),
            "cycle {cycle} reproduced an earlier identity"
        );
        addresses.insert(id.as_ptr() as usize);

        recorder.wait_for(cycle + 1);
        tp.run_down();
        assert_eq!(tp.outstanding(), 0);
    }

    assert_eq!(identities.len(), CYCLES, "every identity must be unique");

    // Whether the allocator actually reuses an address in any given run depends
    // on it and on whatever else the process is doing, so reuse is reported
    // rather than required -- asserting it here made this test flaky. The reuse
    // case is covered deterministically by
    // `a_stale_generation_at_a_live_address_is_rejected` below, which builds the
    // exact identity a reused address would produce.
    if addresses.len() == CYCLES {
        eprintln!(
            "note: no storage address was reused across {CYCLES} cycles in this run, so this \
             test exercised only identity uniqueness"
        );
    }

    // Every completion must have reported the identity its submission returned.
    for (reported, _) in recorder.records() {
        let reported = reported.expect("an operation completion carries an identity");
        assert!(
            identities.contains(&reported),
            "a completion reported an identity that was never issued"
        );
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

// --- the hazard: a retained identity must not cancel a later operation ---

/// A retained identity whose operation has completed must be rejected, even once
/// its address has been handed to a live operation.
#[test]
fn a_retained_identity_cannot_cancel_the_operation_that_recycled_its_address() {
    const CYCLES: usize = 64;

    let path = temp_file_with(b"tp identity durability", "stale-cancel");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let mut stale: Vec<OperationId> = Vec::new();
    let mut completed = 0_usize;
    let mut collided = None;

    for cycle in 0..CYCLES {
        // A short-lived operation, drained so its storage is freed.
        let operation = Operation::new(cycle);
        // SAFETY: one 1-byte overlapped ReadFile into `landed`, drained below.
        let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
        let dead = match submitted {
            Submitted::Pending(id) => id,
            other => panic!("cycle {cycle}: expected pending, got {other:?}"),
        };
        completed += 1;
        recorder.wait_for(completed);
        tp.run_down();
        stale.push(dead);

        // A second operation, which may be given the freed storage.
        let operation = Operation::new(1000 + cycle);
        // SAFETY: one 1-byte overlapped ReadFile into `landed`, drained below.
        let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
        let live = match submitted {
            Submitted::Pending(id) => id,
            other => panic!("cycle {cycle}: expected pending, got {other:?}"),
        };

        if let Some(dead) = stale
            .iter()
            .find(|dead| dead.as_ptr() == live.as_ptr())
            .copied()
        {
            assert_ne!(
                dead.generation(),
                live.generation(),
                "the recycled address must carry a new generation"
            );

            let rejected = tp
                .cancel(dead)
                .expect_err("a stale identity must not be accepted for cancellation");
            assert_rejected_without_a_native_call(&rejected);
            collided = Some((dead, live));
        }

        completed += 1;
        let records = recorder.wait_for(completed);
        tp.run_down();
        stale.push(live);

        if collided.is_some() {
            // The live operation must have completed normally rather than as
            // ERROR_OPERATION_ABORTED, proving the stale cancel never reached it.
            let (reported, result) = records[completed - 1];
            assert_eq!(reported, Some(live), "the wrong operation completed");
            assert_ne!(
                result, ERROR_OPERATION_ABORTED,
                "a stale identity cancelled the operation that recycled its address"
            );
            break;
        }
    }

    // Natural address reuse is opportunistic, so its absence is reported rather
    // than failed -- requiring it here made this test flaky. The hazard itself is
    // covered on every run by `a_stale_generation_at_a_live_address_is_rejected`.
    match collided {
        Some((dead, live)) => assert_eq!(
            dead.as_ptr(),
            live.as_ptr(),
            "the collision must be a genuine address reuse"
        ),
        None => eprintln!(
            "note: no storage address was reused across {CYCLES} cycles in this run, so this \
             test observed no natural collision"
        ),
    }
    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// The recycled-address hazard, synthesized deterministically.
///
/// An identity carrying an *older* generation at an address that is currently
/// live is exactly the value a retained identity would have after the allocator
/// reissued its storage. Forging it with `OperationId::forge`
/// removes the dependence on the allocator actually reusing an address, so this
/// covers the hazard on every run rather than opportunistically.
#[test]
fn a_stale_generation_at_a_live_address_is_rejected() {
    let (endpoint, _client) = pending_pipe("synthetic-aba");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        endpoint,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(11_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let live = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };
    assert_eq!(tp.outstanding(), 1);

    // The identity a previous operation at this same storage would have had.
    // SAFETY: deliberately forging an identity the registry never issued, in
    // order to assert that it is refused. This is the case orge exists for.
    let stale = unsafe { OperationId::forge(live.as_ptr(), live.generation() - 1) };
    assert_eq!(
        stale.as_ptr(),
        live.as_ptr(),
        "same address by construction"
    );
    assert_ne!(stale, live, "different generation by construction");

    let rejected = tp
        .cancel(stale)
        .expect_err("a stale generation must not cancel the live operation");
    assert_rejected_without_a_native_call(&rejected);

    // The live operation must be untouched, and still cancellable by its own
    // identity -- the rejection must not have disturbed it.
    assert_eq!(tp.outstanding(), 1, "the live operation must survive");
    assert_eq!(recorder.len(), 0, "nothing may have completed yet");

    tp.cancel(live).expect("the live identity must still work");
    let records = recorder.wait_for(1);
    assert_eq!(records[0].0, Some(live));
    assert_eq!(records[0].1, ERROR_OPERATION_ABORTED);

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
}

/// A *newer* generation than the live one is also rejected, so the check is an
/// equality test rather than an ordering test.
#[test]
fn a_future_generation_at_a_live_address_is_rejected() {
    let (endpoint, _client) = pending_pipe("future-generation");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        endpoint,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(12_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data.
    let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let live = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };

    // SAFETY: as above -- forged so that its rejection can be asserted.
    let ahead = unsafe { OperationId::forge(live.as_ptr(), live.generation() + 1) };
    let rejected = tp
        .cancel(ahead)
        .expect_err("a generation that was never issued must be rejected");
    assert_rejected_without_a_native_call(&rejected);
    assert_eq!(tp.outstanding(), 1);

    tp.cancel_all().expect("cancel the live operation");
    recorder.wait_for(1);
    tp.run_down();
}

/// An identity whose operation has completed is rejected even when nothing has
/// taken its address.
#[test]
fn a_completed_operations_identity_is_rejected() {
    let path = temp_file_with(b"tp identity durability", "completed");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(1_usize);
    // SAFETY: one 1-byte overlapped ReadFile into `landed`, drained below.
    let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };

    recorder.wait_for(1);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);

    let rejected = tp
        .cancel(id)
        .expect_err("a completed operation's identity must be rejected");
    assert_rejected_without_a_native_call(&rejected);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// An identity minted by one object must not be honored by another.
#[test]
fn an_identity_from_another_object_is_rejected() {
    let (pipe_a, _client_a) = pending_pipe("cross-a");
    let (pipe_b, _client_b) = pending_pipe("cross-b");

    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);
    let tp_a = ThreadpoolIo::new(
        pipe_a,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO a");
    let tp_b = ThreadpoolIo::new(pipe_b, |_| {}, None).expect("create TP_IO b");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(1_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted = unsafe { tp_a.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let id_a = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };
    assert_eq!(tp_a.outstanding(), 1);

    // Object B has never seen this identity, so it must refuse to act on it.
    let rejected = tp_b
        .cancel(id_a)
        .expect_err("an identity from another object must be rejected");
    assert_rejected_without_a_native_call(&rejected);
    assert_eq!(tp_a.outstanding(), 1, "object A's operation must survive");

    // Object A owns it, so its own identity still works.
    tp_a.cancel(id_a).expect("the owning object must cancel");
    let records = recorder.wait_for(1);
    assert_eq!(records[0].0, Some(id_a));
    assert_eq!(records[0].1, ERROR_OPERATION_ABORTED);

    tp_a.run_down();
    assert_eq!(tp_a.outstanding(), 0);
}

// --- identities remain usable for their intended purposes ---

/// A live identity really does cancel its own operation -- rejecting stale
/// identities must not have made cancellation useless.
#[test]
fn a_live_identity_still_cancels_a_genuinely_pending_operation() {
    let (pipe, _client) = pending_pipe("live-cancel");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        pipe,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(9_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };
    assert_eq!(tp.outstanding(), 1);

    tp.cancel(id)
        .expect("a live identity must cancel its own operation");

    let records = recorder.wait_for(1);
    assert_eq!(
        records[0].0,
        Some(id),
        "the completion must report the identity"
    );
    assert_eq!(records[0].1, ERROR_OPERATION_ABORTED);

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
}

/// Cancelling the same live operation twice is harmless: the second attempt is
/// rejected once the first has completed, never redirected elsewhere.
#[test]
fn cancelling_twice_rejects_the_second_attempt() {
    let (pipe, _client) = pending_pipe("double-cancel");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        pipe,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(3_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };

    tp.cancel(id).expect("the first cancel must be accepted");
    recorder.wait_for(1);
    tp.run_down();

    let rejected = tp
        .cancel(id)
        .expect_err("the second cancel must be rejected once the operation is gone");
    assert_rejected_without_a_native_call(&rejected);
    assert_eq!(recorder.len(), 1, "the operation must complete only once");
}

/// Identities survive being moved to another thread, which is what makes the
/// timeout-cancels-operation pattern possible at all.
#[test]
fn an_identity_can_be_cancelled_from_another_thread() {
    let (pipe, _client) = pending_pipe("cross-thread");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        pipe,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(5_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };

    let cancelled = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let tp = &tp;
        let cancelled = &cancelled;
        scope.spawn(move || {
            // The identity crosses the thread boundary by value.
            tp.cancel(id).expect("cancel from another thread");
            cancelled.fetch_add(1, Ordering::SeqCst);
        });
    });
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);

    let records = recorder.wait_for(1);
    assert_eq!(records[0].0, Some(id));
    assert_eq!(records[0].1, ERROR_OPERATION_ABORTED);

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
}

/// Submitting from many threads while completions are being reclaimed must not
/// let a freed storage address be re-registered while its old entry survives.
///
/// This is the regression test for a real race: the backend originally
/// deregistered an operation *after* running the callback, so storage freed by
/// `claim` inside the callback could be handed to a concurrent submission while
/// the completed operation was still registered. Rapid file reads that complete
/// as fast as they are issued, submitted from several threads, are what surface
/// it -- it reproduced within a few runs.
#[test]
fn concurrent_submission_and_reclamation_never_double_registers_an_address() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 64;
    const OPERATIONS: usize = THREADS * PER_THREAD;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "concurrent-recycle");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // Claiming frees the storage inside this callback, which is exactly
            // what made the address available for reuse too early.
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = SharedSlots(landed.as_mut_ptr());

    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let tp = &tp;
            scope.spawn(move || {
                for step in 0..PER_THREAD {
                    let slot = thread * PER_THREAD + step;
                    let mut operation = Operation::new(slot);
                    operation.set_offset(slot as u64);
                    // SAFETY: one 1-byte overlapped ReadFile into this slot's own
                    // byte; slots are disjoint across threads and `landed`
                    // outlives every operation.
                    let submitted =
                        unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, base.at(slot))) };
                    assert!(
                        matches!(submitted, Submitted::Pending(_)),
                        "slot {slot}: expected pending, got {submitted:?}"
                    );
                }
            });
        }
    });

    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(records.len(), OPERATIONS, "every operation must call back");

    let mut identities = HashSet::new();
    for (reported, result) in &records {
        let reported = reported.expect("an operation completion carries an identity");
        assert!(identities.insert(reported), "an identity completed twice");
        assert_eq!(*result, 0, "a read failed");
    }
    assert_eq!(identities.len(), OPERATIONS);

    for (slot, byte) in landed.iter().enumerate() {
        assert_eq!(*byte, slot as u8, "slot {slot} landed the wrong byte");
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// Identities of simultaneously outstanding operations are distinct, and each
/// completion reports its own.
#[test]
fn simultaneous_identities_match_their_own_completions() {
    const OPERATIONS: usize = 128;

    let (pipe, _client) = pending_pipe("simultaneous");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        pipe,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<usize>() };
            seen.push((completion.id(), completion.io_result()));
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    let mut issued = HashSet::new();
    for slot in 0..OPERATIONS {
        let operation = Operation::new(slot);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte on a
        // pipe carrying no data; `landed` outlives every operation.
        let submitted = unsafe { tp.submit(operation, |h, ov| issue_read(h, ov, base.add(slot))) };
        match submitted {
            Submitted::Pending(id) => {
                assert!(issued.insert(id), "slot {slot} reused a live identity");
            }
            other => panic!("expected pending at slot {slot}, got {other:?}"),
        }
    }
    assert_eq!(tp.outstanding(), OPERATIONS);

    tp.cancel_all().expect("cancel every outstanding read");
    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();

    let mut matched = HashSet::new();
    for (reported, _) in &records {
        let reported = reported.expect("an operation completion carries an identity");
        assert!(
            issued.contains(&reported),
            "a completion reported an identity that was never issued"
        );
        assert!(matched.insert(reported), "an identity completed twice");
    }
    assert_eq!(matched.len(), OPERATIONS);
}
