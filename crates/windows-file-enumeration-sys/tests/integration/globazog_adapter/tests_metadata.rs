// Copyright (c) 2026 Mike Grier
//! Metadata and file-identity fidelity through the adapter: native names
//! (including an unpaired surrogate), entry type, reparse status and tag,
//! raw attributes, both sizes, all four times, and volume-qualified
//! identity.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_file_enumeration_sys::{Completion, EnumerationRequest, Session};

use crate::globazog_adapter::adapter::enumerate_dir_native_via_wfe;
use crate::globazog_adapter::tests_support::ascii_name;
use crate::globazog_adapter::types::{self, EnumPlan, FileId};
use crate::support::{Scratch, create_junction};

#[test]
fn native_names_survive_the_round_trip_including_an_unpaired_surrogate() {
    let scratch = Scratch::empty();
    let mut units: Vec<u16> = "surrogate-".encode_utf16().collect();
    units.push(0xD800); // An unpaired high surrogate: no low surrogate follows.
    units.extend(".dat".encode_utf16());
    let name = OsString::from_wide(&units);
    if std::fs::write(scratch.path().join(&name), b"").is_err() {
        // An environmental fact this adapter cannot change; see paths.rs's
        // identical acknowledgement for the crate's own equivalent test.
        eprintln!("skipping: this filesystem rejected an unpaired-surrogate name");
        return;
    }

    let scan = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL).expect("a scan");
    assert_eq!(scan.entries.len(), 1);
    let expected = types::decode_utf16(&units);
    assert_eq!(
        scan.entries[0].name, expected,
        "the unpaired surrogate must survive verbatim, not be replaced or dropped"
    );
}

#[test]
fn entry_type_reparse_status_tag_and_raw_attributes_survive_through_the_adapter() {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

    let scratch = Scratch::with_files(&["plain.dat"]);
    let plain_dir = scratch.subdir("plain-dir");
    let target = plain_dir.join("target");
    std::fs::create_dir_all(&target).expect("a nested target directory");
    let link = scratch.child("a-junction");
    create_junction(&link, &target);

    let scan = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL).expect("a scan");
    assert_eq!(
        scan.entries.len(),
        3,
        "{:?}",
        scan.entries.iter().map(ascii_name).collect::<Vec<_>>()
    );

    for entry in &scan.entries {
        match ascii_name(entry).as_str() {
            "plain.dat" => {
                assert_eq!(entry.entry_type, types::EntryType::File);
                assert!(!entry.is_reparse);
                assert_eq!(entry.reparse_tag, 0);
            }
            "plain-dir" => {
                assert_eq!(entry.entry_type, types::EntryType::Dir);
                assert!(!entry.is_reparse);
            }
            "a-junction" => {
                assert_eq!(entry.entry_type, types::EntryType::Dir);
                assert!(entry.is_reparse);
                assert_eq!(entry.reparse_tag, IO_REPARSE_TAG_MOUNT_POINT);
                assert_eq!(
                    entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT,
                    FILE_ATTRIBUTE_REPARSE_POINT
                );
            }
            other => panic!("unexpected entry: {other}"),
        }
    }
}

#[test]
fn both_sizes_survive_in_the_replaced_engine_even_though_direntry_carries_only_one() {
    let scratch = Scratch::empty();
    let path = scratch.child("sized.dat");
    std::fs::write(&path, vec![0xAB_u8; 5000]).expect("a file");
    let expected_len = std::fs::metadata(&path).expect("metadata").len();

    // The adapter's own `DirEntry`, matching Globazog's real shape, carries
    // only the logical size (`EndOfFile`) -- exactly what Globazog's own
    // `size` field is documented to mean.
    let scan = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL).expect("a scan");
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(scan.entries[0].size, expected_len);

    // Directly against the replaced engine's own public API: the allocation
    // size the adapter's translation does not carry forward is still fully
    // present, so nothing about the *replaced layer* lost it -- Globazog's
    // own `DirEntry` simply never asked for it.
    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    handle.detach();
    let entry = match receiver.recv_timeout(Duration::from_secs(10)) {
        Some(Completion::Entry { entry, .. }) => entry,
        Some(Completion::Terminal { .. }) => panic!("no entry was delivered"),
        None => panic!("no completion arrived"),
    };
    assert_eq!(entry.logical_size(), expected_len);
    assert!(
        entry.allocation_size() >= entry.logical_size(),
        "allocation never falls short of logical size for an ordinary file this small"
    );
}

#[test]
fn all_four_times_translate_to_unix_nanoseconds_in_a_plausible_range() {
    let tolerance = Duration::from_secs(30).as_nanos() as i64;
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos() as i64
        - tolerance;

    let scratch = Scratch::with_files(&["timed.dat"]);
    let scan = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL).expect("a scan");
    assert_eq!(scan.entries.len(), 1);
    let entry = &scan.entries[0];

    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos() as i64
        + tolerance;

    for (label, value) in [
        ("btime", entry.btime),
        ("mtime", entry.mtime),
        ("atime", entry.atime),
        ("ctime", entry.ctime),
    ] {
        assert!(
            value > before && value < after,
            "{label} = {value} is not a plausible Unix-nanosecond timestamp for a file just created"
        );
    }
}

#[test]
fn volume_qualified_identity_is_filled_only_when_requested() {
    let scratch = Scratch::with_files(&["a.dat"]);

    let omitted = enumerate_dir_native_via_wfe(
        scratch.path(),
        EnumPlan {
            want_stat: true,
            want_file_id: false,
            want_reparse_file_id: false,
        },
    )
    .expect("a scan");
    assert_eq!(omitted.entries.len(), 1);
    assert_eq!(
        omitted.entries[0].file_id,
        FileId { volume: 0, id: 0 },
        "identity not requested stays unknown, matching win.rs's own contract"
    );

    let requested = enumerate_dir_native_via_wfe(scratch.path(), EnumPlan::FULL).expect("a scan");
    assert_eq!(requested.entries.len(), 1);
    assert_ne!(
        requested.entries[0].file_id.volume, 0,
        "a local disk always answers the volume-serial query"
    );
}
