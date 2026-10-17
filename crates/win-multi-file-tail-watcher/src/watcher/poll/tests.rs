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
fn a_poll_whose_poller_cannot_be_made_releases_the_gate_and_a_later_poll_goes_on() {
    let dir = TempDir::new("poll-no-poller");
    let watcher = watcher(4);
    let _registrations = changing(&dir, &watcher, 1);
    for _ in 0..2 {
        let failed = watcher.poll_with(|_| Err(PollError::Io(std::io::Error::other("no pool"))));
        assert!(matches!(failed, Err(PollError::Io(_))));
    }
    // Not InFlight, and not Shutdown: the gate was let go and the watcher is as it was.
    watcher.poll().expect("a poll after the failures");
    assert_eq!(values(&until_poll_done(&watcher)), [0]);
}

#[test]
fn poll_errors_say_what_happened() {
    assert_eq!(
        PollError::InFlight.to_string(),
        "a poll is already in flight"
    );
    assert_eq!(
        PollError::Shutdown.to_string(),
        "the watcher has been shut down"
    );
    assert!(std::error::Error::source(&PollError::Shutdown).is_none());
    let io = PollError::from(std::io::Error::from_raw_os_error(5));
    assert!(io.to_string().contains("what the poll runs on"));
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

#[test]
fn a_registration_dropped_while_its_poll_is_parked_is_skipped_when_it_resumes() {
    let dir = TempDir::new("poll-drop-mid");
    let watcher = watcher(2);
    let mut registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    assert_eq!(values(&next_entries(&watcher)), [0, 1]);
    drop(registrations.remove(3));
    watcher.ring_resume();
    let mut rest = driven_to_poll_done(&watcher);
    assert_eq!(rest.pop(), Some(SqEntry::PollDone));
    assert_eq!(values(&rest), [2, 4]);
}

#[test]
fn an_attention_written_for_a_registration_since_dropped_still_carries_its_value() {
    let dir = TempDir::new("poll-drop-after");
    let path = dir.file("a.log", b"0123456789");
    let tag = std::sync::Arc::new(());
    let watcher = Watcher::<std::sync::Arc<()>>::new(8).expect("watcher");
    let registrations: Vec<_> = (0..2)
        .map(|_| {
            watcher
                .register(&wtf(&path), 0, std::sync::Arc::clone(&tag))
                .expect("register")
        })
        .collect();
    watcher.poll().expect("poll");
    assert!(signalled_within(watcher.doorbell(), 10_000));
    drop(registrations);
    // The test's, and one for each attention in the queue; the registrations' are gone.
    assert_eq!(std::sync::Arc::strong_count(&tag), 3);
    let entries: Vec<_> = watcher.drain().collect();
    assert_eq!(entries.len(), 3, "two attentions and poll-done");
    drop(entries);
    assert_eq!(std::sync::Arc::strong_count(&tag), 1);
}

#[test]
fn a_registration_made_while_a_poll_is_parked_is_seen_by_the_next_poll_and_not_this_one() {
    let dir = TempDir::new("poll-add-mid");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 3);
    watcher.poll().expect("poll");
    assert_eq!(values(&next_entries(&watcher)), [0, 1]);
    let late = dir.file("late.log", &[b'y'; 40]);
    let _late = watcher.register(&wtf(&late), 0, 99).expect("register");
    watcher.ring_resume();
    let mut rest = driven_to_poll_done(&watcher);
    assert_eq!(rest.pop(), Some(SqEntry::PollDone));
    assert_eq!(
        values(&rest),
        [2],
        "this poll's list was fixed at its probe"
    );
    watcher.poll().expect("poll again");
    let mut next = driven_to_poll_done(&watcher);
    assert_eq!(next.pop(), Some(SqEntry::PollDone));
    assert_eq!(values(&next), [99]);
}

#[test]
fn registering_and_dropping_do_not_wait_for_a_parked_poll() {
    let dir = TempDir::new("poll-no-wait");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    next_entries(&watcher);
    let path = dir.file("extra.log", b"abc");
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for n in 0..50 {
                drop(watcher.register(&wtf(&path), 0, 100 + n).expect("register"));
            }
            done.send(()).expect("report");
        });
        finished
            .recv_timeout(Duration::from_secs(10))
            .expect("registering and dropping waited for the parked poll");
    });
}

#[test]
fn registrations_made_and_dropped_on_other_threads_while_polls_run_cause_no_failure() {
    const CHURNERS: u32 = 4;
    const EACH: u32 = 250;
    let dir = TempDir::new("poll-churn");
    let watcher = watcher(4);
    let _registrations = changing(&dir, &watcher, 20);
    let path = dir.file("churn.log", b"0123456789");
    let churning = std::sync::atomic::AtomicU32::new(CHURNERS);
    std::thread::scope(|scope| {
        for thread in 0..CHURNERS {
            let (watcher, path, churning) = (&watcher, &path, &churning);
            scope.spawn(move || {
                for n in 0..EACH {
                    let registration = watcher
                        .register(&wtf(path), 0, 1_000 + thread * EACH + n)
                        .expect("register");
                    drop(registration);
                }
                churning.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            });
        }
        let mut polls = 0;
        while churning.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            watcher.poll().expect("poll");
            let entries = driven_to_poll_done(&watcher);
            assert!(
                entries
                    .iter()
                    .all(|entry| matches!(entry, SqEntry::Attention(_) | SqEntry::PollDone))
            );
            polls += 1;
        }
        assert!(polls > 0);
    });
    assert_eq!(
        watcher.registered(),
        20,
        "only the original registrations remain"
    );
}

#[test]
fn a_poll_after_shutdown_is_refused_and_the_request_may_be_repeated() {
    let watcher = watcher(4);
    watcher.shutdown();
    watcher.shutdown();
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
    watcher.shutdown();
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
}

#[test]
fn shutdown_is_refused_ahead_of_in_flight_when_a_poll_is_parked() {
    let dir = TempDir::new("poll-shutdown-parked");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    next_entries(&watcher);
    assert!(matches!(watcher.poll(), Err(PollError::InFlight)));
    watcher.shutdown();
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
}

#[test]
fn a_shutdown_leaves_the_watcher_usable_for_everything_but_polling() {
    let dir = TempDir::new("poll-shutdown-usable");
    let watcher = watcher(4);
    let registrations = changing(&dir, &watcher, 2);
    watcher.shutdown();
    let path = dir.file("late.log", b"abc");
    let late = watcher.register(&wtf(&path), 0, 7).expect("register");
    assert_eq!(watcher.registered(), 3);
    assert_eq!(late.value(), 7);
    drop(registrations);
    assert_eq!(watcher.registered(), 1);
    assert_eq!(watcher.drain().count(), 0);
}

#[test]
fn a_shutdown_from_another_thread_ends_a_loop_of_polls() {
    let dir = TempDir::new("poll-shutdown-loop");
    let watcher = watcher(4);
    let _registrations = changing(&dir, &watcher, 3);
    let polled = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(30));
            watcher.shutdown();
        });
        let mut polled = 0;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match watcher.poll() {
                Ok(()) => {
                    until_poll_done(&watcher);
                    polled += 1;
                }
                Err(PollError::Shutdown) => break polled,
                Err(error) => panic!("{error}"),
            }
            assert!(Instant::now() < deadline, "the shutdown never took effect");
        }
    });
    assert!(polled > 0, "some polls ran before the request");
}

#[test]
fn a_shutdown_stops_a_parked_poll_at_its_next_step_and_it_writes_poll_done() {
    let dir = TempDir::new("poll-stop-parked");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 6);
    watcher.poll().expect("poll");
    assert_eq!(values(&next_entries(&watcher)), [0, 1]);
    watcher.shutdown();
    watcher.ring_resume();
    assert_eq!(
        next_entries(&watcher),
        [SqEntry::PollDone],
        "the rest of the changes were abandoned"
    );
}

#[test]
fn after_a_shutdown_a_poll_done_the_queue_refuses_is_written_once_the_caller_drains_and_rings() {
    let dir = TempDir::new("poll-stop-refused");
    let watcher = watcher(2);
    let _registrations = changing(&dir, &watcher, 6);
    watcher.poll().expect("poll");
    // The queue is full and nobody has drained it.
    assert!(rung(&watcher));
    watcher.reset_doorbell();
    watcher.shutdown();
    // Each ring finds the queue still full, and the poll stays parked for the next one.
    for _ in 0..3 {
        watcher.ring_resume();
        assert!(rung(&watcher), "the resumed callback ran");
        watcher.reset_doorbell();
    }
    assert_eq!(values(&watcher.drain().collect::<Vec<_>>()), [0, 1]);
    watcher.ring_resume();
    assert_eq!(
        next_entries(&watcher),
        [SqEntry::PollDone],
        "poll-done is always delivered"
    );
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
}

#[test]
fn dropping_a_watcher_sets_the_stop_flag_whether_or_not_a_shutdown_was_made() {
    let dir = TempDir::new("poll-drop-flag");
    for shut_down_first in [false, true] {
        let watcher = watcher(4);
        let registrations = changing(&dir, &watcher, 1);
        if shut_down_first {
            watcher.shutdown();
        }
        assert_eq!(registrations[0].shared.stopping(), shut_down_first);
        drop(watcher);
        assert!(registrations[0].shared.stopping());
    }
}

#[test]
fn dropping_a_callbacks_view_of_the_watcher_stops_nothing() {
    // What a poller that could not be made leaves behind to drop: the view its callbacks ran on.
    let dir = TempDir::new("poll-drop-view");
    let watcher = watcher(4);
    let _registrations = changing(&dir, &watcher, 1);
    drop(Watcher::view(std::sync::Arc::clone(&watcher.shared)));
    assert!(!watcher.shared.stopping());
    watcher.poll().expect("a poll after the view was dropped");
    assert_eq!(values(&until_poll_done(&watcher)), [0]);
}

#[test]
fn after_a_watcher_with_a_parked_poll_is_dropped_no_callback_of_the_crate_runs() {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::System::Threading::SetEvent;

    let dir = TempDir::new("poll-drop-quiet");
    let watcher = watcher(2);
    let registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    assert_eq!(values(&next_entries(&watcher)), [0, 1]);
    // The events outlive the watcher through duplicates, so what runs after it can be seen.
    let doorbell = watcher.doorbell().try_clone_to_owned().expect("duplicate");
    let resume = watcher
        .resume_doorbell()
        .try_clone_to_owned()
        .expect("duplicate");

    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(watcher);
        done.send(()).expect("report");
    });
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("dropping the watcher waited for a poll that was only parked");

    // SAFETY: a live event, owned by `resume` for the duration of the call.
    unsafe { SetEvent(resume.as_raw_handle()) };
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !signalled_within(doorbell.as_handle(), 0),
        "a callback ran after the watcher was dropped"
    );
    drop(registrations);
}

// A value whose clone waits for `release`, so a poll can be held inside the report.
struct Gate {
    state: std::sync::Mutex<(bool, bool)>,
    changed: std::sync::Condvar,
}

impl Gate {
    fn wait_until(&self, done: impl Fn(&(bool, bool)) -> bool) {
        let mut state = self.state.lock().expect("gate");
        while !done(&state) {
            state = self.changed.wait(state).expect("gate");
        }
    }

    fn set(&self, change: impl FnOnce(&mut (bool, bool))) {
        change(&mut self.state.lock().expect("gate"));
        self.changed.notify_all();
    }
}

struct Blocker(std::sync::Arc<Gate>);

impl Clone for Blocker {
    fn clone(&self) -> Self {
        self.0.set(|state| state.0 = true);
        self.0.wait_until(|state| state.1);
        Self(std::sync::Arc::clone(&self.0))
    }
}

#[test]
fn dropping_a_watcher_waits_for_a_poll_that_is_running_and_returns_when_it_does() {
    let dir = TempDir::new("poll-drop-waits");
    let gate = std::sync::Arc::new(Gate {
        state: std::sync::Mutex::new((false, false)),
        changed: std::sync::Condvar::new(),
    });
    let path = dir.file("a.log", b"0123456789");
    let watcher = Watcher::<Blocker>::new(4).expect("watcher");
    let _registration = watcher
        .register(&wtf(&path), 0, Blocker(std::sync::Arc::clone(&gate)))
        .expect("register");
    watcher.poll().expect("poll");
    // The poll is inside `T::clone`, holding the roster lock.
    gate.wait_until(|state| state.0);

    let (done, finished) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(watcher);
        done.send(()).expect("report");
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(200)).is_err(),
        "the drop returned while a poll was running"
    );
    gate.set(|state| state.1 = true);
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the drop did not return once the poll did");
    dropper.join().expect("dropper");
}
