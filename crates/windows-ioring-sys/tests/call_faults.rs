// Copyright (c) Mike Grier
//! The safe call-fault responder, failing and stalling a real `IoRing` (M38.1).
//!
//! # Why this is an integration test
//!
//! It opens real rings, an operating-system boundary this repository's Quality rule places in
//! `tests/`. The responder's own rules are `sys::call_faults`'s unit tests, which open no ring.
//!
//! # What this file is for
//!
//! What the unit tests cannot show about a real ring: that a refused `Build*` call leaves nothing
//! queued and a retry works; that a failed completion keeps its identity, so the ring's own
//! accounting still balances; that a stalled submit leaves an operation unrun, a bounded wait for
//! it expires, and the next submit after the stall runs it; and that a ring dropped mid-stall runs
//! down once it ends rather than hanging.

#![cfg(all(windows, feature = "kernel-seam", feature = "fault-injection"))]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_ioring_sys::sys::{Call, call_faults, register};
use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, InjectedFailure, IoRing, IoRingErrorExt, PushOptions,
    SharedHandle, WriteCaching,
};

mod common;

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`, as `InjectedFailure::Win32(1117)` spells it.
const DEVICE: i32 = 0x8007_045D_u32.cast_signed();
const BOUND: Duration = Duration::from_secs(5);

fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

fn scratch(tag: &str) -> (common::TempPath, SharedHandle) {
    let path = common::TempPath::new("m38-1", tag);
    let file = SharedHandle::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .expect("a scratch file")
            .into(),
    );
    (path, file)
}

/// The `HRESULT` a native report carries.
fn code(error: &std::io::Error) -> i32 {
    error.as_ioring_error().expect("a native report").code()
}

#[test]
fn a_refused_file_registration_build_is_reported_and_a_retry_registers() {
    let (_path, file) = scratch("reg-files");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.fail_next(Call::BuildRegisterFiles, 1, device());

    let refused = Batch::new(&mut ring)
        .register_shared_files(vec![file.clone()])
        .expect_err("the build is refused");
    assert_eq!(code(&refused.error), DEVICE);
    assert_eq!(refused.payload.expect("the files are handed back").len(), 1);
    assert_eq!(ring.outstanding(), 0, "a refused build queued nothing");

    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_shared_files(vec![file])
        .expect("the retry builds");
    batch.submit().expect("the registration is submitted");
    let (completion, _) = ring
        .pop_within(BOUND)
        .expect("the pop is answered")
        .expect("the registration completes");
    pending
        .claim_if(&completion)
        .expect("its own completion")
        .expect("and the kernel registered the file");
    ring.run_down().expect("the ring runs down");
}

#[test]
fn a_refused_buffer_registration_build_hands_the_buffers_back() {
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.fail_next(Call::BuildRegisterBuffers, 1, device());

    let refused = Batch::new(&mut ring)
        .register_buffers(vec![vec![7_u8; 16]])
        .expect_err("the build is refused");
    assert_eq!(code(&refused.error), DEVICE);
    assert_eq!(
        refused.payload.expect("the buffers are handed back"),
        [vec![7_u8; 16]],
        "intact: the kernel was never given them"
    );
}

#[test]
fn a_failed_completion_keeps_its_identity_and_the_rings_accounting_balances() {
    let (_path, file) = scratch("completions");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.fail_next_completions(1, device());
    // An empty pop is not a completion, so it spends nothing: the failure waits for a real one.
    assert!(ring.try_pop().expect("an empty pop").is_none());
    control.clear();

    {
        let mut batch = Batch::new(&mut ring);
        for _ in 0..3 {
            batch
                .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
                .expect("a flush builds");
        }
        batch.submit().expect("all are submitted");
    }
    // The last of three in flight, so zeroing its identity would name an operation already
    // retired rather than the one it is.
    let mut results = Vec::new();
    for i in 0..3 {
        if i == 2 {
            control.fail_next_completions(1, device());
        }
        let (completion, held) = ring
            .pop_within(BOUND)
            .expect("the pop is answered")
            .expect("a completion arrives");
        assert!(
            held.is_some(),
            "the identity is the operation's own, so the ring found what it held"
        );
        results.push(completion.result());
    }
    assert!(
        results[..2].iter().all(Result::is_ok),
        "the first two are the kernel's: {results:?}"
    );
    let failed = results[2].as_ref().expect_err("the third was failed");
    assert_eq!(code(failed), DEVICE);
    assert_eq!(ring.outstanding(), 0, "and all retired");
    ring.run_down().expect("the ring runs down");
}

#[test]
fn clearing_ends_a_failed_completion_plan() {
    let (_path, file) = scratch("completions-cleared");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.fail_next_completions(5, device());
    control.clear();

    let mut batch = Batch::new(&mut ring);
    batch
        .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
        .expect("a flush builds");
    batch.submit().expect("submitted");
    let (completion, _) = ring
        .pop_within(BOUND)
        .expect("the pop is answered")
        .expect("a completion arrives");
    completion.result().expect("cleared, so it is the kernel's");
    ring.run_down().expect("the ring runs down");
}

#[test]
fn a_failed_pop_call_is_reported_once_and_the_next_reaches_the_kernel() {
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.fail_next(Call::Pop, 1, device());

    let error = ring.try_pop().expect_err("the pop call fails");
    assert_eq!(code(&error), DEVICE);
    assert!(
        ring.try_pop()
            .expect("the next reaches the kernel")
            .is_none()
    );
}

#[test]
fn a_stalled_submit_leaves_the_operation_unrun_until_the_stall_ends() {
    let (path, file) = scratch("stall");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.stall_submits(Duration::from_millis(500));

    let submitted = {
        let mut batch = Batch::new(&mut ring);
        batch
            .write_owned(
                &file,
                vec![b'x'; 4],
                (),
                0,
                PushOptions::new(),
                WriteCaching::Cached,
            )
            .expect("a write builds");
        batch.submit().expect("a stalled submit still succeeds")
    };
    assert_eq!(submitted, 0, "but submitted nothing");
    assert!(
        std::fs::read(&path).expect("read").is_empty(),
        "so the write has not run"
    );
    assert!(ring.try_pop().expect("pop").is_none());
    assert_eq!(ring.outstanding(), 1, "though the ring is owed it");

    // A bounded wait for it expires, as it would for a kernel that was taking its time.
    assert!(
        ring.pop_within(Duration::from_millis(60))
            .expect("an expired wait is not an error")
            .is_none()
    );
    assert!(std::fs::read(&path).expect("read").is_empty());

    std::thread::sleep(Duration::from_millis(500));
    let submitted = Batch::new(&mut ring)
        .submit()
        .expect("after the stall the kernel answers");
    assert_eq!(submitted, 1, "and runs what was queued");
    let (completion, held) = ring
        .pop_within(BOUND)
        .expect("the pop is answered")
        .expect("the write completes");
    assert_eq!(completion.result().expect("it wrote"), 4);
    assert!(held.is_some());
    assert_eq!(std::fs::read(&path).expect("read"), b"xxxx");
    ring.run_down().expect("the ring runs down");
}

#[test]
fn a_ring_dropped_during_a_stall_runs_down_once_it_ends() {
    let (_path, file) = scratch("drop");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 16).expect("a ring");
    let (faults, control) = call_faults();
    let _registration = register(&ring, Box::new(faults));
    control.stall_submits(Duration::from_millis(300));
    {
        let mut batch = Batch::new(&mut ring);
        batch
            .write_owned(
                &file,
                vec![b'y'; 4],
                (),
                0,
                PushOptions::new(),
                WriteCaching::Cached,
            )
            .expect("a write builds");
        batch.submit().expect("stalled");
    }

    // Dropped on another thread, so a hang is a failed test rather than a stuck one.
    let (done, finished) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        drop(ring);
        let _ = done.send(());
    });
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the ring ran down once the stall ended, rather than hanging");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "and only after it: {:?}",
        started.elapsed()
    );
}
