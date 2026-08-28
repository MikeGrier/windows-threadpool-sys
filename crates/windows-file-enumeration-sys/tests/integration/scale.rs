// Copyright (c) 2026 Mike Grier
//! Thousands of entries, a completion ring too small to hold them all at
//! once, and both buffer extremes -- the scenarios that only show up at
//! scale, which is why they belong here rather than in the crate's unit
//! tests.

use windows_file_enumeration_sys::{
    DEFAULT_BUFFER_CAPACITY, EnumerationRequest, MINIMUM_BUFFER_CAPACITY,
    MINIMUM_COMPLETION_RING_CAPACITY, Session,
};

use crate::support::{Scratch, borrow_all, drain_to_terminal, entry_names, many_file_names};

/// Large enough that no single native buffer holds every record in one
/// refill, and large enough that the record/time quantum budget spans
/// several quanta even with the default buffer.
const MANY: usize = 4_000;

#[test]
fn a_directory_with_thousands_of_entries_delivers_every_one_exactly_once() {
    let names = many_file_names(MANY);
    let borrowed = borrow_all(&names);
    let scratch = Scratch::with_files(&borrowed);

    let (session, receiver) = Session::new(16, 256).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    let mut delivered = entry_names(&entries);
    delivered.sort();
    let mut expected = names;
    expected.sort();
    assert_eq!(delivered, expected, "no entry lost or duplicated at scale");
}

#[test]
fn a_completion_ring_far_smaller_than_the_directory_still_delivers_everything() {
    // The smallest ring the contract allows: every entry beyond the first
    // must park and resume purely from the receiver's own draining, with no
    // polling and no lost wakeups, or this would hang rather than finish.
    let names = many_file_names(500);
    let borrowed = borrow_all(&names);
    let scratch = Scratch::with_files(&borrowed);

    let (session, receiver) = Session::new(8, MINIMUM_COMPLETION_RING_CAPACITY).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    let mut delivered = entry_names(&entries);
    delivered.sort();
    let mut expected = names;
    expected.sort();
    assert_eq!(delivered, expected);
}

#[test]
fn the_minimum_and_default_buffer_capacities_agree_on_the_same_directory() {
    let names = many_file_names(300);
    let borrowed = borrow_all(&names);
    let scratch = Scratch::with_files(&borrowed);

    let (session, receiver) = Session::new(16, 512).expect("room");

    let minimum = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_buffer_capacity(MINIMUM_BUFFER_CAPACITY)
        .expect("representable");
    let minimum_handle = session.try_begin(minimum).expect("room");
    let minimum_id = minimum_handle.id();
    minimum_handle.detach();
    let (minimum_entries, minimum_outcome) = drain_to_terminal(&receiver, minimum_id);
    assert!(minimum_outcome.is_completed(), "{minimum_outcome:?}");

    let default = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_buffer_capacity(DEFAULT_BUFFER_CAPACITY)
        .expect("representable");
    let default_handle = session.try_begin(default).expect("room");
    let default_id = default_handle.id();
    default_handle.detach();
    let (default_entries, default_outcome) = drain_to_terminal(&receiver, default_id);
    assert!(default_outcome.is_completed(), "{default_outcome:?}");

    let mut minimum_names = entry_names(&minimum_entries);
    let mut default_names = entry_names(&default_entries);
    minimum_names.sort();
    default_names.sort();
    assert_eq!(
        minimum_names, default_names,
        "the buffer size must never change what a directory contains"
    );
    let mut expected = names;
    expected.sort();
    assert_eq!(minimum_names, expected);
}
