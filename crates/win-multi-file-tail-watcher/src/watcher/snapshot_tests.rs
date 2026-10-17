// Copyright (c) 2026 Mike Grier
use std::sync::atomic::{AtomicBool, Ordering};

use super::{SnapshotItem, Watcher};
use crate::Registration;
use crate::chunked::Chunked;
use crate::test_support::{TempDir, wtf};

fn watcher() -> Watcher<u32> {
    Watcher::new(64).expect("a valid SQ capacity")
}

fn snapshot_of(watcher: &Watcher<u32>, batch: usize) -> Chunked<SnapshotItem> {
    let mut out = Chunked::new();
    watcher.snapshot_batches(&mut out, batch, || {});
    out
}

fn ids(buffer: &Chunked<SnapshotItem>) -> Vec<u64> {
    buffer.iter().map(|item| item.id).collect()
}

/// `count` registrations of files whose lengths are `base`, `base + 1`, and so on.
fn register_files(
    dir: &TempDir,
    watcher: &Watcher<u32>,
    count: usize,
    base: usize,
) -> Vec<Registration<u32>> {
    (0..count)
        .map(|n| {
            let path = dir.file(format!("f{n}.log"), &vec![b'x'; base + n]);
            watcher
                .register(&wtf(&path), 0, u32::try_from(n).expect("small"))
                .expect("register")
        })
        .collect()
}

#[test]
fn a_registration_starts_with_its_starting_offset_as_its_last_known_length() {
    let dir = TempDir::new("s-offset");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    for offset in [0u64, 1, 4, 9, 10] {
        let registration = watcher.register(&wtf(&path), offset, 0).expect("register");
        assert_eq!(registration.last_known(), offset);
    }
}

#[test]
fn an_empty_watcher_snapshots_nothing() {
    let out = snapshot_of(&watcher(), 4);
    assert!(out.is_empty());
}

#[test]
fn every_registration_is_copied_in_registration_order_with_the_right_observer() {
    let dir = TempDir::new("s-all");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 40, 100);
    let out = snapshot_of(&watcher, 7);
    assert_eq!(out.len(), 40);
    let found = ids(&out);
    assert!(found.windows(2).all(|pair| pair[0] < pair[1]), "{found:?}");
    for (n, item) in out.iter().enumerate() {
        assert_eq!(item.observer.file_len().expect("len"), 100 + n as u64);
        assert_eq!(item.last_known, 0);
    }
    assert_eq!(registrations.len(), 40);
}

#[test]
fn the_copied_last_known_length_is_each_registrations_own() {
    let dir = TempDir::new("s-last");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    let offsets = [3u64, 0, 10, 7];
    let _registrations: Vec<_> = offsets
        .iter()
        .map(|&offset| watcher.register(&wtf(&path), offset, 0).expect("register"))
        .collect();
    let out = snapshot_of(&watcher, 2);
    let copied: Vec<_> = out.iter().map(|item| item.last_known).collect();
    assert_eq!(copied, offsets);
}

#[test]
fn the_batch_size_does_not_change_what_is_copied() {
    let dir = TempDir::new("s-batch");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 23, 10);
    let expected = ids(&snapshot_of(&watcher, 1000));
    assert_eq!(expected.len(), 23);
    for batch in [1usize, 2, 3, 7, 22, 23, 24, 256] {
        let out = snapshot_of(&watcher, batch);
        assert_eq!(ids(&out), expected, "batch {batch}");
        for (n, item) in out.iter().enumerate() {
            assert_eq!(item.observer.file_len().expect("len"), 10 + n as u64);
        }
    }
}

#[test]
fn the_lock_is_taken_once_per_batch_and_released_between_them() {
    let dir = TempDir::new("s-hook");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 7, 1);
    for (batch, expected_pauses) in [(1usize, 7), (2, 3), (3, 2), (6, 1), (7, 1), (8, 0)] {
        let mut pauses = 0;
        let mut out = Chunked::new();
        watcher.snapshot_batches(&mut out, batch, || {
            pauses += 1;
            assert!(
                watcher.shared.roster.try_lock().is_ok(),
                "the roster lock is held across the pause"
            );
            assert_eq!(watcher.registered(), 7);
        });
        assert_eq!(pauses, expected_pauses, "batch {batch}");
        assert_eq!(out.len(), 7, "batch {batch}");
    }
}

#[test]
fn a_registration_dropped_before_the_pass_reaches_it_is_skipped() {
    let dir = TempDir::new("s-skip");
    let watcher = watcher();
    let mut registrations = register_files(&dir, &watcher, 6, 1);
    let ids_before = ids(&snapshot_of(&watcher, 100));
    let mut later = Some(registrations.remove(3));
    let mut out = Chunked::new();
    watcher.snapshot_batches(&mut out, 1, || drop(later.take()));
    let copied = ids(&out);
    assert_eq!(copied.len(), 5);
    assert!(!copied.contains(&ids_before[3]));
}

#[test]
fn a_registration_dropped_after_it_was_copied_still_has_a_working_observer() {
    let dir = TempDir::new("s-after");
    let watcher = watcher();
    let mut registrations = register_files(&dir, &watcher, 4, 50);
    let mut first = Some(registrations.remove(0));
    let mut out = Chunked::new();
    watcher.snapshot_batches(&mut out, 1, || drop(first.take()));
    assert_eq!(out.len(), 4);
    let item = out
        .get(0)
        .expect("the first was copied before it was dropped");
    assert_eq!(item.observer.file_len().expect("len"), 50);
    assert_eq!(watcher.registered(), 3);
}

#[test]
fn a_registration_made_while_the_pass_runs_is_seen_if_the_pass_has_not_ended() {
    let dir = TempDir::new("s-add");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 4, 1);
    let path = dir.file("late.log", b"late");
    let mut late = None;
    let mut out = Chunked::new();
    watcher.snapshot_batches(&mut out, 2, || {
        if late.is_none() {
            late = Some(watcher.register(&wtf(&path), 0, 99).expect("register"));
        }
    });
    assert_eq!(out.len(), 5);
    let last = out.get(4).expect("the late registration");
    assert_eq!(last.observer.file_len().expect("len"), 4);
}

#[test]
fn a_registration_made_after_the_pass_is_in_the_next_one_and_not_this() {
    let dir = TempDir::new("s-late");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 3, 1);
    let first = snapshot_of(&watcher, 8);
    let path = dir.file("late.log", b"late");
    let _late = watcher.register(&wtf(&path), 0, 99).expect("register");
    let second = snapshot_of(&watcher, 8);
    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 4);
    assert_eq!(&ids(&second)[..3], ids(&first).as_slice());
}

#[test]
fn a_snapshot_appends_and_changes_nothing_in_the_roster() {
    let dir = TempDir::new("s-append");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 5, 1);
    let mut out = Chunked::new();
    watcher.snapshot_batches(&mut out, 2, || {});
    watcher.snapshot_batches(&mut out, 2, || {});
    assert_eq!(out.len(), 10);
    assert_eq!(watcher.registered(), 5);
    assert!(registrations.iter().all(|r| r.last_known() == 0));
}

#[test]
fn one_path_registered_twice_is_two_items_with_their_own_handles() {
    let dir = TempDir::new("s-twice");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let _first = watcher.register(&wtf(&path), 0, 1).expect("first");
    let _second = watcher.register(&wtf(&path), 0, 2).expect("second");
    let out = snapshot_of(&watcher, 8);
    let items: Vec<_> = out.iter().collect();
    assert_eq!(items.len(), 2);
    assert_ne!(items[0].id, items[1].id);
    assert!(!items[0].observer.same_handle(&items[1].observer));
}

#[test]
fn an_observer_outlives_its_registration_and_its_watcher() {
    let dir = TempDir::new("s-outlive");
    let path = dir.file("a.log", b"hello");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 0, 1).expect("register");
    let out = snapshot_of(&watcher, 8);
    drop(registration);
    drop(watcher);
    assert_eq!(
        out.get(0).expect("item").observer.file_len().expect("len"),
        5
    );
}

#[test]
fn the_default_batch_covers_more_than_one_acquisition() {
    let dir = TempDir::new("s-default");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let registrations: Vec<_> = (0..600)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    let mut out = Chunked::new();
    watcher.snapshot(&mut out);
    assert_eq!(out.len(), registrations.len());
    assert_eq!(ids(&out), ids(&snapshot_of(&watcher, 1)));
}

#[test]
fn a_snapshot_entry_is_the_size_the_buffer_budget_assumes() {
    assert_eq!(size_of::<SnapshotItem>(), 24);
}

// Stops the churning thread when the test ends any way at all, so a failed assertion fails the
// test instead of leaving the thread scope waiting for a thread that never stops.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[test]
fn registrations_and_drops_on_other_threads_never_break_a_snapshot() {
    let dir = TempDir::new("s-churn");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    let _anchored = register_files(&dir, &watcher, 20, 10);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let _stop = StopOnDrop(&stop);
        scope.spawn(|| {
            let mut kept = Vec::new();
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) {
                kept.push(watcher.register(&wtf(&path), 0, n).expect("register"));
                if kept.len() > 30 {
                    drop(kept.remove(n as usize % 30));
                }
                n += 1;
            }
        });
        for round in 0..100 {
            let out = snapshot_of(&watcher, 4);
            let found = ids(&out);
            assert!(
                found.windows(2).all(|pair| pair[0] < pair[1]),
                "round {round}"
            );
            assert!(found.len() >= 20, "round {round}");
            for item in out.iter() {
                assert!(item.observer.file_len().expect("len") >= 10);
                assert_eq!(item.last_known, 0);
            }
        }
    });
}

#[test]
fn a_new_registration_is_not_terminal() {
    let dir = TempDir::new("t-new");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 3, 1);
    assert!(registrations.iter().all(|r| r.terminal_code().is_none()));
}

#[test]
fn marking_sets_the_code_once_and_the_first_code_stays() {
    let dir = TempDir::new("t-once");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 2, 1);
    let id = registrations[0].id();
    assert!(watcher.mark_terminal(id, 6));
    assert_eq!(registrations[0].terminal_code(), Some(6));
    assert!(!watcher.mark_terminal(id, 1006));
    assert_eq!(registrations[0].terminal_code(), Some(6));
    assert_eq!(registrations[1].terminal_code(), None);
}

#[test]
fn marking_a_registration_that_is_not_there_does_nothing() {
    let dir = TempDir::new("t-gone");
    let watcher = watcher();
    let mut registrations = register_files(&dir, &watcher, 2, 1);
    let dropped = registrations.remove(0);
    let id = dropped.id();
    drop(dropped);
    assert!(!watcher.mark_terminal(id, 6));
    assert!(!watcher.mark_terminal(u64::MAX, 6));
    assert_eq!(registrations[0].terminal_code(), None);
}

#[test]
fn a_terminal_registration_is_not_snapshotted_and_the_others_are() {
    let dir = TempDir::new("t-skip");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 6, 1);
    let skipped = [registrations[1].id(), registrations[4].id()];
    for id in skipped {
        assert!(watcher.mark_terminal(id, 6));
    }
    let found = ids(&snapshot_of(&watcher, 100));
    let expected: Vec<_> = registrations
        .iter()
        .map(crate::Registration::id)
        .filter(|id| !skipped.contains(id))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn a_run_of_terminal_registrations_does_not_end_the_pass_early() {
    let dir = TempDir::new("t-run");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 10, 1);
    for registration in &registrations[..5] {
        assert!(watcher.mark_terminal(registration.id(), 6));
    }
    let expected: Vec<_> = registrations[5..]
        .iter()
        .map(crate::Registration::id)
        .collect();
    for batch in 1..=12 {
        assert_eq!(
            ids(&snapshot_of(&watcher, batch)),
            expected,
            "batch {batch}"
        );
    }
}

#[test]
fn terminal_registrations_between_live_ones_do_not_end_the_pass_early() {
    let dir = TempDir::new("t-between");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 12, 1);
    for registration in registrations.iter().skip(1).step_by(3) {
        assert!(watcher.mark_terminal(registration.id(), 6));
    }
    let expected: Vec<_> = registrations
        .iter()
        .enumerate()
        .filter(|(n, _)| n % 3 != 1)
        .map(|(_, r)| r.id())
        .collect();
    for batch in 1..=13 {
        assert_eq!(
            ids(&snapshot_of(&watcher, batch)),
            expected,
            "batch {batch}"
        );
    }
}

#[test]
fn when_every_registration_is_terminal_the_snapshot_is_empty() {
    let dir = TempDir::new("t-all");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 7, 1);
    for registration in &registrations {
        assert!(watcher.mark_terminal(registration.id(), 1006));
    }
    for batch in [1, 2, 7, 8, 256] {
        assert!(snapshot_of(&watcher, batch).is_empty(), "batch {batch}");
    }
}

#[test]
fn a_terminal_registration_stays_registered_until_it_is_dropped() {
    let dir = TempDir::new("t-registered");
    let watcher = watcher();
    let mut registrations = register_files(&dir, &watcher, 3, 1);
    assert!(watcher.mark_terminal(registrations[0].id(), 6));
    assert_eq!(watcher.registered(), 3);
    assert_eq!(registrations[0].value(), 0);
    drop(registrations.remove(0));
    assert_eq!(watcher.registered(), 2);
}

#[test]
fn marking_does_not_change_the_last_known_length() {
    let dir = TempDir::new("t-last");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 4, 0).expect("register");
    assert!(watcher.mark_terminal(registration.id(), 6));
    assert_eq!(registration.last_known(), 4);
}

#[test]
fn a_terminal_registration_stays_out_of_every_later_snapshot() {
    let dir = TempDir::new("t-stays");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 4, 1);
    assert!(watcher.mark_terminal(registrations[2].id(), 6));
    for _ in 0..3 {
        let found = ids(&snapshot_of(&watcher, 2));
        assert_eq!(found.len(), 3);
        assert!(!found.contains(&registrations[2].id()));
    }
}

#[test]
fn an_entry_marked_between_batches_is_skipped_if_not_yet_reached_and_kept_if_copied() {
    let dir = TempDir::new("t-hook");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 6, 1);
    let (copied, ahead) = (registrations[0].id(), registrations[4].id());
    let mut out = Chunked::new();
    watcher.snapshot_batches(&mut out, 2, || {
        watcher.mark_terminal(copied, 6);
        watcher.mark_terminal(ahead, 6);
    });
    let found = ids(&out);
    assert!(found.contains(&copied));
    assert!(!found.contains(&ahead));
    assert_eq!(found.len(), 5);
}

#[test]
fn many_threads_marking_the_same_entries_mark_each_exactly_once() {
    let dir = TempDir::new("t-race");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 50, 1);
    let marked = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for thread in 0..8u32 {
            let (watcher, registrations, marked) = (&watcher, &registrations, &marked);
            scope.spawn(move || {
                for registration in registrations {
                    if watcher.mark_terminal(registration.id(), 6 + thread) {
                        marked.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    assert_eq!(marked.load(Ordering::SeqCst), 50);
    assert!(registrations.iter().all(|r| r.terminal_code().is_some()));
    assert!(snapshot_of(&watcher, 7).is_empty());
}
