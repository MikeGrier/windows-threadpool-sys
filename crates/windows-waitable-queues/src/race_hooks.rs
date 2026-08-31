// Copyright (c) Mike Grier.

//! Test-only: hooks that drive a two-statement sequence through its own race
//! window, deterministically and on one thread.
//!
//! # Why these exist at all
//!
//! Two places in this crate consist of two statements whose *order* is the
//! whole correctness argument, and whose wrong order is a permanent hang rather
//! than an occasional stall: `Consumer::arm` and [`Doorbell::clear`]. Proving
//! such an order is load-bearing means placing a racing operation strictly
//! between the two statements, and that is not an interleaving a scheduler can
//! be asked for -- the window is tens of nanoseconds wide.
//!
//! # Why a hook rather than a hand-written copy of the code
//!
//! The first attempt at proving `arm` was a duplicate of it with the two
//! statements swapped, driven deterministically. It could only ever show that
//! *a* reversed order is wrong; it was structurally incapable of noticing the
//! **real** `arm` being reversed, which left that case covered only by
//! whatever interleavings the scheduler happened to produce. Measured:
//! sabotaging the real `arm` was then caught in one run out of three.
//!
//! A second copy of a rule is a check of the copy, not of the rule. So the real
//! code carries the hook, and a test drives the real code through the exact
//! window.
//!
//! # Why one facility for both
//!
//! Giving each site its own thread-local would reintroduce, one layer down, the
//! duplication this file exists to avoid. The hooks are thread-local, so two
//! suites running concurrently in one process cannot see each other's.
//!
//! [`Doorbell::clear`]: crate::doorbell::Doorbell::clear

use core::cell::RefCell;
use std::thread::LocalKey;

type Slot = RefCell<Option<Box<dyn FnMut()>>>;

thread_local! {
    static ARM_HOOK: Slot = const { RefCell::new(None) };
    static CLEAR_HOOK: Slot = const { RefCell::new(None) };
}

/// One named race window.
pub(crate) struct Hook(&'static LocalKey<Slot>);

/// Fires inside `Consumer::arm`, between clearing the doorbell and checking
/// whether anything is takeable.
pub(crate) const ARM: Hook = Hook(&ARM_HOOK);

/// Fires inside [`Doorbell::clear`](crate::doorbell::Doorbell::clear), between
/// resetting the event and clearing the flag that mirrors it.
pub(crate) const CLEAR: Hook = Hook(&CLEAR_HOOK);

impl Hook {
    /// Runs the installed hook, if any. Called from the code under test.
    pub(crate) fn run(&self) {
        self.0.with(|hook| {
            // Taken out for the call rather than held borrowed across it, so a
            // hook that re-enters this window cannot trip a `RefCell`
            // re-entrancy panic.
            let taken = hook.borrow_mut().take();
            if let Some(mut race) = taken {
                race();
                *hook.borrow_mut() = Some(race);
            }
        });
    }

    /// Installs a hook for the duration of a closure.
    pub(crate) fn with<R>(&self, race: impl FnMut() + 'static, body: impl FnOnce() -> R) -> R {
        self.0
            .with(|hook| *hook.borrow_mut() = Some(Box::new(race)));
        let result = body();
        self.0.with(|hook| *hook.borrow_mut() = None);
        result
    }
}
