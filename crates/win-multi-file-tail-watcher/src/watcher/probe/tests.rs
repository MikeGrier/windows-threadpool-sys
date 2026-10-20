// Copyright (c) 2026 Mike Grier
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_INVALID, ERROR_INVALID_HANDLE, ERROR_SHARING_VIOLATION,
};

use super::{Change, Outcome};
use crate::chunked::Chunked;
use crate::test_support::{RecordingSink, TempDir, code, wtf};
use crate::watcher::SnapshotItem;
use crate::{Diagnostic, Registration, Watcher};

fn watcher() -> Watcher<u32> {
    Watcher::new(64).expect("a valid SQ capacity")
}

/// Registrations of files `base`, `base + 1`, ... bytes long, each registered at `offset_of(n)`.
fn register_files(
    dir: &TempDir,
    watcher: &Watcher<u32>,
    count: usize,
    base: usize,
    offset_of: impl Fn(usize) -> u64,
) -> Vec<Registration<u32>> {
    (0..count)
        .map(|n| {
            let path = dir.file(format!("f{n}.log"), &vec![b'x'; base + n]);
            watcher
                .register(&wtf(&path), offset_of(n), u32::try_from(n).expect("small"))
                .expect("register")
        })
        .collect()
}

fn snapshot(watcher: &Watcher<u32>) -> Chunked<SnapshotItem> {
    let mut out = Chunked::new();
    watcher.snapshot(&mut out);
    out
}

fn probed(watcher: &Watcher<u32>) -> Vec<Change> {
    let mut snapshot = snapshot(watcher);
    let mut changed = Chunked::new();
    watcher.probe(&mut snapshot, &mut changed);
    changed.iter().copied().collect()
}

fn probed_with(
    watcher: &Watcher<u32>,
    read: impl Fn(&windows_overlapped_io_sys::EndpointObserver) -> io::Result<u64>,
) -> Vec<Change> {
    let mut snapshot = snapshot(watcher);
    let mut changed = Chunked::new();
    watcher.probe_with(&mut snapshot, &mut changed, read);
    changed.iter().copied().collect()
}

fn changed(id: u64, last_known: u64, current: u64) -> Change {
    Change {
        id,
        last_known,
        outcome: Outcome::Changed { current },
    }
}

fn terminal(id: u64, last_known: u64, win32_code: u32) -> Change {
    Change {
        id,
        last_known,
        outcome: Outcome::Terminal { win32_code },
    }
}

fn win32(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code.cast_signed())
}

#[test]
fn registrations_whose_length_is_the_last_reported_one_produce_nothing() {
    let dir = TempDir::new("p-same");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 6, 1, |n| 1 + n as u64);
    assert!(probed(&watcher).is_empty());
}

#[test]
fn a_file_with_content_registered_at_zero_is_reported_from_zero() {
    let dir = TempDir::new("p-zero");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 4, 3, |_| 0);
    let expected: Vec<_> = registrations
        .iter()
        .enumerate()
        .map(|(n, r)| changed(r.id(), 0, 3 + n as u64))
        .collect();
    assert_eq!(probed(&watcher), expected);
}

#[test]
fn an_empty_file_registered_at_zero_is_not_a_change() {
    let dir = TempDir::new("p-empty");
    let watcher = watcher();
    let path = dir.file("e.log", b"");
    let _registration = watcher.register(&wtf(&path), 0, 0).expect("register");
    assert!(probed(&watcher).is_empty());
}

#[test]
fn growth_after_the_snapshot_is_the_length_the_probe_reads() {
    let dir = TempDir::new("p-grow");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 10, 1).expect("register");
    let mut snapshot = snapshot(&watcher);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut file| io::Write::write_all(&mut file, b"abcde"))
        .expect("append");
    let mut found = Chunked::new();
    watcher.probe(&mut snapshot, &mut found);
    let found: Vec<_> = found.iter().copied().collect();
    assert_eq!(found, [changed(registration.id(), 10, 15)]);
}

#[test]
fn a_shrink_is_a_change_in_the_other_direction() {
    let dir = TempDir::new("p-shrink");
    let path = dir.file("a.log", &[b'x'; 100]);
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 100, 1).expect("register");
    OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_len(40)
        .expect("shrink");
    assert_eq!(probed(&watcher), [changed(registration.id(), 100, 40)]);
}

#[test]
fn only_the_changed_ones_are_kept_and_in_id_order() {
    let dir = TempDir::new("p-mixed");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 9, 1, |n| {
        if n % 3 == 0 { 0 } else { 1 + n as u64 }
    });
    let expected: Vec<_> = registrations
        .iter()
        .enumerate()
        .filter(|(n, _)| n % 3 == 0)
        .map(|(n, r)| changed(r.id(), 0, 1 + n as u64))
        .collect();
    assert_eq!(probed(&watcher), expected);
}

#[test]
fn a_probe_empties_the_snapshot_and_changes_nothing_in_the_roster() {
    let dir = TempDir::new("p-roster");
    let watcher = watcher();
    let registrations = register_files(&dir, &watcher, 5, 2, |_| 0);
    let mut snapshot = snapshot(&watcher);
    assert_eq!(snapshot.len(), 5);
    let mut found = Chunked::new();
    watcher.probe(&mut snapshot, &mut found);
    assert!(snapshot.is_empty());
    assert_eq!(found.len(), 5);
    assert!(registrations.iter().all(|r| r.last_known() == 0));
    assert!(registrations.iter().all(|r| r.terminal_code().is_none()));
}

#[test]
fn the_observers_are_dropped_by_the_probe_not_held_until_the_buffer_is() {
    let dir = TempDir::new("p-drop");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 3, 1).expect("register");
    let mut snapshot = snapshot(&watcher);
    drop(registration);
    let exclusive = || OpenOptions::new().read(true).share_mode(0).open(&path);
    let refused = exclusive().expect_err("the observer holds the file open");
    assert_eq!(code(&refused), Some(ERROR_SHARING_VIOLATION));
    let mut found = Chunked::new();
    watcher.probe(&mut snapshot, &mut found);
    assert!(snapshot.is_empty());
    exclusive().expect("the probe let go of every observer");
}

#[test]
fn a_probe_with_nothing_snapshotted_reads_nothing() {
    let watcher = watcher();
    let reads = AtomicUsize::new(0);
    let found = probed_with(&watcher, |_| {
        reads.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    });
    assert!(found.is_empty());
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[test]
fn each_observer_is_read_exactly_once() {
    let dir = TempDir::new("p-once");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 25, 1, |_| 0);
    let reads = AtomicUsize::new(0);
    let found = probed_with(&watcher, |_| {
        reads.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    });
    assert_eq!(reads.load(Ordering::SeqCst), 25);
    assert!(found.is_empty());
}

#[test]
fn the_probe_holds_no_roster_lock() {
    let dir = TempDir::new("p-lock");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let _registrations = register_files(&dir, &watcher, 3, 1, |_| 0);
    let _ = probed_with(&watcher, |_| {
        assert!(
            watcher.shared.roster.try_lock().is_ok(),
            "the roster lock is held across a probe"
        );
        let _ = watcher.register(&wtf(&path), 0, 9).expect("register");
        Ok(0)
    });
}

#[test]
fn a_transient_failure_is_reported_to_the_sink_and_kept_nowhere() {
    let dir = TempDir::new("p-transient");
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(64, sink.clone()).expect("watcher");
    let registrations = register_files(&dir, &watcher, 3, 1, |_| 0);
    let failing = registrations[1].id();
    let calls = AtomicUsize::new(0);
    let found = probed_with(&watcher, |_| {
        let call = calls.fetch_add(1, Ordering::SeqCst);
        if call == 1 {
            Err(win32(ERROR_ACCESS_DENIED))
        } else {
            Ok(7)
        }
    });
    assert_eq!(
        found,
        [
            changed(registrations[0].id(), 0, 7),
            changed(registrations[2].id(), 0, 7)
        ]
    );
    assert_eq!(
        sink.seen(),
        [Diagnostic::ProbeFailed {
            registration: failing,
            win32_code: Some(ERROR_ACCESS_DENIED)
        }]
    );
    assert_eq!(registrations[1].terminal_code(), None);
}

#[test]
fn a_transient_failure_is_probed_again_on_the_next_pass() {
    let dir = TempDir::new("p-again");
    let watcher = watcher();
    let registration = register_files(&dir, &watcher, 1, 4, |_| 0).remove(0);
    let first = probed_with(&watcher, |_| Err(win32(ERROR_ACCESS_DENIED)));
    assert!(first.is_empty());
    assert_eq!(probed(&watcher), [changed(registration.id(), 0, 4)]);
}

#[test]
fn a_failure_with_no_code_is_transient_and_reported_without_one() {
    let dir = TempDir::new("p-nocode");
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(64, sink.clone()).expect("watcher");
    let registration = register_files(&dir, &watcher, 1, 1, |_| 0).remove(0);
    let found = probed_with(&watcher, |_| Err(io::Error::other("no code")));
    assert!(found.is_empty());
    assert_eq!(
        sink.seen(),
        [Diagnostic::ProbeFailed {
            registration: registration.id(),
            win32_code: None
        }]
    );
}

#[test]
fn a_terminal_failure_is_kept_with_its_code_and_is_not_a_diagnostic() {
    let dir = TempDir::new("p-terminal");
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(64, sink.clone()).expect("watcher");
    let registrations = register_files(&dir, &watcher, 2, 3, |_| 1);
    let found = probed_with(&watcher, |_| Err(win32(ERROR_INVALID_HANDLE)));
    assert_eq!(
        found,
        [
            terminal(registrations[0].id(), 1, ERROR_INVALID_HANDLE),
            terminal(registrations[1].id(), 1, ERROR_INVALID_HANDLE),
        ]
    );
    assert!(sink.seen().is_empty());
    assert!(
        registrations.iter().all(|r| r.terminal_code().is_none()),
        "marking an entry terminal is the report pass's"
    );
}

#[test]
fn an_invalidated_file_is_terminal_too() {
    let dir = TempDir::new("p-invalid");
    let watcher = watcher();
    let registration = register_files(&dir, &watcher, 1, 1, |_| 0).remove(0);
    let found = probed_with(&watcher, |_| Err(win32(ERROR_FILE_INVALID)));
    assert_eq!(found, [terminal(registration.id(), 0, ERROR_FILE_INVALID)]);
}

#[test]
fn changes_terminal_failures_and_transient_ones_stay_in_id_order() {
    let dir = TempDir::new("p-order");
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(64, sink.clone()).expect("watcher");
    let registrations = register_files(&dir, &watcher, 8, 1, |_| 0);
    let calls = AtomicUsize::new(0);
    let found = probed_with(&watcher, |_| {
        match calls.fetch_add(1, Ordering::SeqCst) % 4 {
            0 => Ok(5),
            1 => Err(win32(ERROR_INVALID_HANDLE)),
            2 => Err(win32(ERROR_ACCESS_DENIED)),
            _ => Ok(0),
        }
    });
    let r = |n: usize| registrations[n].id();
    assert_eq!(
        found,
        [
            changed(r(0), 0, 5),
            terminal(r(1), 0, ERROR_INVALID_HANDLE),
            changed(r(4), 0, 5),
            terminal(r(5), 0, ERROR_INVALID_HANDLE),
        ]
    );
    assert_eq!(sink.seen().len(), 2);
}

#[test]
fn a_sink_that_wants_nothing_is_asked_for_each_transient_failure_and_given_none() {
    let dir = TempDir::new("p-unwanted");
    let sink = RecordingSink::unwilling();
    let watcher = Watcher::<u32>::with_diagnostics(64, sink.clone()).expect("watcher");
    let _registrations = register_files(&dir, &watcher, 4, 1, |_| 0);
    let found = probed_with(&watcher, |_| Err(win32(ERROR_ACCESS_DENIED)));
    assert!(found.is_empty());
    assert!(sink.seen().is_empty());
    assert_eq!(sink.asked().len(), 4);
}

#[test]
fn a_changed_list_longer_than_a_chunk_keeps_every_change_in_order() {
    let dir = TempDir::new("p-big");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    let registrations: Vec<_> = (0..2200)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    let found = probed(&watcher);
    let expected: Vec<_> = registrations
        .iter()
        .map(|r| changed(r.id(), 0, 10))
        .collect();
    assert_eq!(found, expected);
}
