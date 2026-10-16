// Copyright (c) 2026 Mike Grier
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_overlapped_io_sys::EndpointObserver;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE};

use super::{Engine, RunOutcome};
use crate::test_support::{RecordingSink, TempDir, wtf};
use crate::{Attention, Diagnostic, Registration, SqEntry, Terminal, Watcher};

fn watcher(capacity: usize) -> Watcher<u32> {
    Watcher::new(capacity).expect("a valid SQ capacity")
}

struct Files {
    registrations: Vec<Registration<u32>>,
    paths: Vec<PathBuf>,
}

/// `count` files of `base`, `base + 1`, ... bytes, each registered at offset 0, so each is a change
/// of `0` to its length.
fn register_files(dir: &TempDir, watcher: &Watcher<u32>, count: usize, base: usize) -> Files {
    let mut files = Files {
        registrations: Vec::new(),
        paths: Vec::new(),
    };
    for n in 0..count {
        let path = dir.file(format!("f{n}.log"), &vec![b'x'; base + n]);
        files.registrations.push(
            watcher
                .register(&wtf(&path), 0, u32::try_from(n).expect("small"))
                .expect("register"),
        );
        files.paths.push(path);
    }
    files
}

fn append(path: &PathBuf, bytes: usize) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(&vec![b'y'; bytes]))
        .expect("append");
}

fn drained(watcher: &Watcher<u32>) -> Vec<SqEntry<u32>> {
    watcher.drain().collect()
}

fn attention(value: u32, last_known: u64, current: u64) -> SqEntry<u32> {
    SqEntry::Attention(Attention {
        value,
        last_known,
        current,
    })
}

fn terminal(value: u32, win32_code: u32, last_known: u64) -> SqEntry<u32> {
    SqEntry::Terminal(Terminal {
        value,
        win32_code,
        last_known,
    })
}

fn win32(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code.cast_signed())
}

#[test]
fn an_empty_watcher_completes_with_only_poll_done() {
    let watcher = watcher(4);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
}

#[test]
fn registrations_at_their_length_produce_only_poll_done() {
    let dir = TempDir::new("e-same");
    let watcher = watcher(8);
    let path = dir.file("a.log", b"abcdef");
    let _registrations: Vec<_> = (0..4)
        .map(|n| watcher.register(&wtf(&path), 6, n).expect("register"))
        .collect();
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
}

#[test]
fn changes_are_attentions_in_registration_order_then_poll_done() {
    let dir = TempDir::new("e-order");
    let watcher = watcher(16);
    let _files = register_files(&dir, &watcher, 4, 10);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(
        drained(&watcher),
        [
            attention(0, 0, 10),
            attention(1, 0, 11),
            attention(2, 0, 12),
            attention(3, 0, 13),
            SqEntry::PollDone,
        ]
    );
}

#[test]
fn a_second_run_repeats_nothing_and_leaves_the_last_known_length_current() {
    let dir = TempDir::new("e-repeat");
    let watcher = watcher(16);
    let files = register_files(&dir, &watcher, 3, 5);
    let mut engine = Engine::new(&watcher);
    engine.run(&watcher);
    drained(&watcher);
    for (n, registration) in files.registrations.iter().enumerate() {
        assert_eq!(registration.last_known(), 5 + n as u64);
    }
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
}

#[test]
fn growth_between_polls_is_reported_from_the_length_last_reported() {
    let dir = TempDir::new("e-grow");
    let watcher = watcher(16);
    let files = register_files(&dir, &watcher, 2, 10);
    let mut engine = Engine::new(&watcher);
    engine.run(&watcher);
    drained(&watcher);
    append(&files.paths[1], 7);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [attention(1, 11, 18), SqEntry::PollDone]);
}

#[test]
fn a_shrink_is_reported_as_the_smaller_length() {
    let dir = TempDir::new("e-shrink");
    let watcher = watcher(16);
    let files = register_files(&dir, &watcher, 1, 100);
    let mut engine = Engine::new(&watcher);
    engine.run(&watcher);
    drained(&watcher);
    OpenOptions::new()
        .write(true)
        .open(&files.paths[0])
        .expect("writer")
        .set_len(30)
        .expect("shrink");
    engine.run(&watcher);
    assert_eq!(
        drained(&watcher),
        [attention(0, 100, 30), SqEntry::PollDone]
    );
}

#[test]
fn a_full_sq_stops_the_pass_and_each_run_continues_from_the_refused_change() {
    let dir = TempDir::new("e-stop");
    let watcher = watcher(2);
    let files = register_files(&dir, &watcher, 5, 10);
    let mut engine = Engine::new(&watcher);

    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    assert_eq!(
        drained(&watcher),
        [attention(0, 0, 10), attention(1, 0, 11)]
    );
    assert_eq!(files.registrations[1].last_known(), 11);
    assert_eq!(
        files.registrations[2].last_known(),
        0,
        "refused: not reported"
    );

    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    assert_eq!(
        drained(&watcher),
        [attention(2, 0, 12), attention(3, 0, 13)]
    );

    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [attention(4, 0, 14), SqEntry::PollDone]);
}

#[test]
fn poll_done_waits_for_room_like_any_entry() {
    let dir = TempDir::new("e-done");
    let watcher = watcher(2);
    let _files = register_files(&dir, &watcher, 2, 10);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    assert_eq!(
        drained(&watcher),
        [attention(0, 0, 10), attention(1, 0, 11)]
    );
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
}

#[test]
fn a_stopped_poll_reports_what_it_probed_and_the_next_poll_the_growth_since() {
    let dir = TempDir::new("e-stale");
    let watcher = watcher(2);
    let files = register_files(&dir, &watcher, 3, 10);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    drained(&watcher);
    append(&files.paths[2], 5);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [attention(2, 0, 12), SqEntry::PollDone]);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [attention(2, 12, 17), SqEntry::PollDone]);
}

#[test]
fn a_registration_dropped_while_stopped_is_skipped() {
    let dir = TempDir::new("e-dropped");
    let watcher = watcher(2);
    let mut files = register_files(&dir, &watcher, 5, 10);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    drained(&watcher);
    drop(files.registrations.remove(3));
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    assert_eq!(
        drained(&watcher),
        [attention(2, 0, 12), attention(4, 0, 14)]
    );
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
}

#[test]
fn a_registration_made_while_stopped_is_in_the_next_poll_not_this_one() {
    let dir = TempDir::new("e-added");
    let watcher = watcher(2);
    let _files = register_files(&dir, &watcher, 3, 10);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    drained(&watcher);
    let late = dir.file("late.log", b"late");
    let _late = watcher.register(&wtf(&late), 0, 99).expect("register");
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [attention(2, 0, 12), SqEntry::PollDone]);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [attention(99, 0, 4), SqEntry::PollDone]);
}

#[test]
fn the_value_cloned_for_a_refused_write_is_dropped() {
    let dir = TempDir::new("e-refused");
    let path = dir.file("a.log", b"abc");
    let tag = Arc::new(());
    let watcher = Watcher::<Arc<()>>::new(2).expect("watcher");
    let _registrations: Vec<_> = (0..4)
        .map(|_| {
            watcher
                .register(&wtf(&path), 0, Arc::clone(&tag))
                .expect("register")
        })
        .collect();
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    // The test's, the four registrations', and the two entries in the SQ: not the refused one.
    assert_eq!(Arc::strong_count(&tag), 1 + 4 + 2);
    drop(watcher.drain().collect::<Vec<_>>());
    assert_eq!(Arc::strong_count(&tag), 1 + 4);
}

#[test]
fn a_stopped_engine_holds_no_observer_open() {
    let dir = TempDir::new("e-open");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher(2);
    let registrations: Vec<_> = (0..4)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Stopped);
    drop(registrations);
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .expect("nothing holds the file once the registrations are dropped");
}

#[test]
fn a_terminal_failure_is_written_as_its_own_entry_and_marks_the_registration() {
    let dir = TempDir::new("e-terminal");
    let watcher = watcher(16);
    let files = register_files(&dir, &watcher, 3, 10);
    let mut engine = Engine::new(&watcher);
    let calls = AtomicUsize::new(0);
    engine.begin(&watcher, |_: &EndpointObserver| {
        match calls.fetch_add(1, Ordering::SeqCst) {
            1 => Err(win32(ERROR_INVALID_HANDLE)),
            n => Ok(10 + n as u64),
        }
    });
    assert_eq!(engine.finish(&watcher, 256, || {}), RunOutcome::Complete);
    assert_eq!(
        drained(&watcher),
        [
            attention(0, 0, 10),
            terminal(1, ERROR_INVALID_HANDLE, 0),
            attention(2, 0, 12),
            SqEntry::PollDone,
        ]
    );
    assert_eq!(
        files.registrations[1].terminal_code(),
        Some(ERROR_INVALID_HANDLE)
    );
    assert_eq!(watcher.registered(), 3);
}

#[test]
fn a_terminal_registration_is_not_probed_again_and_not_reported_again() {
    let dir = TempDir::new("e-once");
    let watcher = watcher(16);
    let _files = register_files(&dir, &watcher, 3, 10);
    let mut engine = Engine::new(&watcher);
    engine.begin(&watcher, |_| Err(win32(ERROR_INVALID_HANDLE)));
    engine.finish(&watcher, 256, || {});
    assert_eq!(drained(&watcher).len(), 4);
    let reads = AtomicUsize::new(0);
    engine.begin(&watcher, |_| {
        reads.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    });
    assert_eq!(engine.finish(&watcher, 256, || {}), RunOutcome::Complete);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
}

#[test]
fn a_registration_that_became_terminal_after_the_probe_is_not_reported() {
    let dir = TempDir::new("e-late-terminal");
    let watcher = watcher(16);
    let files = register_files(&dir, &watcher, 3, 10);
    let mut engine = Engine::new(&watcher);
    engine.begin(&watcher, EndpointObserver::file_len);
    assert!(watcher.mark_terminal(files.registrations[1].id(), ERROR_INVALID_HANDLE));
    assert_eq!(engine.finish(&watcher, 256, || {}), RunOutcome::Complete);
    assert_eq!(
        drained(&watcher),
        [attention(0, 0, 10), attention(2, 0, 12), SqEntry::PollDone]
    );
}

#[test]
fn a_terminal_entry_the_sq_refuses_is_not_marked_until_it_is_written() {
    let dir = TempDir::new("e-refused-terminal");
    let watcher = watcher(2);
    let files = register_files(&dir, &watcher, 3, 10);
    let mut engine = Engine::new(&watcher);
    let calls = AtomicUsize::new(0);
    engine.begin(&watcher, |_| match calls.fetch_add(1, Ordering::SeqCst) {
        2 => Err(win32(ERROR_INVALID_HANDLE)),
        n => Ok(10 + n as u64),
    });
    assert_eq!(engine.finish(&watcher, 256, || {}), RunOutcome::Stopped);
    assert_eq!(
        drained(&watcher),
        [attention(0, 0, 10), attention(1, 0, 11)]
    );
    assert_eq!(files.registrations[2].terminal_code(), None);
    assert_eq!(engine.finish(&watcher, 256, || {}), RunOutcome::Complete);
    assert_eq!(
        drained(&watcher),
        [terminal(2, ERROR_INVALID_HANDLE, 0), SqEntry::PollDone]
    );
    assert_eq!(
        files.registrations[2].terminal_code(),
        Some(ERROR_INVALID_HANDLE)
    );
}

#[test]
fn a_transient_failure_is_a_diagnostic_and_the_registration_is_probed_again() {
    let dir = TempDir::new("e-transient");
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(16, sink.clone()).expect("watcher");
    let files = register_files(&dir, &watcher, 2, 10);
    let mut engine = Engine::new(&watcher);
    engine.begin(&watcher, |_| Err(win32(ERROR_ACCESS_DENIED)));
    assert_eq!(engine.finish(&watcher, 256, || {}), RunOutcome::Complete);
    assert_eq!(drained(&watcher), [SqEntry::PollDone]);
    assert_eq!(sink.seen().len(), 2);
    assert!(matches!(
        sink.seen()[0],
        Diagnostic::ProbeFailed {
            win32_code: Some(ERROR_ACCESS_DENIED),
            ..
        }
    ));
    assert_eq!(files.registrations[0].terminal_code(), None);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    assert_eq!(
        drained(&watcher),
        [attention(0, 0, 10), attention(1, 0, 11), SqEntry::PollDone]
    );
}

#[test]
fn the_batch_size_does_not_change_what_is_reported() {
    let mut expected: Vec<_> = (0..9u32)
        .map(|n| attention(n, 0, 3 + u64::from(n)))
        .collect();
    expected.push(SqEntry::PollDone);
    for batch in [1usize, 2, 3, 8, 9, 10, 256] {
        let dir = TempDir::new(&format!("e-batch-{batch}"));
        let watcher = watcher(64);
        let _files = register_files(&dir, &watcher, 9, 3);
        let mut engine = Engine::new(&watcher);
        engine.begin(&watcher, EndpointObserver::file_len);
        assert_eq!(engine.finish(&watcher, batch, || {}), RunOutcome::Complete);
        assert_eq!(drained(&watcher), expected, "batch {batch}");
    }
}

#[test]
fn the_roster_lock_is_released_between_batches() {
    let dir = TempDir::new("e-lock");
    let watcher = watcher(64);
    let _files = register_files(&dir, &watcher, 7, 3);
    let mut engine = Engine::new(&watcher);
    engine.begin(&watcher, EndpointObserver::file_len);
    let mut pauses = 0;
    assert_eq!(
        engine.finish(&watcher, 2, || {
            pauses += 1;
            assert!(
                watcher.shared.roster.try_lock().is_ok(),
                "the roster lock is held across the pause"
            );
        }),
        RunOutcome::Complete
    );
    assert_eq!(pauses, 3);
}

#[test]
fn a_registration_dropped_in_a_pause_between_batches_is_skipped_if_not_yet_reached() {
    let dir = TempDir::new("e-pause");
    let watcher = watcher(64);
    let mut files = register_files(&dir, &watcher, 6, 3);
    let mut engine = Engine::new(&watcher);
    engine.begin(&watcher, EndpointObserver::file_len);
    let mut ahead = Some(files.registrations.remove(4));
    assert_eq!(
        engine.finish(&watcher, 2, || drop(ahead.take())),
        RunOutcome::Complete
    );
    let values: Vec<_> = drained(&watcher)
        .into_iter()
        .filter_map(|entry| match entry {
            SqEntry::Attention(a) => Some(a.value),
            _ => None,
        })
        .collect();
    assert_eq!(values, [0, 1, 2, 3, 5]);
}

#[test]
fn many_changes_through_a_small_sq_are_each_reported_exactly_once_in_order() {
    let dir = TempDir::new("e-many");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher(64);
    let _registrations: Vec<_> = (0..600u32)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    let mut engine = Engine::new(&watcher);
    let mut all = Vec::new();
    let mut stops = 0;
    loop {
        let outcome = engine.run(&watcher);
        all.extend(drained(&watcher));
        match outcome {
            RunOutcome::Stopped => stops += 1,
            RunOutcome::Complete => break,
        }
        assert!(stops < 100, "the pass never finished");
    }
    assert!(stops >= 9, "{stops} stops");
    assert_eq!(all.len(), 601);
    assert_eq!(all.pop(), Some(SqEntry::PollDone));
    let expected: Vec<_> = (0..600u32).map(|n| attention(n, 0, 10)).collect();
    assert_eq!(all, expected);
}

#[test]
fn a_run_after_completion_starts_a_new_poll() {
    let dir = TempDir::new("e-new");
    let watcher = watcher(16);
    let files = register_files(&dir, &watcher, 1, 4);
    let mut engine = Engine::new(&watcher);
    assert_eq!(engine.run(&watcher), RunOutcome::Complete);
    drained(&watcher);
    for round in 1..=5u64 {
        append(&files.paths[0], 3);
        assert_eq!(engine.run(&watcher), RunOutcome::Complete);
        assert_eq!(
            drained(&watcher),
            [
                attention(0, 4 + 3 * (round - 1), 4 + 3 * round),
                SqEntry::PollDone
            ],
            "round {round}"
        );
    }
}

#[test]
fn the_doorbell_is_rung_after_each_full_batch_and_once_more_at_the_end() {
    let dir = TempDir::new("e-rings");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher(1024);
    let _registrations: Vec<_> = (0..600u32)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    let mut engine = Engine::new(&watcher);
    let rings = std::cell::Cell::new(0);
    let outcome = engine.run_ringing(&watcher, || rings.set(rings.get() + 1));
    assert_eq!(outcome, RunOutcome::Complete);
    // Two full batches of the three the report makes, and the end.
    assert_eq!(rings.get(), 3);
}

#[test]
fn the_doorbell_is_rung_once_for_a_poll_with_nothing_to_report_and_once_for_a_stop() {
    let dir = TempDir::new("e-rings-once");
    let empty = watcher(4);
    let mut engine = Engine::new(&empty);
    let rings = std::cell::Cell::new(0);
    engine.run_ringing(&empty, || rings.set(rings.get() + 1));
    assert_eq!(rings.get(), 1);

    let full = watcher(2);
    let _files = register_files(&dir, &full, 5, 10);
    let mut engine = Engine::new(&full);
    rings.set(0);
    assert_eq!(
        engine.run_ringing(&full, || rings.set(rings.get() + 1)),
        RunOutcome::Stopped
    );
    assert_eq!(rings.get(), 1, "a full queue is news to the caller");
}
