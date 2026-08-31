// Copyright (c) Mike Grier.

//! Test-only: the hook that drives `arm` through its own race window.
//!
//! `Consumer::arm` clears the doorbell and *then* checks whether anything is
//! takeable. The reverse order reads more naturally and is a permanent hang,
//! so the correct order has to be proven rather than asserted -- which means a
//! test must place a push inside the window between those two statements.
//!
//! # Why a hook rather than a hand-written copy of `arm`
//!
//! The first attempt at this proof was a duplicate of `arm` with the two
//! statements swapped, driven deterministically. It could only ever show that
//! *a* reversed order is wrong; it was structurally incapable of noticing the
//! **real** `arm` being reversed, which left that case covered only by whatever
//! interleavings the scheduler happened to produce. Measured: sabotaging the
//! real `arm` was then caught in one run out of three, because detection
//! depended on two threads meeting inside a window tens of nanoseconds wide.
//!
//! A second copy of a rule checks the copy, not the rule. So the real `arm`
//! carries this hook, and a test drives the real code through the exact window
//! on one thread.
//!
//! # Why it is shared between the shapes
//!
//! Both bounded shapes implement the same protocol, and each needs the same
//! proof. Giving each its own hook would reintroduce the duplication this file
//! exists to avoid, one layer down. The hook is thread-local, so two shapes'
//! tests running concurrently in one process cannot see each other's.

use core::cell::RefCell;

thread_local! {
    static HOOK: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// Runs the installed hook, if any. Called from inside `arm`.
pub(crate) fn run() {
    HOOK.with(|hook| {
        // Taken out for the call rather than held borrowed across it, so a hook
        // that touches the queue cannot trip a `RefCell` re-entrancy panic.
        let taken = hook.borrow_mut().take();
        if let Some(mut race) = taken {
            race();
            *hook.borrow_mut() = Some(race);
        }
    });
}

/// Installs a hook for the duration of a closure.
pub(crate) fn with<R>(race: impl FnMut() + 'static, body: impl FnOnce() -> R) -> R {
    HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(race)));
    let result = body();
    HOOK.with(|hook| *hook.borrow_mut() = None);
    result
}
