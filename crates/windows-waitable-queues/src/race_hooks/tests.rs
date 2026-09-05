// Copyright (c) Mike Grier.

//! Tests for the hook facility itself.
//!
//! These test the *test infrastructure*, which is worth doing precisely because
//! everything else about the arming protocol is proved through it. A hook that
//! misbehaves does not fail loudly; it makes some other test fail for a reason
//! that has nothing to do with what that test is about.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::ARM;

#[test]
fn a_hook_fires_while_installed_and_not_afterwards() {
    // The static lives inside the test rather than at module scope: this
    // workspace runs tests as threads in one process, so a module-scope counter
    // could be moved by another test.
    static FIRED: AtomicUsize = AtomicUsize::new(0);

    ARM.with(
        || {
            FIRED.fetch_add(1, Ordering::Relaxed);
        },
        || {
            ARM.run();
            ARM.run();
        },
    );
    assert_eq!(FIRED.load(Ordering::Relaxed), 2, "the hook did not fire");

    ARM.run();
    assert_eq!(
        FIRED.load(Ordering::Relaxed),
        2,
        "the hook fired after it was removed"
    );
}

#[test]
fn a_panic_inside_the_body_still_removes_the_hook() {
    // The defect this guards. Removing the hook with a statement after `body`
    // means an unwind skips it, and a test that installs a hook and then fails
    // an assertion -- the ordinary way for a test to fail -- would leave it
    // installed to fire inside whatever ran next on this thread.
    static FIRED: AtomicUsize = AtomicUsize::new(0);

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        ARM.with(
            || {
                FIRED.fetch_add(1, Ordering::Relaxed);
            },
            || panic!("the body fails, as a failing test does"),
        );
    }));
    assert!(panicked.is_err(), "the panic must reach the caller");

    ARM.run();
    assert_eq!(
        FIRED.load(Ordering::Relaxed),
        0,
        "a hook survived an unwind and fired later"
    );
}

#[test]
fn a_hook_that_re_enters_its_own_window_does_not_trip_the_refcell() {
    // `run` takes the hook out for the call rather than holding the borrow
    // across it. Without that, a hook whose body reaches the same window again
    // would panic on a `RefCell` double borrow -- and the panic would look like
    // a fault in the queue rather than in the harness.
    static DEPTH: AtomicUsize = AtomicUsize::new(0);

    ARM.with(
        || {
            if DEPTH.fetch_add(1, Ordering::Relaxed) == 0 {
                ARM.run();
            }
        },
        || ARM.run(),
    );

    assert_eq!(
        DEPTH.load(Ordering::Relaxed),
        1,
        "re-entering the window should find the hook taken out, not re-run it"
    );
}
