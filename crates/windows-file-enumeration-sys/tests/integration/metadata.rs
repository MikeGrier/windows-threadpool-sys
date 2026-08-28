// Copyright (c) 2026 Mike Grier
//! Metadata and file-identity fidelity: what the crate delivers, cross-checked
//! against what the standard library's own metadata query reports for the
//! same real file.

use windows_file_enumeration_sys::{Completion, EnumerationRequest, FileIdentityMode, Session};

use crate::support::{Scratch, drain_to_terminal};

#[test]
fn logical_size_matches_the_standard_librarys_own_metadata() {
    let scratch = Scratch::empty();
    let path = scratch.child("sized.dat");
    std::fs::write(&path, vec![0xAB_u8; 12_345]).expect("a file");
    let expected_len = std::fs::metadata(&path).expect("metadata").len();

    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entries.len(), 1);
    let Completion::Entry { entry, .. } = &entries[0] else {
        unreachable!();
    };
    assert_eq!(entry.logical_size(), expected_len);
    assert!(
        entry.allocation_size() >= entry.logical_size(),
        "allocation never falls short of logical size for an ordinary file this small"
    );
}

#[test]
fn every_inline_field_survives_for_a_directory_and_a_file() {
    let scratch = Scratch::with_files(&["plain.txt"]);
    scratch.subdir("plain-dir");

    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entries.len(), 2);

    for record in &entries {
        let Completion::Entry { entry, .. } = record else {
            unreachable!();
        };
        // Every field is reachable and self-consistent; the specific values
        // are exercised in `predicates.rs` and the size check above.
        let _ = entry.attributes();
        let _ = entry.creation_time();
        let _ = entry.last_access_time();
        let _ = entry.last_write_time();
        let _ = entry.change_time();
        let _ = entry.extended_attribute_size();
        assert!(
            !entry.identity().is_volume_qualified(),
            "omitted by default"
        );
    }
}

#[test]
fn best_effort_identity_is_volume_qualified_on_a_local_disk() {
    let scratch = Scratch::with_files(&["a.txt"]);
    let request = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_file_identity(FileIdentityMode::BestEffort);

    let (session, receiver) = Session::new(8, 8).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entries.len(), 1);
    let Completion::Entry { entry, .. } = &entries[0] else {
        unreachable!();
    };
    assert!(
        entry.identity().is_volume_qualified(),
        "a local disk always answers the volume-serial query"
    );
}

#[test]
fn two_entries_from_the_same_enumeration_share_a_volume_serial() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt"]);
    let request = EnumerationRequest::for_path(scratch.path())
        .expect("resolvable")
        .with_file_identity(FileIdentityMode::Required);

    let (session, receiver) = Session::new(8, 8).expect("room");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let (entries, outcome) = drain_to_terminal(&receiver, enumeration);
    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(entries.len(), 2);
    let mut serials = entries.iter().map(|record| {
        let Completion::Entry { entry, .. } = record else {
            unreachable!();
        };
        entry.identity().volume_serial().expect("required identity")
    });
    let first = serials.next().expect("an entry");
    assert!(
        serials.all(|serial| serial == first),
        "one directory, one volume"
    );

    let ids: std::collections::HashSet<[u8; 16]> = entries
        .iter()
        .map(|record| {
            let Completion::Entry { entry, .. } = record else {
                unreachable!();
            };
            entry.identity().id_bytes()
        })
        .collect();
    assert_eq!(ids.len(), 2, "distinct files must carry distinct file ids");
}
