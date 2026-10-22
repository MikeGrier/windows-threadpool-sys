// Copyright (c) 2026 Mike Grier
//! Unit tests for `signal_object_and_wait`.

use std::io;
use std::os::windows::io::OwnedHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

use crate::test_support::{BOUND, auto, is_signalled, manual};
use crate::{
    Access, Alertable, Event, WaitOutcome, duplicate_handle_with_access,
    queue_user_apc_to_current_thread, signal_object_and_wait, wait_for_single_object,
};

const NO: Alertable = Alertable::No;
const POLL: Option<Duration> = Some(Duration::ZERO);

fn denied() -> Option<i32> {
    i32::try_from(ERROR_ACCESS_DENIED).ok()
}

#[test]
fn the_signal_is_sent_and_the_wait_is_satisfied() {
    let signal = manual(false);
    let wait = auto(true);

    let outcome = signal_object_and_wait(&signal, &wait, POLL, NO).expect("the call");
    assert_eq!(outcome, WaitOutcome::Signalled);
    assert!(is_signalled(&signal), "the first object was set");
    assert!(
        !is_signalled(&wait),
        "and the auto-reset second one was taken"
    );
}

#[test]
fn a_wait_that_times_out_has_still_signalled() {
    let signal = manual(false);
    let wait = auto(false);

    let outcome = signal_object_and_wait(&signal, &wait, POLL, NO).expect("the call");
    assert_eq!(outcome, WaitOutcome::TimedOut);
    assert!(
        is_signalled(&signal),
        "the signal is not undone by the timeout"
    );
}

#[test]
fn the_signal_object_may_be_given_by_reference_or_by_value() {
    let wait = manual(true);
    let by_reference = manual(false);
    signal_object_and_wait(&by_reference, &wait, POLL, NO).expect("by reference");
    assert!(is_signalled(&by_reference));

    let by_value = manual(false);
    let keeper = by_value.clone();
    signal_object_and_wait(by_value, &wait, POLL, NO).expect("by value");
    assert!(is_signalled(&keeper));
}

#[test]
fn the_wait_object_may_be_any_handle() {
    let signal = manual(false);
    let owned: OwnedHandle = manual(true).try_into_owned().expect("the only holder");
    let outcome = signal_object_and_wait(&signal, &owned, POLL, NO).expect("the call");
    assert_eq!(outcome, WaitOutcome::Signalled);
}

#[test]
fn two_threads_hand_a_turn_back_and_forth() {
    const ROUNDS: usize = 200;
    let (ping, pong) = (auto(false), auto(false));
    let (their_ping, their_pong) = (ping.clone(), pong.clone());

    // Each side signals only after the other's signal was taken, so an auto-reset signal is never
    // set twice before it is taken, which would merge two turns into one.
    let worker = std::thread::spawn(move || {
        let first = wait_for_single_object(&their_ping, Some(BOUND)).expect("the first turn");
        assert_eq!(first, WaitOutcome::Signalled);
        for _ in 1..ROUNDS {
            let outcome = signal_object_and_wait(&their_pong, &their_ping, Some(BOUND), NO)
                .expect("the worker's step");
            assert_eq!(outcome, WaitOutcome::Signalled);
        }
        their_pong.set().expect("the last answer");
    });

    for _ in 0..ROUNDS {
        let outcome = signal_object_and_wait(&ping, &pong, Some(BOUND), NO).expect("the step");
        assert_eq!(outcome, WaitOutcome::Signalled);
    }
    worker.join().expect("the worker");
}

#[test]
fn an_alertable_step_is_ended_by_an_apc_after_signalling() {
    let signal = manual(false);
    let wait = auto(false);
    queue_user_apc_to_current_thread(|| {}).expect("queue");

    let outcome =
        signal_object_and_wait(&signal, &wait, Some(BOUND), Alertable::Yes).expect("call");
    assert_eq!(outcome, WaitOutcome::Alerted);
    assert!(
        is_signalled(&signal),
        "the signal was sent before the wait was ended"
    );
}

#[test]
fn a_signal_object_that_cannot_be_signalled_fails_and_waits_for_nothing() {
    let event = manual(false);
    let no_modify = duplicate_handle_with_access(&event, Access::SYNCHRONIZE).expect("duplicate");
    // SAFETY: a duplicate of an event is an event, and this one carries SYNCHRONIZE.
    let no_modify = unsafe { Event::from_owned_handle(no_modify) };
    let wait = auto(true);

    let error = signal_object_and_wait(&no_modify, &wait, POLL, NO).expect_err("cannot signal");
    assert_eq!(error.raw_os_error(), denied());
    assert!(is_signalled(&wait), "the wait object was not taken from");
    assert!(!is_signalled(&event), "and the event was not set");
}

#[test]
fn a_wait_object_that_cannot_be_waited_on_fails() {
    let signal = manual(false);
    let event = auto(true);
    let no_wait =
        duplicate_handle_with_access(&event, Access::EVENT_MODIFY_STATE).expect("duplicate");

    let error = signal_object_and_wait(&signal, &no_wait, POLL, NO).expect_err("cannot wait");
    assert_eq!(error.raw_os_error(), denied());
}

#[test]
fn a_timeout_the_call_cannot_express_is_refused_before_anything_is_signalled() {
    let signal = manual(false);
    let wait = manual(true);
    let too_long = Some(Duration::from_millis(u64::from(u32::MAX)));

    let error = signal_object_and_wait(&signal, &wait, too_long, NO).expect_err("refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(!is_signalled(&signal), "nothing was signalled");
}
