// Copyright (c) 2026 Mike Grier
//! `CompletionWake` over a ring its owner keeps: what it delivers, the setup signal's reach, the
//! owner submitting from inside a call, and silence once dropped.

use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::CompletionWake;
use crate::{Batch, IoRing, PushOptions, SharedHandle};

/// Generous: each wait is for a real completion on a local file and a pool callback.
const BOUND: Duration = Duration::from_secs(5);

type Owned = Arc<Mutex<IoRing<Vec<u8>, u64>>>;

/// A file of `len` bytes, removed when dropped.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ioring-wake-{tag}-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, vec![7_u8; 64]).expect("write fixture");
        Self(path)
    }

    fn open(&self) -> SharedHandle {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(&self.0)
            .expect("open fixture");
        SharedHandle::new(OwnedHandle::from(file))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn owned() -> Owned {
    Arc::new(Mutex::new(
        IoRing::with_inventory(8, 8).expect("create ring"),
    ))
}

fn lock(ring: &Owned) -> std::sync::MutexGuard<'_, IoRing<Vec<u8>, u64>> {
    ring.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Push a read tagged `tag` and submit it.
fn read(ring: &mut IoRing<Vec<u8>, u64>, file: &SharedHandle, tag: u64) {
    let mut batch = Batch::new(ring);
    batch
        .read_owned(file, vec![0; 8], tag, 0, PushOptions::new())
        .expect("queue read");
    batch.submit().expect("submit");
}

/// A wake whose owner drains `ring` to empty on every call, sending each completion's tag, and
/// counting its calls.
fn draining(ring: &Owned) -> (CompletionWake, Receiver<u64>, Arc<AtomicUsize>) {
    let (tx, rx) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let (owner, counted) = (Arc::clone(ring), Arc::clone(&calls));
    let wake = CompletionWake::new(
        &mut *lock(ring),
        move || {
            counted.fetch_add(1, Ordering::SeqCst);
            loop {
                let popped = lock(&owner).try_pop();
                match popped {
                    Ok(Some((_, Some((_, tag))))) => {
                        let _ = tx.send(tag);
                    }
                    Ok(Some((_, None))) => {}
                    Ok(None) | Err(_) => break,
                }
            }
        },
        None,
    )
    .expect("wire the wake");
    (wake, rx, calls)
}

#[test]
fn a_completion_after_the_wake_is_built_is_drained_by_its_owner() {
    let fixture = Fixture::new("after");
    let file = fixture.open();
    let ring = owned();
    let (wake, rx, _) = draining(&ring);
    read(&mut lock(&ring), &file, 41);
    assert_eq!(rx.recv_timeout(BOUND), Ok(41));
    drop(wake);
}

#[test]
fn the_setup_signal_calls_the_owner_once_with_nothing_queued() {
    let ring = owned();
    let (wake, rx, calls) = draining(&ring);
    let started = std::time::Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < BOUND,
            "the setup signal never called the owner"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(wake);
    assert!(rx.try_recv().is_err(), "nothing was queued");
}

#[test]
fn a_backlog_queued_before_the_wake_is_reached_by_the_setup_signal() {
    let fixture = Fixture::new("backlog");
    let file = fixture.open();
    let ring = owned();
    {
        // Submitted and waited for, so the completion sits queued before any wake exists: the
        // edge it raised was spent, and only the setup signal can reach it (M26.12).
        let mut ring = lock(&ring);
        let mut batch = Batch::new(&mut ring);
        batch
            .read_owned(&file, vec![0; 8], 7, 0, PushOptions::new())
            .expect("queue read");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
    }
    let (wake, rx, _) = draining(&ring);
    assert_eq!(rx.recv_timeout(BOUND), Ok(7));
    drop(wake);
}

#[test]
fn the_owner_may_submit_from_inside_a_call() {
    let fixture = Fixture::new("submit");
    let file = fixture.open();
    let ring = owned();
    let (tx, rx) = mpsc::channel();
    let (owner, handle) = (Arc::clone(&ring), file.clone());
    let wake = CompletionWake::new(
        &mut *lock(&ring),
        move || {
            loop {
                let mut guard = lock(&owner);
                let tag = match guard.try_pop() {
                    Ok(Some((_, Some((_, tag))))) => tag,
                    Ok(Some((_, None))) => continue,
                    // Empty: drained, as the wake's contract asks.
                    Ok(None) | Err(_) => break,
                };
                if tag == 1 {
                    // A second read, pushed and submitted from inside the call: the wake holds
                    // no lock of its own, so this cannot deadlock (D-84).
                    let mut batch = Batch::new(&mut guard);
                    if batch
                        .read_owned(&handle, vec![0; 8], 2, 0, PushOptions::new())
                        .is_ok()
                    {
                        let _ = batch.submit();
                    }
                }
                drop(guard);
                let _ = tx.send(tag);
            }
        },
        None,
    )
    .expect("wire the wake");
    read(&mut lock(&ring), &file, 1);
    assert_eq!(rx.recv_timeout(BOUND), Ok(1));
    assert_eq!(
        rx.recv_timeout(BOUND),
        Ok(2),
        "the read submitted from inside the call"
    );
    drop(wake);
}

#[test]
fn nothing_is_called_once_the_wake_is_dropped() {
    let fixture = Fixture::new("dropped");
    let file = fixture.open();
    let ring = owned();
    let (wake, rx, calls) = draining(&ring);
    read(&mut lock(&ring), &file, 1);
    assert_eq!(rx.recv_timeout(BOUND), Ok(1));
    drop(wake);
    let before = calls.load(Ordering::SeqCst);
    {
        let mut ring = lock(&ring);
        let mut batch = Batch::new(&mut ring);
        batch
            .read_owned(&file, vec![0; 8], 2, 0, PushOptions::new())
            .expect("queue read");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "a dropped wake calls no one"
    );
    assert!(rx.try_recv().is_err());
    let popped = lock(&ring).pop_within(BOUND).expect("pop");
    assert!(
        matches!(popped, Some((_, Some((_, 2))))),
        "the completion is still the owner's to pop"
    );
}
