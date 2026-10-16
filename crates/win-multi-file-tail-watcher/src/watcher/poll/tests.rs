// Copyright (c) 2026 Mike Grier
use std::time::{Duration, Instant};

use crate::test_support::{TempDir, signalled_within, wtf};
use crate::{PollError, Registration, SqEntry, Watcher};

fn watcher(capacity: usize) -> Watcher<u32> {
    Watcher::new(capacity).expect("a valid SQ capacity")
}

/// `count` registered files, each of `10 + n` bytes registered at offset 0, so each is a change.
fn changing(dir: &TempDir, watcher: &Watcher<u32>, count: u32) -> Vec<Registration<u32>> {
    (0..count)
        .map(|n| {
            let path = dir.file(format!("f{n}.log"), &vec![b'x'; 10 + n as usize]);
            watcher.register(&wtf(&path), 0, n).expect("register")
        })
        .collect()
}

fn rung(watcher: &Watcher<u32>) -> bool {
    signalled_within(watcher.doorbell(), 10_000)
}

// Wait for the doorbell, reset it first, then read what it announced.
fn next_entries(watcher: &Watcher<u32>) -> Vec<SqEntry<u32>> {
    assert!(rung(watcher), "the doorbell was not rung");
    watcher.reset_doorbell();
    watcher.drain().collect()
}

// Entries until poll-done has been read.
fn until_poll_done(watcher: &Watcher<u32>) -> Vec<SqEntry<u32>> {
    let mut all = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        all.extend(next_entries(watcher));
        if all.iter().any(|entry| matches!(entry, SqEntry::PollDone)) {
            return all;
        }
        assert!(Instant::now() < deadline, "poll-done never came");
    }
}

fn values(entries: &[SqEntry<u32>]) -> Vec<u32> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            SqEntry::Attention(a) => Some(a.value),
            _ => None,
        })
        .collect()
}

#[test]
fn a_poll_of_an_empty_watcher_rings_the_doorbell_and_writes_only_poll_done() {
    let watcher = watcher(4);
    watcher.poll().expect("poll");
    assert_eq!(next_entries(&watcher), [SqEntry::PollDone]);
}

#[test]
fn a_poll_reports_each_change_then_poll_done() {
    let dir = TempDir::new("poll-changes");
    let watcher = watcher(16);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    let entries = until_poll_done(&watcher);
    assert_eq!(values(&entries), [0, 1, 2, 3, 4]);
    assert_eq!(entries.last(), Some(&SqEntry::PollDone));
}

#[test]
fn a_second_poll_reports_only_what_changed_since() {
    let dir = TempDir::new("poll-since");
    let watcher = watcher(16);
    let _registrations = changing(&dir, &watcher, 3);
    watcher.poll().expect("poll");
    until_poll_done(&watcher);
    watcher.poll().expect("poll again");
    assert_eq!(until_poll_done(&watcher), [SqEntry::PollDone]);
}

#[test]
fn a_poll_that_stopped_on_a_full_queue_is_still_in_flight() {
    let dir = TempDir::new("poll-parked");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    let entries = next_entries(&watcher);
    assert_eq!(values(&entries), [0, 1], "the queue held two");
    assert!(
        !entries.contains(&SqEntry::PollDone),
        "poll-done needs room too"
    );
    assert!(matches!(watcher.poll(), Err(PollError::InFlight)));
    assert!(matches!(watcher.poll(), Err(PollError::InFlight)));
}

#[test]
fn of_many_threads_polling_a_stopped_poll_only_the_first_is_accepted() {
    let dir = TempDir::new("poll-storm");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 6);
    let accepted = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| watcher.poll().is_ok()))
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a poller"))
            .filter(|&accepted| accepted)
            .count()
    });
    assert_eq!(accepted, 1);
}

#[test]
fn a_caller_who_has_read_poll_done_may_poll_again_at_once() {
    let dir = TempDir::new("poll-again");
    let watcher = watcher(4);
    let _registrations = changing(&dir, &watcher, 1);
    for round in 0..300 {
        watcher.poll().unwrap_or_else(|error| {
            panic!("round {round}: refused right after poll-done: {error}")
        });
        until_poll_done(&watcher);
    }
}

#[test]
fn a_watcher_polled_and_dropped_at_once_neither_hangs_nor_crashes() {
    let dir = TempDir::new("poll-drop");
    for _ in 0..20 {
        let watcher = watcher(4);
        let _registrations = changing(&dir, &watcher, 3);
        watcher.poll().expect("poll");
        drop(watcher);
    }
}

#[test]
fn a_registration_outliving_its_polled_watcher_is_usable() {
    let dir = TempDir::new("poll-outlive");
    let watcher = watcher(4);
    let registrations = changing(&dir, &watcher, 1);
    watcher.poll().expect("poll");
    until_poll_done(&watcher);
    drop(watcher);
    assert_eq!(registrations[0].value(), 0);
}

#[test]
fn poll_errors_say_what_happened() {
    assert_eq!(
        PollError::InFlight.to_string(),
        "a poll is already in flight"
    );
    let io = PollError::from(std::io::Error::from_raw_os_error(5));
    assert!(io.to_string().contains("thread-pool work"));
    assert!(std::error::Error::source(&io).is_some());
    assert!(std::error::Error::source(&PollError::InFlight).is_none());
}

#[test]
fn a_poll_whose_poll_done_the_queue_refuses_is_still_in_flight() {
    let dir = TempDir::new("poll-done-refused");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 2);
    watcher.poll().expect("poll");
    let entries = next_entries(&watcher);
    assert_eq!(values(&entries), [0, 1]);
    assert!(!entries.contains(&SqEntry::PollDone), "there was no room");
    assert!(matches!(watcher.poll(), Err(PollError::InFlight)));
}

#[test]
fn the_gate_makes_a_begin_that_finds_it_finishing_wait_for_the_outcome() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::PollGate;

    // `begin` is asked while poll-done is being written, and is answered only when the write is.
    fn begin_during_the_write(outcome: impl FnOnce(&PollGate)) -> bool {
        let gate = PollGate::new();
        assert!(gate.begin());
        gate.finishing();
        let asking = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                asking.store(true, Ordering::SeqCst);
                gate.begin()
            });
            while !asking.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(50));
            outcome(&gate);
            waiter.join().expect("a waiter")
        })
    }

    assert!(
        begin_during_the_write(PollGate::finished),
        "a written poll-done opens the gate"
    );
    assert!(
        !begin_during_the_write(PollGate::unfinished),
        "a refused poll-done leaves the poll in flight"
    );
}

#[test]
fn a_reader_who_polls_the_instant_poll_done_is_visible_is_never_refused() {
    let dir = TempDir::new("poll-instant");
    let watcher = watcher(4);
    let _registrations = changing(&dir, &watcher, 1);
    let deadline = Instant::now() + Duration::from_secs(60);
    for round in 0..2_000 {
        watcher
            .poll()
            .unwrap_or_else(|error| panic!("round {round}: {error}"));
        while !watcher
            .drain()
            .any(|entry| matches!(entry, SqEntry::PollDone))
        {
            assert!(Instant::now() < deadline, "poll-done never came");
        }
    }
}

// Entries until poll-done has been read, ringing the resume doorbell after each drain.
fn driven_to_poll_done(watcher: &Watcher<u32>) -> Vec<SqEntry<u32>> {
    let mut all = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        all.extend(next_entries(watcher));
        if all.iter().any(|entry| matches!(entry, SqEntry::PollDone)) {
            return all;
        }
        watcher.ring_resume();
        assert!(Instant::now() < deadline, "the poll never finished");
    }
}

#[test]
fn a_parked_poll_continues_each_time_the_caller_rings_the_resume_doorbell() {
    let dir = TempDir::new("poll-resume");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    assert_eq!(values(&next_entries(&watcher)), [0, 1]);
    watcher.ring_resume();
    assert_eq!(values(&next_entries(&watcher)), [2, 3]);
    watcher.ring_resume();
    let last = next_entries(&watcher);
    assert_eq!(last, [att(4, 0, 14), SqEntry::PollDone]);
    watcher.poll().expect("a finished poll is not in flight");
}

#[test]
fn six_hundred_changes_through_a_queue_of_sixty_four_are_each_reported_once_in_order() {
    let dir = TempDir::new("poll-many");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher(64);
    let _registrations: Vec<_> = (0..600u32)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    watcher.poll().expect("poll");
    let mut entries = driven_to_poll_done(&watcher);
    assert_eq!(entries.pop(), Some(SqEntry::PollDone));
    assert_eq!(values(&entries), (0..600).collect::<Vec<u32>>());
}

#[test]
fn the_resumed_callback_clears_the_resume_doorbell() {
    let dir = TempDir::new("poll-reset");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    next_entries(&watcher);
    watcher.ring_resume();
    next_entries(&watcher);
    assert!(
        !signalled_within(watcher.resume_doorbell(), 0),
        "the resume doorbell was left rung"
    );
}

#[test]
fn a_ring_made_before_the_poll_parks_is_not_lost() {
    let dir = TempDir::new("poll-early");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.ring_resume();
    watcher.poll().expect("poll");
    let mut entries = driven_to_poll_done(&watcher);
    assert_eq!(entries.pop(), Some(SqEntry::PollDone));
    assert_eq!(values(&entries), [0, 1, 2, 3, 4]);
}

#[test]
fn a_ring_between_polls_starts_nothing() {
    let dir = TempDir::new("poll-stray");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 2);
    watcher.poll().expect("poll");
    let mut entries = driven_to_poll_done(&watcher);
    assert_eq!(entries.pop(), Some(SqEntry::PollDone));
    watcher.ring_resume();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(watcher.drain().count(), 0, "a ring started a poll");
    watcher.poll().expect("and the next poll is accepted");
}

#[test]
fn a_watcher_dropped_while_its_poll_is_parked_neither_hangs_nor_crashes() {
    let dir = TempDir::new("poll-drop-parked");
    for _ in 0..20 {
        let watcher = watcher(2);
        let _registrations = changing(&dir, &watcher, 5);
        watcher.poll().expect("poll");
        next_entries(&watcher);
        drop(watcher);
    }
}

#[test]
fn a_watcher_dropped_as_its_poll_resumes_neither_hangs_nor_crashes() {
    let dir = TempDir::new("poll-drop-resuming");
    for _ in 0..20 {
        let watcher = watcher(2);
        let _registrations = changing(&dir, &watcher, 5);
        watcher.poll().expect("poll");
        next_entries(&watcher);
        watcher.ring_resume();
        drop(watcher);
    }
}

fn att(value: u32, last_known: u64, current: u64) -> SqEntry<u32> {
    SqEntry::Attention(crate::Attention {
        value,
        last_known,
        current,
    })
}
