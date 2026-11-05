// Copyright (c) 2026 Mike Grier
//! Cancellation at each phase, and receiver abandonment, against the live
//! thread pool and real directories.

use std::time::{Duration, Instant};

use windows_file_enumeration_sys::{
    Completion, DEFAULT_BUFFER_CAPACITY, EnumerationRequest, MINIMUM_BUFFER_CAPACITY, Session,
    TerminalOutcome,
};

use crate::support::{Scratch, borrow_all, many_file_names};

#[test]
fn cancelling_before_any_quantum_runs_still_yields_one_terminal() {
    // The handle is cancelled before the session has even been given a
    // chance to service the begin, let alone run it.
    let scratch = Scratch::with_files(&["a.txt", "b.txt", "c.txt"]);
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.cancel();

    let outcome = loop {
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Some(Completion::Terminal {
                enumeration: id,
                outcome,
            }) => {
                assert_eq!(id, enumeration);
                break outcome;
            }
            Some(Completion::Entry { .. }) => continue,
            None => panic!("no terminal arrived"),
        }
    };
    assert!(matches!(outcome, TerminalOutcome::Cancelled), "{outcome:?}");
    assert!(receiver.try_recv().is_none(), "exactly one terminal");
}

/// A directory of `count` files, which a request with a native buffer of
/// `buffer` bytes enumerates over many refills and quanta, so that an
/// enumeration is still running when the test acts on it.
fn running_enumeration(count: usize, buffer: usize) -> (Scratch, EnumerationRequest) {
    let names = many_file_names(count);
    let borrowed = borrow_all(&names);
    let scratch = Scratch::with_files(&borrowed);
    let request = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_buffer_capacity(buffer)
        .expect("representable");
    (scratch, request)
}

/// More than two quanta of records (a quantum is at most 256), which with the
/// minimum buffer is also many refills. Creating the files is what a test
/// here costs, so this is as few as keeps an enumeration running for many
/// quanta; [`FULL`] is the scale the claims were first made at.
const SPANNING: usize = 600;

/// Thousands of entries through the default buffer. See [`long_haul`].
const FULL: usize = 4_000;

/// Cancelling an enumeration of `count` entries ends it with exactly one
/// terminal and no entry after it.
fn cancelling_ends_it_with_no_entry_after_the_terminal(count: usize, buffer: usize) {
    // Many quanta keep a real worker busy, giving the cancellation a real
    // chance to land on a running, not yet finished, enumeration -- proof
    // against the live engine, not a scripted one.
    let (_scratch, request) = running_enumeration(count, buffer);

    let (session, receiver) = Session::new(8, 64).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.cancel();

    let mut saw_terminal = false;
    let mut entries_seen = 0usize;
    loop {
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Some(Completion::Entry { .. }) => {
                assert!(!saw_terminal, "an entry arrived after the terminal");
                entries_seen += 1;
            }
            Some(Completion::Terminal {
                enumeration: id,
                outcome,
            }) => {
                assert_eq!(id, enumeration);
                assert!(!saw_terminal, "more than one terminal");
                saw_terminal = true;
                // Cancelling this early against a directory this large should
                // reliably beat the enumeration to completion; if it somehow
                // did not, `Completed` is still an acceptable outcome of an
                // inherently asynchronous cancel.
                assert!(
                    matches!(
                        outcome,
                        TerminalOutcome::Cancelled | TerminalOutcome::Completed
                    ),
                    "{outcome:?}"
                );
                break;
            }
            None => panic!("no terminal arrived"),
        }
    }
    assert!(saw_terminal);
    let _ = entries_seen;
}

#[test]
fn cancelling_a_running_enumeration_over_many_refills_ends_it_with_no_entry_after_the_terminal() {
    cancelling_ends_it_with_no_entry_after_the_terminal(SPANNING, MINIMUM_BUFFER_CAPACITY);
}

#[test]
fn cancelling_an_already_completed_enumeration_adds_no_second_terminal() {
    let scratch = Scratch::empty();
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();

    // Let it run to completion first.
    let outcome = loop {
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Some(Completion::Terminal {
                enumeration: id,
                outcome,
            }) => {
                assert_eq!(id, enumeration);
                break outcome;
            }
            Some(Completion::Entry { .. }) => continue,
            None => panic!("no terminal arrived"),
        }
    };
    assert!(outcome.is_completed(), "{outcome:?}");

    // A cancel that loses the race entirely is not an error, and must not
    // manufacture a second terminal for an enumeration that is already gone.
    handle.cancel();
    assert!(
        receiver.recv_timeout(Duration::from_millis(200)).is_none(),
        "no second terminal for an enumeration that already finished"
    );
}

/// Dropping the receiver of an enumeration of `count` entries returns at once
/// and releases the enumeration.
fn dropping_the_receiver_ends_it_promptly(count: usize, buffer: usize) {
    let (_scratch, request) = running_enumeration(count, buffer);

    let (session, receiver) = Session::new(8, 64).expect("room");
    let handle = session.try_begin(request).expect("room");
    handle.detach();

    let started = Instant::now();
    drop(receiver);
    let dropped_in = started.elapsed();
    assert!(
        dropped_in < Duration::from_millis(500),
        "abandonment must not wait on a directory query: {dropped_in:?}"
    );

    for _ in 0..2000 {
        if session.enumerations() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the abandoned enumeration was never released");
}

#[test]
fn dropping_the_receiver_ends_an_enumeration_running_over_many_refills_promptly() {
    dropping_the_receiver_ends_it_promptly(SPANNING, MINIMUM_BUFFER_CAPACITY);
}

/// The two claims above at the scale they were first made at: thousands of
/// entries through the default buffer, which is mostly the time to create
/// them. Nothing about either differs from its small form but the size.
mod long_haul {
    use super::*;

    #[test]
    #[ignore = "long-haul: creates 4000 files and cancels an enumeration of them"]
    fn cancelling_a_large_running_enumeration_ends_it_with_no_entry_after_the_terminal() {
        cancelling_ends_it_with_no_entry_after_the_terminal(FULL, DEFAULT_BUFFER_CAPACITY);
    }

    #[test]
    #[ignore = "long-haul: creates 4000 files and drops the receiver of an enumeration of them"]
    fn dropping_the_receiver_ends_a_large_running_enumeration_promptly() {
        dropping_the_receiver_ends_it_promptly(FULL, DEFAULT_BUFFER_CAPACITY);
    }
}
