// Copyright (c) 2026 Mike Grier
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::Doorbell;
use crate::test_support::{is_signalled, signalled_within};
use crate::{CreateError, Watcher};

#[test]
fn a_new_doorbell_is_not_rung() {
    let bell = Doorbell::new().expect("create");
    assert!(!is_signalled(bell.handle()));
}

#[test]
fn ringing_stays_rung_until_it_is_reset() {
    let bell = Doorbell::new().expect("create");
    bell.ring();
    for _ in 0..5 {
        assert!(
            is_signalled(bell.handle()),
            "a manual-reset event is not consumed by a wait"
        );
    }
    bell.reset();
    assert!(!is_signalled(bell.handle()));
}

#[test]
fn ringing_twice_is_cleared_by_one_reset() {
    let bell = Doorbell::new().expect("create");
    bell.ring();
    bell.ring();
    bell.reset();
    assert!(!is_signalled(bell.handle()));
}

#[test]
fn resetting_an_unrung_doorbell_does_nothing() {
    let bell = Doorbell::new().expect("create");
    bell.reset();
    bell.reset();
    assert!(!is_signalled(bell.handle()));
    bell.ring();
    assert!(is_signalled(bell.handle()));
}

#[test]
fn ring_and_reset_alternate() {
    let bell = Doorbell::new().expect("create");
    for round in 0..200 {
        bell.ring();
        assert!(is_signalled(bell.handle()), "round {round}");
        bell.reset();
        assert!(!is_signalled(bell.handle()), "round {round}");
    }
}

#[test]
fn two_doorbells_are_two_events() {
    let first = Doorbell::new().expect("create");
    let second = Doorbell::new().expect("create");
    first.ring();
    assert!(is_signalled(first.handle()));
    assert!(!is_signalled(second.handle()));
    second.ring();
    first.reset();
    assert!(!is_signalled(first.handle()));
    assert!(is_signalled(second.handle()));
}

#[test]
fn a_wait_started_before_the_ring_is_released_by_it() {
    let bell = Doorbell::new().expect("create");
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| signalled_within(bell.handle(), 10_000));
        std::thread::sleep(Duration::from_millis(50));
        bell.ring();
        assert!(waiter.join().expect("waiter"));
    });
}

#[test]
fn one_ring_releases_every_waiter() {
    const WAITERS: usize = 4;
    let bell = Doorbell::new().expect("create");
    let released = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..WAITERS {
            scope.spawn(|| {
                if signalled_within(bell.handle(), 10_000) {
                    released.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
        std::thread::sleep(Duration::from_millis(100));
        bell.ring();
    });
    assert_eq!(released.load(Ordering::SeqCst), WAITERS);
}

#[test]
fn an_unrung_doorbell_times_a_wait_out() {
    let bell = Doorbell::new().expect("create");
    assert!(!signalled_within(bell.handle(), 30));
}

#[test]
fn it_crosses_threads() {
    fn send_sync<X: Send + Sync>() {}
    send_sync::<Doorbell>();
}

#[test]
fn a_new_watchers_doorbells_are_two_unrung_events() {
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    assert!(!is_signalled(watcher.doorbell()));
    assert!(!is_signalled(watcher.resume_doorbell()));
}

#[test]
fn the_crate_rings_one_and_the_caller_resets_it() {
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    watcher.ring_doorbell();
    assert!(is_signalled(watcher.doorbell()));
    assert!(!is_signalled(watcher.resume_doorbell()));
    watcher.reset_doorbell();
    assert!(!is_signalled(watcher.doorbell()));
}

#[test]
fn the_caller_rings_the_other_and_the_crate_resets_it() {
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    watcher.ring_resume();
    assert!(is_signalled(watcher.resume_doorbell()));
    assert!(!is_signalled(watcher.doorbell()));
    watcher.reset_resume();
    assert!(!is_signalled(watcher.resume_doorbell()));
}

#[test]
fn the_two_watchers_do_not_share_doorbells() {
    let first = Watcher::<u32>::new(4).expect("watcher");
    let second = Watcher::<u32>::new(4).expect("watcher");
    first.ring_doorbell();
    first.ring_resume();
    assert!(!is_signalled(second.doorbell()));
    assert!(!is_signalled(second.resume_doorbell()));
}

#[test]
fn a_caller_waiting_on_the_doorbell_is_woken_by_the_crate() {
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| signalled_within(watcher.doorbell(), 10_000));
        std::thread::sleep(Duration::from_millis(50));
        watcher.ring_doorbell();
        assert!(waiter.join().expect("waiter"));
    });
}

#[test]
fn a_bad_capacity_is_the_capacity_variant_and_a_good_one_creates_the_doorbells() {
    match Watcher::<u32>::new(3).expect_err("not a power of two") {
        CreateError::Capacity(error) => assert_eq!(error.requested, 3),
        CreateError::Io(error) => panic!("a capacity error, not {error}"),
    }
    assert!(Watcher::<u32>::new(2).is_ok());
}

#[test]
fn a_create_error_names_its_cause() {
    let error = Watcher::<u32>::new(0).expect_err("invalid");
    assert!(error.to_string().contains('0'));
    assert!(std::error::Error::source(&error).is_some());
    let io = CreateError::from(std::io::Error::from_raw_os_error(5));
    match &io {
        CreateError::Io(error) => assert_eq!(error.raw_os_error(), Some(5)),
        CreateError::Capacity(_) => panic!("an I/O error"),
    }
    assert!(std::error::Error::source(&io).is_some());
}
