// Copyright (c) 2026 Mike Grier
//! At least ten ordinary directories, plus empty, single-entry, and a mix of
//! files and subdirectories.

use windows_file_enumeration_sys::{EntryType, EnumerationRequest, Session};

use crate::support::{Scratch, drain_many, drain_to_terminal, entry_names};

#[test]
fn ten_ordinary_directories_each_deliver_exactly_their_own_files() {
    // Ten independent directories, ten independent enumerations, sharing one
    // session -- proof that ordinary use is simply ordinary, ten times over.
    let scratches: Vec<Scratch> = (0..10)
        .map(|index| {
            let names: Vec<String> = (0..(index + 1))
                .map(|entry| format!("dir{index}-file{entry}.txt"))
                .collect();
            let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
            Scratch::with_files(&borrowed)
        })
        .collect();

    let (session, receiver) = Session::new(64, 64).expect("room");
    let mut expected = Vec::new();
    let mut handles = Vec::new();
    for (index, scratch) in scratches.iter().enumerate() {
        let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
        let handle = session.try_begin(request).expect("room");
        let mut names: Vec<String> = (0..=index)
            .map(|entry| format!("dir{index}-file{entry}.txt"))
            .collect();
        names.sort();
        expected.push((handle.id(), names));
        handles.push(handle);
    }
    for handle in handles {
        handle.detach();
    }

    let enumerations: Vec<_> = expected.iter().map(|(id, _)| *id).collect();
    let mut finished = drain_many(&receiver, &enumerations);
    for (enumeration, mut names) in expected {
        let (entries, outcome) = finished.remove(&enumeration).expect("this enumeration ran");
        assert!(outcome.is_completed(), "{outcome:?}");
        let mut delivered = entry_names(&entries);
        delivered.sort();
        names.sort();
        assert_eq!(delivered, names);
    }
}

#[test]
fn an_empty_directory_completes_with_no_entries() {
    let scratch = Scratch::empty();
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert!(entries.is_empty(), "{entries:?}");
}

#[test]
fn a_single_entry_directory_delivers_exactly_that_entry() {
    let scratch = Scratch::with_files(&["only.txt"]);
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entry_names(&entries), ["only.txt"]);
}

#[test]
fn files_and_subdirectories_are_both_reported_with_the_right_entry_type() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt"]);
    scratch.subdir("child-one");
    scratch.subdir("child-two");

    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entries.len(), 4);

    for record in &entries {
        let windows_file_enumeration_sys::Completion::Entry { entry, .. } = record else {
            unreachable!("terminals are never collected as entries");
        };
        let name = entry.name().to_string_lossy();
        let expected_type = if name.starts_with("child") {
            EntryType::Directory
        } else {
            EntryType::File
        };
        assert_eq!(entry.entry_type(), expected_type, "{name}");
    }
}
