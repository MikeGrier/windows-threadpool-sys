// Copyright (c) Mike Grier
//! Tests for registering a responder for one ring (M35.1, D-85).
//!
//! # What these establish
//!
//! The dispatch, each property in both directions: a registration answers its
//! own ring and no other, on any thread; it ends when it is ended or its ring
//! drops, and not when its handle drops; and a responder is not consulted
//! re-entrantly for its own ring, nor wedged by its own panic.
//!
//! # What they deliberately do not do
//!
//! **Open a ring**: every ring here is a bare `RingId`, and every call through
//! the seam reaches a responder that answers without forwarding, so nothing
//! reaches Win32. That keeps them out of the population
//! `tools/check-ring-tests.ps1` tracks (D-49). That a real
//! ring's calls -- its submits on another thread, its rundown as it drops --
//! reach its registration is `tests/responder_per_ring.rs`'s.

use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::S_OK;
use windows_sys::core::HRESULT;

use super::{Responses, register_id, ring_dropped, with};
use crate::accounting::RingId;
use crate::sys::{Resolver, ResolverWatch};

/// What a ring's responder answers `submit` with: distinct from `S_OK`, which
/// the re-entrancy test's responder answers with.
const RING_ANSWER: HRESULT = 0x8007_045D_u32 as HRESULT;

/// Answers `submit` with a fixed code and counts its calls, forwarding
/// nothing.
struct Reporting {
    answer: HRESULT,
    seen: Arc<AtomicUsize>,
}

impl Responses for Reporting {
    unsafe fn submit(
        &mut self,
        _ring: *mut c_void,
        _wait_operations: u32,
        _milliseconds: u32,
        _submitted: *mut u32,
    ) -> HRESULT {
        self.seen.fetch_add(1, Ordering::Relaxed);
        self.answer
    }
}

/// A `Reporting` answering `answer`, and the count of its calls.
fn reporting(answer: HRESULT) -> (Box<Reporting>, Arc<AtomicUsize>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let responder = Box::new(Reporting {
        answer,
        seen: Arc::clone(&seen),
    });
    (responder, seen)
}

/// Call the seam's `submit` for `ring` with a null handle.
///
/// # Safety
///
/// Only sound while `ring` has a responder that answers `submit` **without
/// forwarding**; with none, a null handle reaches
/// `SubmitIoRing`.
unsafe fn submit(ring: RingId) -> HRESULT {
    let mut submitted = 0_u32;
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::sys::submit(ring, std::ptr::null_mut(), 0, 0, &raw mut submitted) }
}

/// Whether `ring` has a registered responder, asked at the dispatch.
fn registered(ring: RingId) -> bool {
    with(ring, |_| ()).is_some()
}

#[test]
fn a_registration_answers_its_own_ring_and_no_other() {
    let (ours, other) = (RingId::next(), RingId::next());
    let (responder, seen) = reporting(RING_ANSWER);
    let registration = register_id(ours, responder);

    // SAFETY: `ours` has a responder that does not forward.
    assert_eq!(
        unsafe { submit(ours) },
        RING_ANSWER,
        "its own ring is answered"
    );
    assert_eq!(seen.load(Ordering::Relaxed), 1);
    assert!(!registered(other), "another ring is not");
    assert_eq!(seen.load(Ordering::Relaxed), 1, "nor consulted for it");

    assert!(registration.end().is_some());
}

#[test]
fn a_registration_answers_its_ring_on_any_thread() {
    // The property the thread-local form lacks, and the reason D-85 exists:
    // a ring used from a pool thread is answered there too.
    let ring = RingId::next();
    let (responder, seen) = reporting(RING_ANSWER);
    let registration = register_id(ring, responder);

    // SAFETY: `ring` has a responder that does not forward, on every thread.
    let elsewhere = std::thread::spawn(move || unsafe { submit(ring) })
        .join()
        .expect("the other thread does not panic");

    assert_eq!(elsewhere, RING_ANSWER);
    assert_eq!(seen.load(Ordering::Relaxed), 1);
    assert!(registration.end().is_some());
}

#[test]
fn ending_hands_the_responder_back_and_the_ring_falls_through() {
    let ring = RingId::next();
    let (responder, seen) = reporting(RING_ANSWER);
    let registration = register_id(ring, responder);

    let mut handed_back = registration.end().expect("it had not ended");
    assert!(!registered(ring), "the ring's calls reach the next layer");

    // The responder handed back is the one registered.
    let mut submitted = 0_u32;
    // SAFETY: called directly, not through the seam; `Reporting` ignores its
    // pointers.
    let answer = unsafe { handed_back.submit(std::ptr::null_mut(), 0, 0, &raw mut submitted) };
    assert_eq!(answer, RING_ANSWER);
    assert_eq!(seen.load(Ordering::Relaxed), 1);
}

#[test]
fn a_dropped_ring_ends_its_registration() {
    let ring = RingId::next();
    let (responder, _) = reporting(RING_ANSWER);
    let registration = register_id(ring, responder);
    assert!(registered(ring));

    ring_dropped(ring);

    assert!(!registered(ring), "the ring's drop ended it");
    assert!(
        registration.end().is_none(),
        "so there is nothing left to end"
    );
}

#[test]
fn dropping_a_registration_ends_nothing() {
    // Ending on drop would, in the ordinary `let ring; let registration;`
    // shape, end it before the ring's rundown: locals drop in reverse order.
    let ring = RingId::next();
    let (responder, _) = reporting(RING_ANSWER);
    // `Registration` has no `Drop` at all, which is the point; this discards
    // it at once.
    let _ = register_id(ring, responder);

    assert!(registered(ring), "only the ring's drop, or `end`, ends it");
    ring_dropped(ring);
    assert!(!registered(ring));
}

#[test]
#[should_panic(expected = "already registered")]
fn registering_twice_for_one_ring_is_refused() {
    let ring = RingId::next();
    let (first, _) = reporting(RING_ANSWER);
    let (second, _) = reporting(RING_ANSWER);
    let _first = register_id(ring, first);
    let _second = register_id(ring, second);
}

#[test]
fn a_responder_is_not_consulted_re_entrantly_for_its_own_ring() {
    /// From inside its own `submit`, asks the seam for its own ring and for
    /// another.
    struct Reentrant {
        own: RingId,
        other: RingId,
        checked: Arc<AtomicUsize>,
    }

    impl Responses for Reentrant {
        unsafe fn submit(
            &mut self,
            _ring: *mut c_void,
            _wait: u32,
            _ms: u32,
            _submitted: *mut u32,
        ) -> HRESULT {
            // Without the guard this would block forever on its own slot.
            assert!(
                !registered(self.own),
                "a nested call for its own ring must reach the kernel"
            );
            assert!(
                registered(self.other),
                "while another ring's responder is still consulted"
            );
            self.checked.fetch_add(1, Ordering::Relaxed);
            S_OK
        }
    }

    let (own, other) = (RingId::next(), RingId::next());
    let checked = Arc::new(AtomicUsize::new(0));
    let (other_responder, _) = reporting(RING_ANSWER);
    let other_registration = register_id(other, other_responder);
    let registration = register_id(
        own,
        Box::new(Reentrant {
            own,
            other,
            checked: Arc::clone(&checked),
        }),
    );

    // SAFETY: `own` has a responder that does not forward.
    assert_eq!(unsafe { submit(own) }, S_OK);
    assert_eq!(checked.load(Ordering::Relaxed), 1);
    assert!(
        registered(own),
        "and once it returns, it is consulted again"
    );

    assert!(registration.end().is_some());
    assert!(other_registration.end().is_some());
}

#[test]
fn a_responder_that_panics_does_not_wedge_its_ring() {
    /// Panics on its first `submit`, answers after.
    struct PanicsOnce {
        calls: usize,
    }

    impl Responses for PanicsOnce {
        unsafe fn submit(
            &mut self,
            _ring: *mut c_void,
            _wait: u32,
            _ms: u32,
            _submitted: *mut u32,
        ) -> HRESULT {
            self.calls += 1;
            assert!(self.calls > 1, "the responder panics");
            RING_ANSWER
        }
    }

    let ring = RingId::next();
    let registration = register_id(ring, Box::new(PanicsOnce { calls: 0 }));

    // SAFETY: `ring` has a responder that does not forward.
    let first = panic::catch_unwind(AssertUnwindSafe(|| unsafe { submit(ring) }));
    assert!(first.is_err(), "the panic reaches the caller");

    // Its lock is poisoned and this thread was marked as answering for it;
    // neither may leave the ring unanswerable.
    // SAFETY: as above.
    assert_eq!(unsafe { submit(ring) }, RING_ANSWER);
    assert!(registration.end().is_some());
}

#[test]
fn the_resolver_and_its_watch_can_follow_a_ring_across_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<ResolverWatch>();
    // Compiles only while `Resolver` is `Send`, which registering requires.
    let resolver: Box<dyn Responses + Send> = Box::new(Resolver::new(0x35));
    drop(resolver);
}
