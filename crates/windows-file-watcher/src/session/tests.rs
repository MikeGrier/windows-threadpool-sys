// Copyright (c) 2026 Mike Grier
//! Unit tests for sessions.
//!
//! What is under test is the binding a session establishes: which receiver its
//! notifications reach, that separate sessions do not leak into one another, and
//! that the binding survives cloning and outlives the monitor in the defined way.
//!
//! Notifications are synthesised rather than produced by a real watcher, because
//! nothing creates a watcher *from a session* until M3.5; what M3.2 owns is the
//! wiring, and a synthetic notification exercises exactly that.

use std::time::Duration;

use crate::monitor::Monitor;
use crate::notify::DesyncCause;
use crate::queue::{Notification, WatchId};

/// Upper bound for waiting on a notification that has already been enqueued.
const RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// A distinguishable notification, so a test can tell whose stream it came from.
fn marker(watch: u64) -> Notification {
    Notification::Desync {
        watch: WatchId::from_raw(watch),
        cause: DesyncCause::Overflow,
    }
}

#[test]
fn a_session_delivers_to_its_own_receiver() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    session.sink().send(marker(1));

    let received = receiver
        .recv_timeout(RECV_TIMEOUT)
        .expect("the notification");
    assert_eq!(received.watch(), WatchId::from_raw(1));
}

#[test]
fn notifications_arrive_in_submission_order() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    for watch in 0..64 {
        session.sink().send(marker(watch));
    }

    let seen: Vec<u64> = (0..64)
        .map(|_| {
            receiver
                .recv_timeout(RECV_TIMEOUT)
                .expect("the notification")
                .watch()
                .get()
        })
        .collect();
    let expected: Vec<u64> = (0..64).collect();
    assert_eq!(
        seen, expected,
        "delivery within a session is ordered (D-12)"
    );
}

#[test]
fn two_sessions_have_independent_streams() {
    let monitor = Monitor::new().expect("create the monitor");
    let (first, first_receiver) = monitor.session();
    let (second, second_receiver) = monitor.session();

    first.sink().send(marker(1));
    second.sink().send(marker(2));

    // The binding is per session, not per monitor: a client that opened two
    // streams must not have to filter one out of the other.
    assert_eq!(
        first_receiver
            .recv_timeout(RECV_TIMEOUT)
            .expect("the first notification")
            .watch(),
        WatchId::from_raw(1)
    );
    assert_eq!(
        second_receiver
            .recv_timeout(RECV_TIMEOUT)
            .expect("the second notification")
            .watch(),
        WatchId::from_raw(2)
    );
    assert!(first_receiver.is_empty(), "no cross-talk between sessions");
    assert!(second_receiver.is_empty(), "no cross-talk between sessions");
}

#[test]
fn a_clone_delivers_to_the_same_receiver() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let clone = session.clone();

    clone.sink().send(marker(7));

    assert_eq!(
        receiver
            .recv_timeout(RECV_TIMEOUT)
            .expect("the notification")
            .watch(),
        WatchId::from_raw(7)
    );
}

#[test]
fn many_producers_deliver_to_one_receiver() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();

    // The multi-producer floor D-11 imposes: several client threads, one stream.
    let producers: Vec<_> = (0..8)
        .map(|producer| {
            let session = session.clone();
            std::thread::spawn(move || {
                for index in 0..100 {
                    session.sink().send(marker(producer * 100 + index));
                }
            })
        })
        .collect();
    for producer in producers {
        producer.join().expect("producer");
    }

    let mut seen: Vec<u64> = (0..800)
        .map(|_| {
            receiver
                .recv_timeout(RECV_TIMEOUT)
                .expect("the notification")
                .watch()
                .get()
        })
        .collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 800, "every notification arrived exactly once");
}

#[test]
fn dropping_every_session_disconnects_the_receiver() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let clone = session.clone();

    drop(session);
    assert!(
        !receiver.is_disconnected(),
        "a surviving clone still holds the stream open"
    );

    drop(clone);
    assert!(
        receiver.is_disconnected(),
        "the last session away ends the stream, so a `recv` loop terminates"
    );
    assert!(receiver.recv().is_none());
}

#[test]
fn a_session_is_open_while_its_monitor_runs() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();
    assert!(session.is_open());
}

#[test]
fn shutting_down_the_monitor_closes_every_session() {
    let monitor = Monitor::new().expect("create the monitor");
    let (first, _first_receiver) = monitor.session();
    let (second, _second_receiver) = monitor.session();

    monitor.shut_down();

    assert!(!first.is_open());
    assert!(!second.is_open());
}

#[test]
fn a_session_outliving_its_monitor_reports_itself_closed() {
    let (session, _receiver) = {
        let monitor = Monitor::new().expect("create the monitor");
        monitor.session()
    };

    // The alternative -- a forgotten session keeping the monitor and its watchers
    // alive -- would make teardown depend on the client's drop order (D-20).
    assert!(
        !session.is_open(),
        "the monitor's teardown must not wait on a session the client still holds"
    );
}

#[test]
fn a_session_outliving_its_monitor_still_delivers_what_it_holds() {
    let (session, receiver) = {
        let monitor = Monitor::new().expect("create the monitor");
        monitor.session()
    };

    // The notification queue is not the monitor's; it belongs to the session and
    // its receiver, so shutting the monitor down must not sever a stream the
    // client is still draining.
    session.sink().send(marker(3));
    assert_eq!(
        receiver
            .recv_timeout(RECV_TIMEOUT)
            .expect("the notification")
            .watch(),
        WatchId::from_raw(3)
    );
}

#[test]
fn sessions_can_be_opened_from_several_threads() {
    let monitor = Monitor::new().expect("create the monitor");
    let monitor = &monitor;

    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|index| {
                scope.spawn(move || {
                    let (session, receiver) = monitor.session();
                    session.sink().send(marker(index));
                    assert_eq!(
                        receiver
                            .recv_timeout(RECV_TIMEOUT)
                            .expect("the notification")
                            .watch(),
                        WatchId::from_raw(index)
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("worker");
        }
    });
}

#[test]
fn debug_reports_whether_the_session_is_open() {
    let monitor = Monitor::new().expect("create the monitor");
    let (session, _receiver) = monitor.session();

    assert!(format!("{session:?}").contains("open: true"));
    monitor.shut_down();
    assert!(format!("{session:?}").contains("open: false"));
}
