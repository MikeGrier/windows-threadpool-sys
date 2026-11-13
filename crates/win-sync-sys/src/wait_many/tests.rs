// Copyright (c) 2026 Mike Grier
//! Unit tests for `wait_for_any` and `wait_for_all`.

use std::io;
use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, WAIT_ABANDONED_0, WAIT_FAILED,
    WAIT_IO_COMPLETION, WAIT_OBJECT_0, WAIT_TIMEOUT,
};

use super::{
    MAXIMUM_OBJECTS, WaitAnyOutcome, all_outcome, any_outcome, wait_for_all, wait_for_any,
};
use crate::test_support::{BOUND, abandoned_mutex, auto, is_signalled, manual};
use crate::{
    Access, Alertable, Event, SealedWaitOnlyEvent, WaitObject, WaitOutcome,
    duplicate_handle_with_access, queue_user_apc_to_current_thread,
};

const NO: Alertable = Alertable::No;
const POLL: Option<Duration> = Some(Duration::ZERO);

#[test]
fn sealed_mixture_reports_each_signalled_object_without_lending_handles() {
    let owners = [manual(false), manual(false), manual(false)];
    let sealed = owners[1]
        .duplicate_sealed_wait_only()
        .expect("sealed event");
    let objects = [
        owners[0].as_handle().into(),
        sealed.wait_object(),
        owners[2].as_handle().into(),
    ];
    assert_eq!(
        wait_for_any(&objects, POLL, NO).expect("poll"),
        WaitAnyOutcome::TimedOut
    );
    for (index, owner) in owners.iter().enumerate() {
        owner.set().expect("set");
        assert_eq!(
            wait_for_any(&objects, POLL, NO).expect("wait"),
            WaitAnyOutcome::Signalled(index)
        );
        owner.reset().expect("reset");
    }
}

#[test]
fn sealed_mixture_wait_all_takes_nothing_until_every_object_is_signalled() {
    for sealed_first in [false, true] {
        let owner = auto(false);
        let other = auto(false);
        let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
        let objects = [sealed.wait_object(), other.as_handle().into()];
        let (first, last) = if sealed_first {
            (&owner, &other)
        } else {
            (&other, &owner)
        };
        first.set().expect("first signal");
        assert_eq!(
            wait_for_all(&objects, POLL, NO).expect("partial wait"),
            WaitOutcome::TimedOut
        );
        assert!(is_signalled(first), "timed-out wait consumed nothing");
        first.set().expect("restore signal consumed by observation");
        last.set().expect("last signal");
        assert_eq!(
            wait_for_all(&objects, Some(BOUND), NO).expect("complete wait"),
            WaitOutcome::Signalled
        );
        assert!(!is_signalled(&owner));
        assert!(!is_signalled(&other));
    }
}

#[test]
fn sealed_mixture_wait_any_consumes_only_the_lowest_signalled_auto_event() {
    let owners = [auto(true), auto(true), auto(true)];
    let sealed = owners[1]
        .duplicate_sealed_wait_only()
        .expect("sealed event");
    let objects = [
        owners[0].as_handle().into(),
        sealed.wait_object(),
        owners[2].as_handle().into(),
    ];
    for (index, owner) in owners.iter().enumerate() {
        assert_eq!(
            wait_for_any(&objects, POLL, NO).expect("wait"),
            WaitAnyOutcome::Signalled(index)
        );
        assert!(!is_signalled(owner));
    }
    assert_eq!(
        wait_for_any(&objects, POLL, NO).expect("poll"),
        WaitAnyOutcome::TimedOut
    );
}

#[test]
fn sealed_mixture_wait_all_leaves_manual_events_signalled() {
    let owner = manual(true);
    let other = manual(true);
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let objects = [sealed.wait_object(), other.as_handle().into()];
    for _ in 0..2 {
        assert_eq!(
            wait_for_all(&objects, POLL, NO).expect("wait"),
            WaitOutcome::Signalled
        );
    }
    assert!(is_signalled(&owner));
    assert!(is_signalled(&other));
}

#[test]
fn sealed_mixtures_honor_alertability_without_consuming_signals() {
    let owner = auto(true);
    let other = auto(true);
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let objects = [sealed.wait_object(), other.as_handle().into()];
    queue_user_apc_to_current_thread(|| {}).expect("queue");
    assert_eq!(
        wait_for_any(&objects, Some(BOUND), Alertable::Yes).expect("alerted any"),
        WaitAnyOutcome::Alerted
    );
    queue_user_apc_to_current_thread(|| {}).expect("queue");
    assert_eq!(
        wait_for_all(&objects, Some(BOUND), Alertable::Yes).expect("alerted all"),
        WaitOutcome::Alerted
    );
    assert!(is_signalled(&owner));
    assert!(is_signalled(&other));
    queue_user_apc_to_current_thread(|| {}).expect("queue");
    assert_eq!(
        wait_for_any(&objects, POLL, NO).expect("ordinary poll"),
        WaitAnyOutcome::TimedOut
    );
    assert_eq!(
        wait_for_all(&objects, POLL, NO).expect("ordinary poll"),
        WaitOutcome::TimedOut
    );
    assert_eq!(
        wait_for_any(&objects, Some(BOUND), Alertable::Yes).expect("drain APC"),
        WaitAnyOutcome::Alerted
    );
}

#[test]
fn sealed_events_alone_accept_owned_and_reference_slices() {
    let owners = [manual(true), manual(true)];
    let sealed: Vec<SealedWaitOnlyEvent> = owners
        .iter()
        .map(|owner| owner.duplicate_sealed_wait_only().expect("sealed event"))
        .collect();
    assert_eq!(
        wait_for_any(&sealed, POLL, NO).expect("owned any"),
        WaitAnyOutcome::Signalled(0)
    );
    assert_eq!(
        wait_for_all(&sealed, POLL, NO).expect("owned all"),
        WaitOutcome::Signalled
    );
    let borrowed = [&sealed[0], &sealed[1]];
    assert_eq!(
        wait_for_any(&borrowed, POLL, NO).expect("borrowed any"),
        WaitAnyOutcome::Signalled(0)
    );
    assert_eq!(
        wait_for_all(&borrowed, POLL, NO).expect("borrowed all"),
        WaitOutcome::Signalled
    );
}

#[test]
fn sealed_mixtures_preserve_native_access_errors() {
    let owner = manual(true);
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let no_wait = duplicate_handle_with_access(&owner, Access::EVENT_MODIFY_STATE)
        .expect("non-waitable handle");
    let objects = [no_wait.as_handle().into(), sealed.wait_object()];
    for result in [
        wait_for_any(&objects, POLL, NO).map(|_| ()),
        wait_for_all(&objects, POLL, NO).map(|_| ()),
    ] {
        assert_eq!(
            result.expect_err("access denied").raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );
    }
    assert!(is_signalled(&owner));
}

#[test]
fn sealed_mixtures_support_the_maximum_count_and_refuse_invalid_counts_and_timeouts() {
    let owners = events(MAXIMUM_OBJECTS);
    let sealed = owners[MAXIMUM_OBJECTS - 1]
        .duplicate_sealed_wait_only()
        .expect("sealed event");
    let mut objects: Vec<WaitObject<'_>> = owners[..MAXIMUM_OBJECTS - 1]
        .iter()
        .map(|owner| owner.as_handle().into())
        .collect();
    objects.push(sealed.wait_object());
    owners[MAXIMUM_OBJECTS - 1].set().expect("last signal");
    assert_eq!(
        wait_for_any(&objects, POLL, NO).expect("maximum any"),
        WaitAnyOutcome::Signalled(MAXIMUM_OBJECTS - 1)
    );
    for owner in &owners {
        owner.set().expect("set");
    }
    assert_eq!(
        wait_for_all(&objects, POLL, NO).expect("maximum all"),
        WaitOutcome::Signalled
    );
    assert_eq!(
        wait_for_any(&objects, POLL, NO).expect("consumed"),
        WaitAnyOutcome::TimedOut
    );
    let empty: [WaitObject<'_>; 0] = [];
    objects.push(sealed.wait_object());
    for targets in [&empty[..], &objects[..]] {
        for result in [
            wait_for_any(targets, POLL, NO).map(|_| ()),
            wait_for_all(targets, POLL, NO).map(|_| ()),
        ] {
            assert_eq!(
                result.expect_err("invalid count").kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
    objects.pop();
    owners[MAXIMUM_OBJECTS - 1].set().expect("set");
    let too_long = Some(Duration::from_millis(u64::from(u32::MAX)));
    for result in [
        wait_for_any(&objects, too_long, NO).map(|_| ()),
        wait_for_all(&objects, too_long, NO).map(|_| ()),
    ] {
        assert_eq!(
            result.expect_err("invalid timeout").kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert!(is_signalled(&owners[MAXIMUM_OBJECTS - 1]));
}

#[test]
fn sealed_mixture_accepts_a_mutex_and_reports_abandonment() {
    let owner = auto(true);
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let mutex = abandoned_mutex();
    let objects = [mutex.as_handle().into(), sealed.wait_object()];
    assert_eq!(
        wait_for_any(&objects, POLL, NO).expect("abandoned any"),
        WaitAnyOutcome::Abandoned(0)
    );
    assert!(is_signalled(&owner), "the sealed event was not selected");
    owner.set().expect("restore signal");
    let second_mutex = abandoned_mutex();
    let objects = [sealed.wait_object(), second_mutex.as_handle().into()];
    assert_eq!(
        wait_for_all(&objects, POLL, NO).expect("abandoned all"),
        WaitOutcome::Abandoned
    );
    assert!(!is_signalled(&owner));
}

#[test]
fn sealed_wait_object_copies_are_the_same_target_not_duplicate_handles() {
    let owner = manual(true);
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let object = sealed.wait_object();
    let twice = [object, object];
    assert_eq!(
        wait_for_any(&twice, POLL, NO).expect("any"),
        WaitAnyOutcome::Signalled(0)
    );
    assert_eq!(
        wait_for_all(&twice, POLL, NO)
            .expect_err("same handle twice")
            .raw_os_error(),
        Some(ERROR_INVALID_PARAMETER as i32)
    );
    assert!(is_signalled(&owner));
}

#[test]
fn sealed_mixture_waits_for_another_thread_to_set_the_event() {
    let owner = manual(false);
    let other = manual(false);
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let signaller = owner.clone();
    let worker = std::thread::spawn(move || signaller.set());
    let objects = [other.as_handle().into(), sealed.wait_object()];
    assert_eq!(
        wait_for_any(&objects, Some(BOUND), NO).expect("wait"),
        WaitAnyOutcome::Signalled(1)
    );
    worker.join().expect("worker").expect("set");
}

/// `count` auto-reset events, none signalled.
fn events(count: usize) -> Vec<Event> {
    (0..count).map(|_| auto(false)).collect()
}

fn any(objects: &[impl AsHandle], timeout: Option<Duration>) -> WaitAnyOutcome {
    wait_for_any(objects, timeout, NO).expect("the wait is answered")
}

fn all(objects: &[impl AsHandle], timeout: Option<Duration>) -> WaitOutcome {
    wait_for_all(objects, timeout, NO).expect("the wait is answered")
}

#[test]
fn any_reports_the_index_of_the_signalled_object_at_every_position() {
    let events = events(5);
    for index in 0..events.len() {
        events[index].set().expect("set");
        assert_eq!(
            any(&events, Some(BOUND)),
            WaitAnyOutcome::Signalled(index),
            "for the event at {index}"
        );
    }
}

#[test]
fn any_takes_from_the_one_it_reports_and_from_no_other() {
    let events = events(3);
    events[1].set().expect("set");
    events[2].set().expect("set");

    assert_eq!(any(&events, POLL), WaitAnyOutcome::Signalled(1));
    assert!(
        !is_signalled(&events[1]),
        "the reported auto-reset event was taken"
    );
    assert!(is_signalled(&events[2]), "the other one was left signalled");
}

#[test]
fn of_several_signalled_any_reports_the_lowest_index_each_time() {
    let events = events(4);
    for event in &events {
        event.set().expect("set");
    }
    for expected in 0..events.len() {
        assert_eq!(any(&events, POLL), WaitAnyOutcome::Signalled(expected));
    }
    assert_eq!(any(&events, POLL), WaitAnyOutcome::TimedOut);
}

#[test]
fn any_times_out_when_nothing_is_signalled_and_takes_at_least_its_time() {
    let events = events(3);
    assert_eq!(any(&events, POLL), WaitAnyOutcome::TimedOut);

    let timeout = Duration::from_millis(60);
    let started = Instant::now();
    assert_eq!(any(&events, Some(timeout)), WaitAnyOutcome::TimedOut);
    assert!(started.elapsed() >= timeout - Duration::from_millis(40));
}

#[test]
fn any_is_released_by_a_set_on_another_thread() {
    let events = events(3);
    let signaller = events[2].clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        signaller.set()
    });
    assert_eq!(any(&events, Some(BOUND)), WaitAnyOutcome::Signalled(2));
    worker.join().expect("worker").expect("set");
}

#[test]
fn any_reports_an_abandoned_mutex_by_its_index_and_owns_it_after() {
    let mutex = abandoned_mutex();
    let first = auto(false);
    let objects = [first.as_handle(), mutex.as_handle()];
    assert_eq!(any(&objects, Some(BOUND)), WaitAnyOutcome::Abandoned(1));
    // The waiting thread owns the mutex now, which is recursive for its owner.
    assert_eq!(any(&objects, Some(BOUND)), WaitAnyOutcome::Signalled(1));
}

#[test]
fn any_takes_a_mixture_of_object_kinds_as_borrowed_handles() {
    let event = auto(false);
    let owned: OwnedHandle = manual(true).try_into_owned().expect("the only holder");
    let mutex = abandoned_mutex();
    let objects: [BorrowedHandle<'_>; 3] =
        [event.as_handle(), owned.as_handle(), mutex.as_handle()];
    assert_eq!(any(&objects, POLL), WaitAnyOutcome::Signalled(1));
}

#[test]
fn the_widest_wait_takes_the_maximum_number_of_objects_and_sees_the_last() {
    let events = events(MAXIMUM_OBJECTS);
    events[MAXIMUM_OBJECTS - 1].set().expect("set");
    assert_eq!(
        any(&events, POLL),
        WaitAnyOutcome::Signalled(MAXIMUM_OBJECTS - 1)
    );

    for event in &events {
        event.set().expect("set");
    }
    assert_eq!(all(&events, Some(BOUND)), WaitOutcome::Signalled);
    assert_eq!(
        any(&events, POLL),
        WaitAnyOutcome::TimedOut,
        "all were taken"
    );
}

#[test]
fn a_wait_for_none_or_for_too_many_is_refused_without_waiting() {
    let none: [Event; 0] = [];
    let too_many = events(MAXIMUM_OBJECTS + 1);
    too_many[0].set().expect("set");

    let started = Instant::now();
    for result in [
        wait_for_any(&none, None, NO).map(|_| ()),
        wait_for_all(&none, None, NO).map(|_| ()),
        wait_for_any(&too_many, None, NO).map(|_| ()),
        wait_for_all(&too_many, None, NO).map(|_| ()),
    ] {
        let error = result.expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    assert!(started.elapsed() < BOUND, "refused rather than waited for");
    assert!(is_signalled(&too_many[0]), "and nothing was taken");
}

#[test]
fn a_timeout_the_call_cannot_express_is_refused_by_both() {
    let events = events(2);
    events[0].set().expect("set");
    let too_long = Some(Duration::from_millis(u64::from(u32::MAX)));
    assert_eq!(
        wait_for_any(&events, too_long, NO)
            .expect_err("refused")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        wait_for_all(&events, too_long, NO)
            .expect_err("refused")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(is_signalled(&events[0]), "nothing was taken");
}

#[test]
fn all_is_satisfied_only_when_every_object_is_signalled_and_takes_them_all() {
    let events = events(3);
    for event in &events {
        event.set().expect("set");
    }
    assert_eq!(all(&events, POLL), WaitOutcome::Signalled);
    for event in &events {
        assert!(!is_signalled(event), "an auto-reset event was taken");
    }
}

#[test]
fn all_that_times_out_has_taken_nothing() {
    let events = events(3);
    events[0].set().expect("set");
    events[1].set().expect("set");

    assert_eq!(all(&events, POLL), WaitOutcome::TimedOut);
    assert!(is_signalled(&events[0]), "the signalled ones are untouched");
    assert!(is_signalled(&events[1]));
}

#[test]
fn all_is_released_when_the_last_object_is_set_on_another_thread() {
    let events = events(3);
    events[0].set().expect("set");
    events[1].set().expect("set");
    let signaller = events[2].clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        signaller.set()
    });
    assert_eq!(all(&events, Some(BOUND)), WaitOutcome::Signalled);
    worker.join().expect("worker").expect("set");
    assert!(!is_signalled(&events[0]), "and every one was taken");
}

#[test]
fn all_with_one_object_is_a_single_wait() {
    let events = events(1);
    assert_eq!(all(&events, POLL), WaitOutcome::TimedOut);
    events[0].set().expect("set");
    assert_eq!(all(&events, POLL), WaitOutcome::Signalled);
}

#[test]
fn all_reports_an_abandoned_mutex_among_signalled_events_and_owns_it() {
    let mutex = abandoned_mutex();
    let event = auto(true);
    let objects = [event.as_handle(), mutex.as_handle()];
    assert_eq!(all(&objects, Some(BOUND)), WaitOutcome::Abandoned);
    assert!(!is_signalled(&event), "the event was taken too");
    assert!(
        is_signalled(&mutex),
        "the thread owns the mutex, which is recursive"
    );
}

#[test]
fn an_alertable_wait_for_any_or_all_is_ended_by_an_apc_and_takes_nothing() {
    let events = events(2);
    events[0].set().expect("set");

    queue_user_apc_to_current_thread(|| {}).expect("queue");
    let unsatisfied = wait_for_all(&events, Some(BOUND), Alertable::Yes).expect("wait");
    assert_eq!(unsatisfied, WaitOutcome::Alerted);
    assert!(is_signalled(&events[0]), "an alerted wait took nothing");
    events[0].set().expect("set");

    queue_user_apc_to_current_thread(|| {}).expect("queue");
    let none_signalled = [auto(false), auto(false)];
    let outcome = wait_for_any(&none_signalled, Some(BOUND), Alertable::Yes).expect("wait");
    assert_eq!(outcome, WaitAnyOutcome::Alerted);
}

#[test]
fn a_wait_that_is_not_alertable_leaves_the_apc_for_a_later_one() {
    let events = events(2);
    queue_user_apc_to_current_thread(|| {}).expect("queue");
    assert_eq!(any(&events, POLL), WaitAnyOutcome::TimedOut);
    assert_eq!(
        wait_for_any(&events, Some(BOUND), Alertable::Yes).expect("wait"),
        WaitAnyOutcome::Alerted
    );
}

#[test]
fn a_wait_that_fails_reports_the_system_error_for_either() {
    let event = auto(true);
    let no_wait_access =
        duplicate_handle_with_access(&event, Access::EVENT_MODIFY_STATE).expect("duplicate");
    let objects = [no_wait_access];
    let denied = i32::try_from(ERROR_ACCESS_DENIED).ok();
    for result in [
        wait_for_any(&objects, POLL, NO).map(|_| ()),
        wait_for_all(&objects, POLL, NO).map(|_| ()),
    ] {
        assert_eq!(result.expect_err("cannot wait").raw_os_error(), denied);
    }
}

/// What Windows does with a handle named twice, observed on this host and written down: a wait for
/// any tolerates it and reports the first index, and a wait for all refuses it, as the Win32
/// documentation says it may.
#[test]
fn the_same_handle_twice_is_tolerated_by_any_and_refused_by_all() {
    let event = manual(true);
    let twice = [event.as_handle(), event.as_handle()];
    assert_eq!(any(&twice, POLL), WaitAnyOutcome::Signalled(0));

    let error = wait_for_all(&twice, POLL, NO).expect_err("refused");
    assert_eq!(
        error.raw_os_error(),
        i32::try_from(ERROR_INVALID_PARAMETER).ok()
    );
    assert!(is_signalled(&event), "and the refused wait took nothing");
}

/// Observed on this host and written down: an alertable wait runs a pending APC and reports
/// `Alerted` even when an object is already signalled, and leaves that object alone, so waiting
/// again takes it.
#[test]
fn a_pending_apc_ends_an_alertable_wait_even_when_an_object_is_already_signalled() {
    let events = events(2);
    events[1].set().expect("set");

    queue_user_apc_to_current_thread(|| {}).expect("queue");
    let outcome = wait_for_any(&events, POLL, Alertable::Yes).expect("wait");
    assert_eq!(outcome, WaitAnyOutcome::Alerted);
    assert!(
        is_signalled(&events[1]),
        "the signalled object was left alone"
    );
    events[1].set().expect("set");

    queue_user_apc_to_current_thread(|| {}).expect("queue");
    let outcome = wait_for_all(&events[1..], POLL, Alertable::Yes).expect("wait");
    assert_eq!(outcome, WaitOutcome::Alerted);
    assert_eq!(
        wait_for_all(&events[1..], POLL, Alertable::Yes).expect("wait"),
        WaitOutcome::Signalled,
        "the next wait takes it"
    );
}

#[test]
fn any_outcome_maps_every_code_the_call_returns() {
    for index in 0..3_u32 {
        let signalled = any_outcome(WAIT_OBJECT_0 + index, 3).expect("ok");
        assert_eq!(signalled, WaitAnyOutcome::Signalled(index as usize));
        let abandoned = any_outcome(WAIT_ABANDONED_0 + index, 3).expect("ok");
        assert_eq!(abandoned, WaitAnyOutcome::Abandoned(index as usize));
    }
    assert_eq!(
        any_outcome(WAIT_TIMEOUT, 3).expect("ok"),
        WaitAnyOutcome::TimedOut
    );
    assert_eq!(
        any_outcome(WAIT_IO_COMPLETION, 3).expect("ok"),
        WaitAnyOutcome::Alerted
    );

    // An index past the objects given is not one of them, and is an error rather than a guess.
    for code in [
        WAIT_OBJECT_0 + 3,
        WAIT_ABANDONED_0 + 3,
        WAIT_FAILED - 1,
        0x1234,
    ] {
        let error = any_outcome(code, 3).expect_err("not an outcome");
        assert_eq!(error.kind(), io::ErrorKind::Other, "for {code:#x}");
    }
}

#[test]
fn all_outcome_maps_every_code_the_call_returns() {
    assert_eq!(
        all_outcome(WAIT_OBJECT_0, 3).expect("ok"),
        WaitOutcome::Signalled
    );
    assert_eq!(
        all_outcome(WAIT_ABANDONED_0, 3).expect("ok"),
        WaitOutcome::Abandoned
    );
    assert_eq!(
        all_outcome(WAIT_ABANDONED_0 + 2, 3).expect("ok"),
        WaitOutcome::Abandoned
    );
    assert_eq!(
        all_outcome(WAIT_TIMEOUT, 3).expect("ok"),
        WaitOutcome::TimedOut
    );
    assert_eq!(
        all_outcome(WAIT_IO_COMPLETION, 3).expect("ok"),
        WaitOutcome::Alerted
    );

    for code in [WAIT_OBJECT_0 + 3, WAIT_ABANDONED_0 + 3, 0x1234] {
        let error = all_outcome(code, 3).expect_err("not an outcome");
        assert_eq!(error.kind(), io::ErrorKind::Other, "for {code:#x}");
    }
}
