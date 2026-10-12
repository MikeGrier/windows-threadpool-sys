// Copyright (c) Mike Grier
//! The safe submit-fault responder, failing a real `IoRing`'s submits (M37.1).
//!
//! # Why this is an integration test
//!
//! It opens a real ring, an operating-system boundary this repository's
//! Quality rule places in `tests/`. The responder's own rules are
//! `sys::submit_faults`'s unit tests, which open no ring.
//!
//! # What this file is for
//!
//! What the unit tests cannot show: that a failed submit **leaves the entry
//! queued on a real ring**, as `SubmitIoRing` documents (RS-P-7), so a later
//! submit runs it; that the kernel never saw the entry in between; and that
//! every other call still reaches the kernel while the fault is armed.

#![cfg(all(windows, feature = "kernel-seam", feature = "fault-injection"))]

use std::time::Duration;

use windows_ioring_sys::sys::{register, submit_faults};
use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, InjectedFailure, IoRing, IoRingErrorExt, SharedHandle,
};

mod common;

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`, as `InjectedFailure::Win32(1117)`
/// spells it.
const DEVICE: i32 = 0x8007_045D_u32.cast_signed();

#[test]
fn a_failed_submit_leaves_the_entry_queued_and_the_next_submit_runs_it() {
    let path = common::TempPath::new("m37-1", "queued");
    let file = SharedHandle::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .expect("a scratch file")
            .into(),
    );
    let mut ring: IoRing = IoRing::new(8, 16).expect("a ring");
    let (faults, control) = submit_faults();
    // Ended by the ring's drop, after its rundown.
    let _registration = register(&ring, Box::new(faults));
    control.fail_until_cleared(InjectedFailure::Win32(1117));

    let error = {
        let mut batch = Batch::new(&mut ring);
        batch
            .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
            .expect("a flush builds");
        batch
            .submit()
            .expect_err("the armed responder fails the submit")
    };
    assert_eq!(
        error.as_ioring_error().expect("a native report").code(),
        DEVICE,
        "with the code it was armed with"
    );
    assert_eq!(ring.outstanding(), 1, "the entry was built and is owed");
    assert!(
        ring.try_pop()
            .expect("a pop is not a submit, so it still reaches the kernel")
            .is_none(),
        "the kernel never saw the entry, so nothing completed"
    );

    control.clear();
    let submitted = Batch::new(&mut ring)
        .submit()
        .expect("with the fault cleared, the kernel answers");
    assert_eq!(submitted, 1, "the queued entry is what this submit ran");
    let (_completion, held) = ring
        .pop_within(Duration::from_secs(5))
        .expect("the pop is answered")
        .expect("the flush completes once it has been submitted");
    assert!(held.is_some(), "and it is the operation that was queued");
    assert_eq!(ring.outstanding(), 0);
    assert_eq!(
        control.failures(),
        [std::thread::current().id()],
        "exactly one submit failed, on this thread"
    );

    ring.run_down().expect("the ring runs down");
    drop(file);
}
