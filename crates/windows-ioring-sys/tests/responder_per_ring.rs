// Copyright (c) Mike Grier
//! A responder registered for one real `IoRing` (M35.1,
//! [D-85](../DESIGN-NOTES.md#d-85)).
//!
//! # Why this is an integration test
//!
//! It opens real rings, an operating-system boundary, which this repository's
//! Quality rule places in `tests/` -- and `check-ring-tests.ps1` keeps that
//! population out of the lib suite. The registry's own rules are
//! `sys::registered`'s unit tests, which open no ring.
//!
//! # What this file is for
//!
//! **Reachability through a real ring**, for the two things the unit tests
//! cannot show: that a ring's own calls -- made by the crate's `Batch` and its
//! drop, not by a test calling the seam -- reach its registration, and that
//! they do so from a thread other than the one that registered it, and
//! through the ring's rundown as it drops.

#![cfg(all(windows, feature = "kernel-seam"))]

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_ioring_sys::sys::{Responses, register};
use windows_ioring_sys::{Batch, FlushCoverage, FlushMode, IoRing, IoRingErrorExt, SharedHandle};
use windows_sys::Win32::Storage::FileSystem::{IORING_CQE, PopIoRingCompletion, SubmitIoRing};
use windows_sys::core::HRESULT;

mod common;

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`: what the failing responder answers,
/// and nothing the kernel answers an empty submit with.
const RING_ANSWER: HRESULT = 0x8007_045D_u32 as HRESULT;

/// Fails every submit, counting them; every other call reaches the kernel.
struct FailsSubmit {
    seen: Arc<AtomicUsize>,
}

impl Responses for FailsSubmit {
    unsafe fn submit(
        &mut self,
        _ring: *mut c_void,
        _wait_operations: u32,
        _milliseconds: u32,
        _submitted: *mut u32,
    ) -> HRESULT {
        self.seen.fetch_add(1, Ordering::Relaxed);
        RING_ANSWER
    }
}

/// Forwards every submit and pop to the kernel, counting them.
struct Counting {
    submits: Arc<AtomicUsize>,
    pops: Arc<AtomicUsize>,
}

impl Responses for Counting {
    unsafe fn submit(
        &mut self,
        ring: *mut c_void,
        wait_operations: u32,
        milliseconds: u32,
        submitted: *mut u32,
    ) -> HRESULT {
        self.submits.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded unchanged from the seam's caller.
        unsafe { SubmitIoRing(ring, wait_operations, milliseconds, submitted) }
    }

    unsafe fn pop(&mut self, ring: *mut c_void, cqe: *mut IORING_CQE) -> HRESULT {
        self.pops.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as `submit`.
        unsafe { PopIoRingCompletion(ring, cqe) }
    }
}

fn failing() -> (Box<FailsSubmit>, Arc<AtomicUsize>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let responder = Box::new(FailsSubmit {
        seen: Arc::clone(&seen),
    });
    (responder, seen)
}

fn ring() -> IoRing {
    IoRing::new(8, 16).expect("a ring")
}

/// Submit nothing on `ring`: the kernel accepts that, so only a responder
/// makes it fail.
fn submit_nothing(ring: &mut IoRing) -> std::io::Result<u32> {
    Batch::new(ring).submit()
}

/// The `HRESULT` a failed submit reported.
fn code(error: &std::io::Error) -> HRESULT {
    error.as_ioring_error().expect("a native report").code()
}

#[test]
fn a_registration_answers_its_ring_on_a_thread_other_than_the_one_that_registered_it() {
    let mut ring = ring();
    let (responder, seen) = failing();
    let registration = register(&ring, responder);

    // The case D-85 exists for: the ring is used where nothing is installed.
    let (mut ring, outcome) = std::thread::spawn(move || {
        let outcome = submit_nothing(&mut ring);
        (ring, outcome)
    })
    .join()
    .expect("the other thread does not panic");

    let error = outcome.expect_err("the registered responder fails the submit");
    assert_eq!(code(&error), RING_ANSWER);
    assert_eq!(seen.load(Ordering::Relaxed), 1);

    assert!(registration.end().is_some());
    submit_nothing(&mut ring).expect("once it ends, the kernel answers");
    assert_eq!(
        seen.load(Ordering::Relaxed),
        1,
        "and the responder is not asked"
    );
}

#[test]
fn a_registration_answers_its_own_ring_and_not_another() {
    let (mut ours, mut other) = (ring(), ring());
    let (responder, seen) = failing();
    let registration = register(&ours, responder);

    submit_nothing(&mut other).expect("another ring reaches the kernel");
    assert_eq!(seen.load(Ordering::Relaxed), 0);
    let error = submit_nothing(&mut ours).expect_err("its own ring is answered");
    assert_eq!(code(&error), RING_ANSWER);
    assert_eq!(seen.load(Ordering::Relaxed), 1);

    assert!(registration.end().is_some());
}

#[test]
fn a_registration_answers_its_rings_rundown_and_then_ends() {
    // The guarantee `Resolver::scoped` had from being installed before its
    // ring existed, now structural: the ring's drop runs down through its
    // responder, then ends the registration.
    let path = common::TempPath::new("m35-1", "rundown");
    let file = SharedHandle::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .expect("a scratch file")
            .into(),
    );
    let mut ring = ring();
    let submits = Arc::new(AtomicUsize::new(0));
    let pops = Arc::new(AtomicUsize::new(0));
    let registration = register(
        &ring,
        Box::new(Counting {
            submits: Arc::clone(&submits),
            pops: Arc::clone(&pops),
        }),
    );

    {
        let mut batch = Batch::new(&mut ring);
        batch
            .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
            .expect("a flush builds");
        batch.submit().expect("the flush is submitted");
    }
    assert_eq!(ring.outstanding(), 1, "left for the rundown");
    assert_eq!(submits.load(Ordering::Relaxed), 1);
    assert_eq!(pops.load(Ordering::Relaxed), 0, "nothing popped yet");

    drop(ring);

    assert!(
        pops.load(Ordering::Relaxed) > 0,
        "the rundown popped through the registered responder"
    );
    assert!(
        registration.end().is_none(),
        "and the ring's drop ended the registration"
    );
}

#[test]
fn a_dropped_rings_registration_does_not_answer_a_later_ring() {
    // Keyed by `RingId`, never reused, rather than by a handle the kernel may
    // hand out again once the first ring closes.
    let first = ring();
    let (responder, seen) = failing();
    let registration = register(&first, responder);
    drop(first);

    let mut later = ring();
    submit_nothing(&mut later).expect("a later ring reaches the kernel");
    assert_eq!(seen.load(Ordering::Relaxed), 0);
    assert!(registration.end().is_none());
}
