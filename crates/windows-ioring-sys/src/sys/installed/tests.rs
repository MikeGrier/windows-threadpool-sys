// Copyright (c) Mike Grier
//! Tests for the seam's install point (M26.2).
//!
//! # What these establish
//!
//! That the indirection is **transparent** with nothing installed, that an
//! installed responder is **actually consulted**, and that it is **scoped to
//! one thread and one guard**. Those are the three properties `M26.3`'s
//! resolver will rest on, and each of them can silently stop holding.
//!
//! # What they deliberately do not do
//!
//! **Open a ring.** Every test here either asks the install point directly or
//! passes a null ring handle to a responder that never forwards, so nothing
//! reaches Win32. That keeps them out of the ring-opening population
//! `tools/check-ring-tests.ps1` tracks (D-49), and it is sound because what is
//! under test is the *dispatch*, not the calls. The forwarding path is
//! exercised by every other test in the crate, all of which run with no
//! responder installed.

use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{E_FAIL, S_OK};
use windows_sys::core::HRESULT;

use super::{Responses, install};

/// Answers `submit` with a fixed code and counts its calls, forwarding
/// nothing.
struct Reporting {
    answer: HRESULT,
    seen: Rc<Cell<usize>>,
}

impl Responses for Reporting {
    unsafe fn submit(
        &mut self,
        _ring: *mut c_void,
        _wait_operations: u32,
        _milliseconds: u32,
        _submitted: *mut u32,
    ) -> HRESULT {
        self.seen.set(self.seen.get() + 1);
        self.answer
    }
}

/// Call the seam once with arguments no real ring would accept.
///
/// # Safety
///
/// Only sound while a responder that answers `submit` **without forwarding**
/// is installed. With nothing installed this would pass a null handle to
/// `SubmitIoRing`, which is why no test calls it in that state.
unsafe fn submit_through_seam() -> HRESULT {
    let mut submitted = 0_u32;
    // SAFETY: forwarded from this function's own contract -- the caller has
    // installed a non-forwarding responder.
    unsafe { crate::sys::submit(std::ptr::null_mut(), 0, 0, &raw mut submitted) }
}

#[test]
fn an_installed_responder_answers_instead_of_the_kernel() {
    let seen = Rc::new(Cell::new(0));
    let guard = install(Box::new(Reporting {
        answer: E_FAIL,
        seen: Rc::clone(&seen),
    }));

    // SAFETY: the responder above answers without forwarding.
    let answer = unsafe { submit_through_seam() };

    assert_eq!(
        answer, E_FAIL,
        "the seam must return what the responder said, not what the kernel would"
    );
    assert_eq!(seen.get(), 1, "the responder must have been consulted once");
    drop(guard);
}

#[test]
fn the_responder_is_gone_once_its_guard_drops() {
    let seen = Rc::new(Cell::new(0));
    {
        let _guard = install(Box::new(Reporting {
            answer: S_OK,
            seen: Rc::clone(&seen),
        }));
        // SAFETY: as above.
        let _ = unsafe { submit_through_seam() };
    }
    assert_eq!(seen.get(), 1, "the responder answered while installed");

    // Asked at the install point rather than through the seam: calling the
    // seam with nothing installed would reach Win32 with a null handle.
    assert!(
        super::with(|_| ()).is_none(),
        "the guard must uninstall on drop, or a later test inherits this one's responder"
    );
}

#[test]
fn a_responder_is_scoped_to_the_thread_that_installed_it() {
    let seen = Rc::new(Cell::new(0));
    let _guard = install(Box::new(Reporting {
        answer: S_OK,
        seen: Rc::clone(&seen),
    }));

    // The property that makes this thread-local at all: `cargo test` runs
    // tests as threads in one process, so a process-global responder would
    // answer for tests that never asked for one.
    let elsewhere = std::thread::spawn(|| super::with(|_| ()).is_some())
        .join()
        .expect("the probe thread does not panic");

    assert!(
        !elsewhere,
        "another thread must not see this thread's responder"
    );
    assert_eq!(seen.get(), 0, "and must not have consulted it");
}

#[test]
#[should_panic(expected = "already installed")]
fn installing_twice_on_one_thread_is_refused() {
    // Nesting would make which responder answered a call depend on drop
    // order. Refusing is what keeps a suite that nested by accident readable.
    let _first = install(Box::new(Reporting {
        answer: S_OK,
        seen: Rc::new(Cell::new(0)),
    }));
    let _second = install(Box::new(Reporting {
        answer: S_OK,
        seen: Rc::new(Cell::new(0)),
    }));
}

#[test]
fn nothing_is_installed_by_default() {
    // The transparency property, asserted at the dispatch. With nothing
    // installed the seam makes the real call, and that path is what every
    // other test in this crate already exercises.
    assert!(
        super::with(|_| ()).is_none(),
        "a thread with no responder must fall through to the kernel"
    );
}

#[test]
fn a_responder_is_not_consulted_re_entrantly() {
    /// Re-enters the seam from inside its own `submit`.
    struct Reentrant {
        depth: Rc<Cell<usize>>,
    }

    impl Responses for Reentrant {
        unsafe fn submit(
            &mut self,
            _ring: *mut c_void,
            _wait: u32,
            _ms: u32,
            _submitted: *mut u32,
        ) -> HRESULT {
            self.depth.set(self.depth.get() + 1);
            // The inner call must find the slot borrowed and report `None`
            // rather than panicking on the `RefCell` or aliasing the `&mut`.
            assert!(
                super::with(|_| ()).is_none(),
                "a nested call must not reach the responder again"
            );
            S_OK
        }
    }

    let depth = Rc::new(Cell::new(0));
    let _guard = install(Box::new(Reentrant {
        depth: Rc::clone(&depth),
    }));
    // SAFETY: the responder answers without forwarding.
    let _ = unsafe { submit_through_seam() };
    assert_eq!(depth.get(), 1, "the outer call is consulted exactly once");
}
