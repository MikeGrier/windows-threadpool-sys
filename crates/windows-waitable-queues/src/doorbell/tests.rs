// Copyright (c) Mike Grier.

//! Tests for the doorbell in isolation, with no queue attached.
//!
//! These assert the properties the arming protocol is built on -- laziness,
//! level semantics, and that a redundant signal is skipped without losing a
//! necessary one. The protocol *itself* cannot be tested here, because it is a
//! statement about a queue this type cannot see; that is `spsc`'s job.

use std::os::windows::io::AsRawHandle;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use super::Doorbell;
use crate::race_hooks;

/// Whether the doorbell is signalled right now, by asking the kernel rather
/// than by reading the mirror flag.
///
/// A test that consulted the flag would be testing the flag against itself. The
/// zero timeout makes this a state query rather than a wait, and it does not
/// consume the signal because the event is manual-reset.
fn is_signalled(doorbell: &Doorbell) -> bool {
    let handle = doorbell.handle().expect("the doorbell must be creatable");
    // SAFETY: a live event handle borrowed for the call; a zero timeout returns
    // immediately and has no other precondition.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
    assert!(
        result == WAIT_OBJECT_0 || result == WAIT_TIMEOUT,
        "the wait must resolve to signalled or not, got {result:#x}"
    );
    result == WAIT_OBJECT_0
}

#[test]
fn creates_no_kernel_object_until_asked() {
    let doorbell = Doorbell::new();
    assert!(
        !doorbell.is_armed(),
        "a fresh doorbell must own no event, so a polling consumer pays nothing"
    );
}

#[test]
fn signalling_an_unarmed_doorbell_creates_nothing() {
    let doorbell = Doorbell::new();
    doorbell.signal();
    doorbell.signal();
    assert!(
        !doorbell.is_armed(),
        "a producer must not conjure a kernel object nobody asked for"
    );
}

#[test]
fn clearing_an_unarmed_doorbell_creates_nothing() {
    let doorbell = Doorbell::new();
    doorbell.clear();
    assert!(!doorbell.is_armed(), "clearing must not create the event");
}

#[test]
fn asking_for_the_handle_creates_the_event() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");
    assert!(
        doorbell.is_armed(),
        "the handle request must create the event"
    );
}

#[test]
fn the_handle_is_stable_across_calls() {
    let doorbell = Doorbell::new();
    let first = doorbell
        .handle()
        .expect("creation must succeed")
        .as_raw_handle();
    let second = doorbell
        .handle()
        .expect("creation must succeed")
        .as_raw_handle();
    assert_eq!(
        first, second,
        "the event is created once, so every borrow must name the same object"
    );
}

#[test]
fn a_new_doorbell_is_unsignalled() {
    let doorbell = Doorbell::new();
    assert!(
        !is_signalled(&doorbell),
        "a doorbell must not claim readiness before anything is pushed"
    );
}

#[test]
fn signal_makes_it_signalled() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");
    doorbell.signal();
    assert!(is_signalled(&doorbell), "signalling must be observable");
}

#[test]
fn the_signal_is_a_level_and_survives_being_observed() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");
    doorbell.signal();

    // Three observations, because the failure this guards against is an
    // auto-reset event, where the first wait consumes the signal and the second
    // blocks. That exact mistake hung the crate's own doorbell probe for four
    // hundred seconds before the design was fixed.
    for observation in 1..=3 {
        assert!(
            is_signalled(&doorbell),
            "observation {observation} must still see the level"
        );
    }
}

#[test]
fn clear_makes_it_unsignalled() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");
    doorbell.signal();
    doorbell.clear();
    assert!(!is_signalled(&doorbell), "clearing must reset the level");
}

#[test]
fn a_signal_after_a_clear_is_delivered() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");

    // The sequence that the skip-redundant-signals flag could plausibly break:
    // if `clear` failed to reset the flag, this second signal would be skipped
    // and the doorbell would stay dark with an item waiting.
    doorbell.signal();
    doorbell.clear();
    doorbell.signal();

    assert!(
        is_signalled(&doorbell),
        "a signal after a clear is the one signal that must never be skipped"
    );
}

#[test]
fn many_clear_signal_cycles_stay_in_agreement() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");

    for cycle in 0..64 {
        doorbell.signal();
        assert!(is_signalled(&doorbell), "cycle {cycle} must signal");
        doorbell.clear();
        assert!(!is_signalled(&doorbell), "cycle {cycle} must clear");
    }
}

#[test]
fn repeated_signals_remain_signalled() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");

    // The redundant ones take the skip path. The observable state must not
    // depend on how many were issued.
    for _ in 0..16 {
        doorbell.signal();
    }
    assert!(
        is_signalled(&doorbell),
        "redundant signals must not clear it"
    );

    doorbell.clear();
    assert!(
        !is_signalled(&doorbell),
        "one clear must undo any number of signals, because the event is a level"
    );
}

#[test]
fn repeated_clears_remain_clear() {
    let doorbell = Doorbell::new();
    let _handle = doorbell.handle().expect("creation must succeed");
    doorbell.signal();
    for _ in 0..16 {
        doorbell.clear();
    }
    assert!(
        !is_signalled(&doorbell),
        "redundant clears must not signal it"
    );
}

#[test]
fn a_signal_issued_before_the_handle_existed_is_not_delivered() {
    let doorbell = Doorbell::new();

    // This is the lazy-creation hole, asserted rather than hoped about: the
    // producer ran while there was no event, so its signal went nowhere. The
    // arming protocol's re-check is what makes this survivable, and that is
    // tested against a real queue in `spsc`.
    doorbell.signal();

    assert!(
        !is_signalled(&doorbell),
        "a doorbell created after the fact cannot know what it missed, which is \
         precisely why the owner must re-check emptiness before waiting"
    );
}

#[test]
fn the_owned_duplicate_names_the_same_event() {
    let doorbell = Doorbell::new();
    let owned = doorbell.owned().expect("duplication must succeed");

    // A distinct handle value, but the same underlying object: signalling
    // through the queue's copy must be visible through the caller's.
    doorbell.signal();

    // SAFETY: `owned` is a live event handle; a zero timeout returns at once.
    let result = unsafe { WaitForSingleObject(owned.as_raw_handle(), 0) };
    assert_eq!(
        result, WAIT_OBJECT_0,
        "the duplicate must observe the original's signal"
    );

    doorbell.clear();
    // SAFETY: as above.
    let result = unsafe { WaitForSingleObject(owned.as_raw_handle(), 0) };
    assert_eq!(
        result, WAIT_TIMEOUT,
        "the duplicate must observe the original's clear"
    );
}

#[test]
fn dropping_the_owned_duplicate_leaves_the_doorbell_usable() {
    let doorbell = Doorbell::new();
    let owned = doorbell.owned().expect("duplication must succeed");
    drop(owned);

    // The caller closing its own copy must not close the queue's. If it did,
    // this signal would be a use-after-close rather than a no-op.
    doorbell.signal();
    assert!(
        is_signalled(&doorbell),
        "the queue's event must outlive any duplicate handed out"
    );
}

#[test]
fn a_duplicate_taken_before_a_signal_still_sees_it() {
    let doorbell = Doorbell::new();
    let owned = doorbell.owned().expect("duplication must succeed");
    doorbell.signal();

    // SAFETY: `owned` is a live event handle; a zero timeout returns at once.
    let result = unsafe { WaitForSingleObject(owned.as_raw_handle(), 0) };
    assert_eq!(
        result, WAIT_OBJECT_0,
        "duplication order must not affect what the duplicate observes"
    );
}

#[test]
fn several_duplicates_all_observe_the_same_state() {
    let doorbell = Doorbell::new();
    let handles: Vec<_> = (0..4)
        .map(|_| doorbell.owned().expect("duplication must succeed"))
        .collect();
    doorbell.signal();

    for (index, handle) in handles.iter().enumerate() {
        // SAFETY: each is a live event handle; a zero timeout returns at once.
        let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
        assert_eq!(
            result, WAIT_OBJECT_0,
            "duplicate {index} must see the signal"
        );
    }
}

#[test]
fn a_waiting_thread_is_released_by_a_signal() {
    use std::sync::Arc;
    use std::thread;

    // The property the whole crate exists for, end to end through the kernel:
    // a thread parked in a real blocking wait is released by `signal`. Every
    // other test here uses a zero timeout, which never actually blocks.
    let doorbell = Arc::new(Doorbell::new());
    let waiter = doorbell.clone();
    let handle = waiter.owned().expect("duplication must succeed");

    let joiner = thread::spawn(move || {
        // Five seconds is not a timing assertion; it is a bound so a broken
        // doorbell fails the suite instead of hanging it forever.
        // SAFETY: a live event handle owned by this thread for the call.
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 5_000) }
    });

    doorbell.signal();

    let result = joiner.join().expect("the waiting thread must not panic");
    assert_eq!(
        result, WAIT_OBJECT_0,
        "a blocked waiter must be released by a signal, not by the timeout"
    );
}

// ---------------------------------------------------------------------------
// The flag must never outlive the signal it mirrors.
//
// `clear` resets the event and *then* clears the flag. Written the other way
// round -- which is how this shipped originally -- a producer signalling
// between the two lines finds a clear flag, sets it, and issues a real
// `SetEvent`; the `ResetEvent` that follows erases that signal and leaves the
// flag set. The doorbell is then wedged dark while claiming to be lit, and
// every later `signal` skips its syscall.
//
// The window is two instructions wide, so the race is driven through the real
// `clear` by a hook rather than raced for on two threads: an interleaving that
// must be hit to prove a point is not one to leave to the scheduler.
// ---------------------------------------------------------------------------

#[test]
fn a_signal_racing_a_clear_leaves_the_next_one_able_to_ring() {
    // Shared rather than borrowed because the hook must be `'static`. One
    // thread throughout -- the `Arc` is a lifetime device, not concurrency.
    let doorbell = Arc::new(Doorbell::new());
    doorbell.handle().expect("the doorbell must be creatable");

    // Start from the state that makes the wrong order fatal: already lit, so a
    // producer racing the clear can find the flag either way depending on the
    // order of the two lines.
    doorbell.signal();
    assert!(is_signalled(&doorbell), "the setup must actually light it");

    let racing = Arc::clone(&doorbell);
    race_hooks::CLEAR.with(move || racing.signal(), || doorbell.clear());

    // Nothing is asserted about the event's state right here, and the omission
    // is deliberate. Whether the racing signal survived the clear depends on
    // whether it was skipped, which depends on the flag optimisation -- and a
    // signal that races a clear is entitled to leave the event lit, because
    // that is a spurious wakeup and consumers tolerate those by contract. An
    // earlier version of this test did assert it, and the sabotage sweep's
    // control -- "signal always syscalls, skipping the flag optimisation" --
    // caught it, which is exactly what that control exists to do: it reported
    // this test as asserting the implementation instead of the contract.

    // The assertion that matters, and the one the wrong order fails: whatever
    // happened during the window, `clear` must leave the doorbell able to ring
    // again. A queue's consumer parks immediately after this returns, and its
    // wakeup is the next producer's `signal`.
    doorbell.signal();
    assert!(
        is_signalled(&doorbell),
        "a signal racing a clear must not wedge the doorbell dark; the flag \
         would be claiming 'already lit' over an event nothing will ever set"
    );
}

#[test]
fn a_clear_with_nothing_racing_it_still_re_arms() {
    // The control for the test above: it must not pass merely because `clear`
    // never leaves the doorbell ringable, so the same sequence is checked with
    // an empty window.
    let doorbell = Arc::new(Doorbell::new());
    doorbell.handle().expect("the doorbell must be creatable");
    doorbell.signal();

    race_hooks::CLEAR.with(|| {}, || doorbell.clear());
    assert!(!is_signalled(&doorbell), "nothing raced it, so it is dark");

    doorbell.signal();
    assert!(is_signalled(&doorbell), "and the next signal rings");
}

#[test]
fn the_debug_rendering_shows_whether_the_event_exists_and_its_state() {
    // Both fields, and both values of the one that moves. This rendering is how
    // a reader tells "the doorbell was never created" from "it was created and
    // is unsignalled" -- the laziness being visible, per D-5 -- so an empty
    // rendering loses exactly the distinction it exists to show.
    let doorbell = Doorbell::new();
    let before = format!("{doorbell:?}");
    assert!(before.contains("Doorbell"), "got {before}");
    assert!(
        before.contains("false"),
        "an untouched doorbell is neither created nor signalled: {before}"
    );

    // Signalling one nobody has asked a handle for is a no-op by design -- the
    // laziness D-5 describes -- so the rendering must still read false. This is
    // the distinction the rendering exists to show, and it is why the test
    // cannot simply signal and look for "true".
    doorbell.signal();
    let unheard = format!("{doorbell:?}");
    assert!(
        !unheard.contains("true"),
        "a doorbell nobody is listening to was not created or signalled: {unheard}"
    );

    // Asking for the handle is what creates it; only then does a signal land.
    doorbell.handle().expect("the doorbell must be creatable");
    doorbell.signal();
    let after = format!("{doorbell:?}");
    assert!(
        after.contains("created: true"),
        "the event now exists: {after}"
    );
    assert!(
        after.contains("signalled: true"),
        "and has been rung: {after}"
    );
}
