// Copyright (c) 2026 Mike Grier
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SHARING_VIOLATION};

use super::{Registration, Watcher};
use crate::OpenError;
use crate::test_support::{TempDir, code, wtf};

fn watcher<T: Clone>() -> Watcher<T> {
    Watcher::new(64).expect("a valid SQ capacity")
}

#[test]
fn a_new_watcher_has_no_registrations() {
    let watcher = watcher::<u32>();
    assert_eq!(watcher.registered(), 0);
    assert_eq!(watcher.sq_capacity(), 64);
}

#[test]
fn registering_a_file_adds_one_and_keeps_its_value_and_offset() {
    let dir = TempDir::new("w-one");
    let path = dir.file("a.log", b"0123456789");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 4, "first").expect("register");
    assert_eq!(watcher.registered(), 1);
    assert_eq!(registration.value(), "first");
    assert_eq!(registration.start_offset(), 4);
}

#[test]
fn dropping_the_registration_unregisters() {
    let dir = TempDir::new("w-drop");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 0, 1u8).expect("register");
    assert_eq!(watcher.registered(), 1);
    drop(registration);
    assert_eq!(watcher.registered(), 0);
}

#[test]
fn the_value_is_moved_in_and_released_with_the_registration() {
    let dir = TempDir::new("w-move");
    let path = dir.file("a.log", b"abc");
    let tag = Arc::new(());
    let watcher = watcher();
    let registration = watcher
        .register(&wtf(&path), 0, Arc::clone(&tag))
        .expect("register");
    assert_eq!(Arc::strong_count(&tag), 2);
    let cloned = registration.value();
    assert_eq!(Arc::strong_count(&tag), 3);
    drop(cloned);
    drop(registration);
    assert_eq!(Arc::strong_count(&tag), 1);
}

#[test]
fn one_path_registered_twice_gives_two_independent_registrations() {
    let dir = TempDir::new("w-twice");
    let path = dir.file("a.log", b"abcdef");
    let watcher = watcher();
    let first = watcher.register(&wtf(&path), 1, "first").expect("first");
    let second = watcher.register(&wtf(&path), 5, "second").expect("second");
    assert_eq!(watcher.registered(), 2);
    drop(first);
    assert_eq!(watcher.registered(), 1);
    assert_eq!(second.value(), "second");
    assert_eq!(second.start_offset(), 5);
}

#[test]
fn many_registrations_unregister_in_any_order() {
    let dir = TempDir::new("w-many");
    let path = dir.file("a.log", b"x");
    let watcher = watcher();
    let mut registrations: Vec<_> = (0..200u32)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    assert_eq!(watcher.registered(), 200);

    let evens: Vec<_> = (0..registrations.len())
        .rev()
        .filter(|n| n % 2 == 0)
        .map(|n| registrations.remove(n))
        .collect();
    assert_eq!(evens.len(), 100);
    drop(evens);
    assert_eq!(watcher.registered(), 100);
    for (registration, expected) in registrations.iter().zip((0..200u32).filter(|n| n % 2 == 1)) {
        assert_eq!(registration.value(), expected);
    }

    registrations.reverse();
    registrations.truncate(10);
    assert_eq!(watcher.registered(), 10);
    registrations.clear();
    assert_eq!(watcher.registered(), 0);
}

#[test]
fn a_failed_open_registers_nothing_and_drops_the_value() {
    let dir = TempDir::new("w-fail");
    let tag = Arc::new(());
    let watcher = watcher();
    let error = watcher
        .register(&wtf(&dir.0.join("absent.log")), 0, Arc::clone(&tag))
        .expect_err("no such file");
    match error {
        OpenError::Io(io) => assert_eq!(code(&io), Some(ERROR_FILE_NOT_FOUND)),
        OpenError::OffsetBeyondLength { .. } => panic!("an open failure, not an offset"),
    }
    assert_eq!(watcher.registered(), 0);
    assert_eq!(Arc::strong_count(&tag), 1);
}

#[test]
fn an_offset_past_the_length_registers_nothing() {
    let dir = TempDir::new("w-offset");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let error = watcher
        .register(&wtf(&path), 4, ())
        .expect_err("past the end");
    assert!(matches!(
        error,
        OpenError::OffsetBeyondLength { offset: 4, len: 3 }
    ));
    assert_eq!(watcher.registered(), 0);
    assert!(watcher.register(&wtf(&path), 3, ()).is_ok());
}

#[test]
fn a_registration_may_outlive_its_watcher() {
    let dir = TempDir::new("w-outlive");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 2, 77u64).expect("register");
    drop(watcher);
    assert_eq!(registration.value(), 77);
    assert_eq!(registration.start_offset(), 2);
}

#[test]
fn the_file_is_held_while_registered_and_released_when_it_drops() {
    let dir = TempDir::new("w-held");
    let path = dir.file("a.log", b"abc");
    let exclusive = || OpenOptions::new().read(true).share_mode(0).open(&path);
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 0, ()).expect("register");
    let refused = exclusive().expect_err("held");
    assert_eq!(code(&refused), Some(ERROR_SHARING_VIOLATION));
    drop(registration);
    exclusive().expect("released with the registration");
}

#[test]
fn registering_from_many_threads_at_once() {
    let dir = TempDir::new("w-threads");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let registrations: Vec<Registration<usize>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8usize)
            .map(|worker| {
                let (watcher, path) = (&watcher, &path);
                scope.spawn(move || {
                    (0..50usize)
                        .map(|n| {
                            watcher
                                .register(&wtf(path), 0, worker * 50 + n)
                                .expect("register")
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("worker"))
            .collect()
    });
    assert_eq!(watcher.registered(), 400);
    let mut values: Vec<_> = registrations.iter().map(Registration::value).collect();
    values.sort_unstable();
    assert_eq!(values, (0..400).collect::<Vec<_>>());
    drop(registrations);
    assert_eq!(watcher.registered(), 0);
}

#[test]
fn registrations_unregister_from_another_thread() {
    let dir = TempDir::new("w-cross");
    let path = dir.file("a.log", b"abc");
    let watcher = watcher();
    let registration = watcher.register(&wtf(&path), 0, 5u8).expect("register");
    std::thread::spawn(move || drop(registration))
        .join()
        .expect("thread");
    assert_eq!(watcher.registered(), 0);
}

#[test]
fn the_types_cross_threads_when_their_value_does() {
    fn send_sync<X: Send + Sync>() {}
    send_sync::<Watcher<Arc<u32>>>();
    send_sync::<Registration<Arc<u32>>>();
}
