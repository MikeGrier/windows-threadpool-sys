// Copyright (c) 2026 Mike Grier
//! Shutdown from every state of a poll, requested from the caller's own callback on a thread-pool
//! wait on the watcher's doorbell, which is where a caller would make it: a callback that must not
//! block.
#![cfg(windows)]

use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use win_multi_file_tail_watcher::{PollError, Registration, SqEntry, Watcher};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{SetEvent, WaitForSingleObject};
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use wtf_string::Wtf16String;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "win-multi-file-tail-watcher-shutdown-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("create the test directory");
        Self(dir)
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("create the test file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wtf(path: &Path) -> Wtf16String {
    Wtf16String::from(path.as_os_str())
}

// `count` files of `10 + n` bytes registered at offset 0, so each is a change.
fn changing(dir: &TempDir, watcher: &Watcher<u32>, count: u32) -> Vec<Registration<u32>> {
    (0..count)
        .map(|n| {
            let path = dir.file(&format!("f{n}.log"), &vec![b'x'; 10 + n as usize]);
            watcher.register(&wtf(&path), 0, n).expect("register")
        })
        .collect()
}

fn rung<T: Clone>(watcher: &Watcher<T>, millis: u32) -> bool {
    // SAFETY: the doorbell is a live event for as long as the watcher is, and waiting on it has no
    // other precondition.
    unsafe { WaitForSingleObject(watcher.doorbell().as_raw_handle(), millis) == WAIT_OBJECT_0 }
}

// Wait for the doorbell, reset it, and read what it announced.
fn round<T: Clone>(watcher: &Watcher<T>) -> Vec<SqEntry<T>> {
    assert!(rung(watcher, 30_000), "the doorbell was not rung");
    watcher.reset_doorbell();
    watcher.drain().collect()
}

// Rounds, ringing the resume doorbell after each, until poll-done has been read.
fn until_poll_done<T: Clone>(watcher: &Watcher<T>) -> Vec<SqEntry<T>> {
    let mut all = round(watcher);
    while !all.iter().any(|entry| matches!(entry, SqEntry::PollDone)) {
        watcher.ring_resume();
        all.extend(round(watcher));
    }
    all
}

fn attentions<T>(entries: &[SqEntry<T>]) -> Vec<&T> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            SqEntry::Attention(a) => Some(&a.value),
            _ => None,
        })
        .collect()
}

fn ring(event: &OwnedHandle) {
    // SAFETY: a live event owned by `event` for the duration of the call.
    unsafe { SetEvent(event.as_raw_handle()) };
}

// The caller's own callback: a thread-pool wait on a duplicate of the watcher's doorbell which, when
// the doorbell is rung, requests a shutdown `times` times and reports how long that took.
fn arm_caller<T: Clone + Send + 'static>(
    watcher: &Arc<Watcher<T>>,
    times: usize,
) -> (ThreadpoolWait, Receiver<Duration>) {
    let handle = watcher.doorbell().try_clone_to_owned().expect("duplicate");
    // SAFETY: an event is a waitable object the pool supports, and the duplicate is owned by
    // nothing else.
    let target = unsafe { WaitableHandle::assume_waitable(handle) };
    let (sent, requested) = mpsc::channel();
    let watcher = Arc::clone(watcher);
    let wait = ThreadpoolWait::new(
        target,
        move |_| {
            let start = Instant::now();
            for _ in 0..times {
                watcher.shutdown();
            }
            let _ = sent.send(start.elapsed());
        },
        None,
    )
    .expect("wait");
    wait.arm(None);
    (wait, requested)
}

fn doorbell_copy<T: Clone>(watcher: &Watcher<T>) -> OwnedHandle {
    watcher.doorbell().try_clone_to_owned().expect("duplicate")
}

#[test]
fn a_request_from_the_callers_wait_while_a_poll_is_parked_stops_it_and_poll_done_is_delivered() {
    let dir = TempDir::new("parked");
    let watcher = Arc::new(Watcher::new(2).expect("watcher"));
    let _registrations = changing(&dir, &watcher, 6);
    let (wait, requested) = arm_caller(&watcher, 1);

    watcher.poll().expect("poll");
    // The queue fills and the doorbell rings; the caller's wait answers it with the request.
    let took = requested
        .recv_timeout(Duration::from_secs(10))
        .expect("the caller's callback ran");
    assert!(took < Duration::from_secs(1), "the request took {took:?}");

    let first = round(&watcher);
    assert_eq!(attentions(&first), [&0, &1]);
    watcher.ring_resume();
    assert_eq!(round(&watcher), [SqEntry::PollDone]);
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
    wait.stop_and_drain();
}

#[test]
fn a_request_from_the_callers_wait_while_a_poll_is_inside_its_probe_returns_at_once() {
    const FILES: u32 = 20_000;
    let dir = TempDir::new("probe");
    let path = dir.file("a.log", b"0123456789");
    // Registered at their length, so a poll is all probe and reports nothing.
    let watcher = Arc::new(Watcher::new(4).expect("watcher"));
    let _registrations: Vec<_> = (0..FILES)
        .map(|n| watcher.register(&wtf(&path), 10, n).expect("register"))
        .collect();
    let start = Instant::now();
    watcher.poll().expect("poll");
    assert_eq!(round(&watcher), [SqEntry::PollDone]);
    let pass = start.elapsed();

    // Armed only now, so the first poll was not asked to stop.
    let (wait, requested) = arm_caller(&watcher, 1);
    let bell = doorbell_copy(&watcher);
    let start = Instant::now();
    watcher.poll().expect("poll");
    // The poll is probing twenty thousand files, which does not ring the doorbell; ring it for the
    // caller's wait.
    ring(&bell);
    let took = requested
        .recv_timeout(Duration::from_secs(10))
        .expect("the caller's callback ran");
    assert!(took < Duration::from_secs(1), "the request took {took:?}");
    // The doorbell was rung by hand, so the first round may be empty.
    assert_eq!(until_poll_done(&watcher), [SqEntry::PollDone]);
    let stopped = start.elapsed();

    // A pass too quick to compare against says nothing about where the poll stopped.
    if pass >= Duration::from_millis(10) {
        assert!(
            stopped < pass / 2,
            "the poll took {stopped:?} to stop against a pass of {pass:?}"
        );
    }
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
    wait.stop_and_drain();
}

// A value whose clone waits for `release`, so a poll can be held inside the report.
struct Gate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
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

struct Blocker(Arc<Gate>);

impl Clone for Blocker {
    fn clone(&self) -> Self {
        self.0.set(|state| state.0 = true);
        self.0.wait_until(|state| state.1);
        Self(Arc::clone(&self.0))
    }
}

#[test]
fn a_request_from_the_callers_wait_while_a_poll_is_held_inside_the_report_does_not_wait_for_it() {
    let dir = TempDir::new("report");
    let gate = Arc::new(Gate {
        state: Mutex::new((false, false)),
        changed: Condvar::new(),
    });
    let watcher = Arc::new(Watcher::<Blocker>::new(8).expect("watcher"));
    let _registrations: Vec<_> = (0..5)
        .map(|n| {
            let path = dir.file(&format!("f{n}.log"), &vec![b'x'; 10 + n]);
            watcher
                .register(&wtf(&path), 0, Blocker(Arc::clone(&gate)))
                .expect("register")
        })
        .collect();
    let (wait, requested) = arm_caller(&watcher, 1);
    let bell = doorbell_copy(&watcher);

    watcher.poll().expect("poll");
    // The poll is inside `T::clone` for its first change, holding the roster lock.
    gate.wait_until(|state| state.0);
    ring(&bell);
    let took = requested
        .recv_timeout(Duration::from_secs(10))
        .expect("the request waited for the poll it was stopping");
    assert!(took < Duration::from_secs(1), "the request took {took:?}");

    gate.set(|state| state.1 = true);
    let entries = until_poll_done(&watcher);
    assert!(matches!(entries.last(), Some(SqEntry::PollDone)));
    assert_eq!(
        attentions(&entries).len(),
        1,
        "the change in hand was written and the rest abandoned"
    );
    wait.stop_and_drain();
}

#[test]
fn repeated_requests_from_the_callers_wait_and_from_other_threads_are_harmless() {
    let watcher = Arc::new(Watcher::<u32>::new(4).expect("watcher"));
    let (wait, requested) = arm_caller(&watcher, 5);
    let bell = doorbell_copy(&watcher);
    ring(&bell);
    requested
        .recv_timeout(Duration::from_secs(10))
        .expect("the caller's callback ran");
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..100 {
                    watcher.shutdown();
                }
            });
        }
    });
    watcher.shutdown();
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
    assert!(matches!(watcher.poll(), Err(PollError::Shutdown)));
    wait.stop_and_drain();
}

#[test]
fn dropping_during_a_poll_over_a_large_roster_takes_a_step_not_a_pass() {
    const FILES: u32 = 20_000;
    let dir = TempDir::new("drop-large");
    let path = dir.file("a.log", b"0123456789");
    // Registered at their length, so a poll is all probe and reports nothing.
    let build = || {
        let watcher = Watcher::<u32>::new(4).expect("watcher");
        let registrations: Vec<_> = (0..FILES)
            .map(|n| watcher.register(&wtf(&path), 10, n).expect("register"))
            .collect();
        (watcher, registrations)
    };

    let (first, _first_registrations) = build();
    first.poll().expect("poll");
    assert_eq!(round(&first), [SqEntry::PollDone]);
    let start = Instant::now();
    first.poll().expect("poll");
    assert_eq!(round(&first), [SqEntry::PollDone]);
    let pass = start.elapsed();

    first.poll().expect("poll");
    let start = Instant::now();
    drop(first);
    let without_a_request = start.elapsed();

    let (second, _second_registrations) = build();
    second.poll().expect("poll");
    second.shutdown();
    let start = Instant::now();
    drop(second);
    let with_a_request = start.elapsed();

    // A pass that is too quick to compare against says nothing about the drops.
    if pass >= Duration::from_millis(10) {
        assert!(
            without_a_request < pass / 2,
            "a drop took {without_a_request:?} against a pass of {pass:?}"
        );
        assert!(
            with_a_request < pass / 2,
            "a drop after a request took {with_a_request:?} against a pass of {pass:?}"
        );
    }
}

#[test]
fn no_callback_of_the_crate_runs_after_a_watcher_with_a_parked_poll_is_dropped() {
    let dir = TempDir::new("quiet");
    let watcher = Watcher::new(2).expect("watcher");
    let registrations = changing(&dir, &watcher, 5);
    watcher.poll().expect("poll");
    assert_eq!(attentions(&round(&watcher)), [&0, &1]);
    let bell = doorbell_copy(&watcher);
    let resume = watcher
        .resume_doorbell()
        .try_clone_to_owned()
        .expect("duplicate");

    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        drop(watcher);
        done.send(()).expect("report");
    });
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("dropping the watcher waited for a poll that was only parked");

    // SAFETY: a live event owned by `bell` for the duration of the call.
    unsafe { windows_sys::Win32::System::Threading::ResetEvent(bell.as_raw_handle()) };
    ring(&resume);
    std::thread::sleep(Duration::from_millis(200));
    // SAFETY: as above.
    let quiet = unsafe { WaitForSingleObject(bell.as_raw_handle(), 0) != WAIT_OBJECT_0 };
    assert!(quiet, "a callback ran after the watcher was dropped");
    drop(registrations);
}
