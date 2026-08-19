// Copyright (c) 2026 Mike Grier
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
    FILE_ACTION_RENAMED_OLD_NAME,
};

use super::{Change, ChangeKind, DecodedBatch, DesyncCause, decode_batch, records};

/// UTF-16 code units of `s`.
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Encode one `FILE_NOTIFY_INFORMATION` record with an explicit `NextEntryOffset`
/// and a name length taken from `name` (unless overridden by the caller building
/// a malformed buffer by hand).
fn record(next_offset: u32, action: u32, name: &[u16]) -> Vec<u8> {
    let name_bytes: Vec<u8> = name.iter().flat_map(|u| u.to_le_bytes()).collect();
    let mut v = Vec::new();
    v.extend_from_slice(&next_offset.to_le_bytes());
    v.extend_from_slice(&action.to_le_bytes());
    v.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
    v.extend_from_slice(&name_bytes);
    v
}

/// DWORD-aligned byte length of a record with `name_units` code units, matching
/// how the kernel lays records out.
fn aligned(name_units: usize) -> usize {
    (12 + name_units * 2).next_multiple_of(4)
}

/// Build a properly chained, DWORD-aligned buffer of `(action, name)` records.
fn chain(recs: &[(u32, Vec<u16>)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (i, (action, name)) in recs.iter().enumerate() {
        let is_last = i + 1 == recs.len();
        let this = aligned(name.len());
        let next = if is_last { 0 } else { this as u32 };
        let start = buf.len();
        buf.extend(record(next, *action, name));
        buf.resize(start + this, 0);
    }
    buf
}

/// Decode `buf`, asserting it produced changes rather than a desync.
fn changes(buf: &[u8]) -> Vec<Change> {
    match decode_batch(buf) {
        DecodedBatch::Changes(c) => c,
        DecodedBatch::Desync(cause) => panic!("expected changes, got desync {cause:?}"),
    }
}

// --- normal cases ---

#[test]
fn single_added_record() {
    let c = changes(&chain(&[(FILE_ACTION_ADDED, w("a.txt"))]));
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::Added);
    assert_eq!(c[0].name.as_wide(), w("a.txt").as_slice());
}

#[test]
fn every_recognised_action_maps() {
    let c = changes(&chain(&[
        (FILE_ACTION_ADDED, w("added")),
        (FILE_ACTION_REMOVED, w("removed")),
        (FILE_ACTION_MODIFIED, w("modified")),
        (FILE_ACTION_RENAMED_OLD_NAME, w("old")),
        (FILE_ACTION_RENAMED_NEW_NAME, w("new")),
    ]));
    let kinds: Vec<_> = c.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![
            ChangeKind::Added,
            ChangeKind::Removed,
            ChangeKind::Modified,
            ChangeKind::RenamedOldName,
            ChangeKind::RenamedNewName,
        ]
    );
}

#[test]
fn rename_pair_is_kept_distinct_not_joined() {
    let c = changes(&chain(&[
        (FILE_ACTION_RENAMED_OLD_NAME, w("before.txt")),
        (FILE_ACTION_RENAMED_NEW_NAME, w("after.txt")),
    ]));
    assert_eq!(c.len(), 2, "the pair must not be joined into one event");
    assert_eq!(c[0].kind, ChangeKind::RenamedOldName);
    assert_eq!(c[1].kind, ChangeKind::RenamedNewName);
}

#[test]
fn unknown_action_is_preserved() {
    let c = changes(&chain(&[(4242, w("mystery"))]));
    assert_eq!(c[0].kind, ChangeKind::Unknown(4242));
}

#[test]
fn name_round_trips_to_os_string() {
    let c = changes(&chain(&[(FILE_ACTION_ADDED, w("café.txt"))]));
    assert_eq!(
        c[0].name.to_os_string(),
        std::ffi::OsString::from("café.txt")
    );
}

#[test]
fn name_round_trips_to_path_buf() {
    let c = changes(&chain(&[(FILE_ACTION_MODIFIED, w("dir\\leaf.txt"))]));
    assert_eq!(
        c[0].name.to_path_buf(),
        std::path::PathBuf::from("dir\\leaf.txt")
    );
}

#[test]
fn raw_units_are_preserved() {
    let units = w("preserved");
    let c = changes(&chain(&[(FILE_ACTION_ADDED, units.clone())]));
    assert_eq!(c[0].name.as_wide(), units.as_slice());
}

#[test]
fn two_records_in_order() {
    let c = changes(&chain(&[
        (FILE_ACTION_ADDED, w("first")),
        (FILE_ACTION_REMOVED, w("second")),
    ]));
    assert_eq!(c[0].name.as_wide(), w("first").as_slice());
    assert_eq!(c[1].name.as_wide(), w("second").as_slice());
}

#[test]
fn long_name_beyond_max_path() {
    let long = "x".repeat(400);
    let c = changes(&chain(&[(FILE_ACTION_ADDED, w(&long))]));
    assert_eq!(c[0].name.as_wide().len(), 400);
    assert_eq!(c[0].name.to_os_string(), std::ffi::OsString::from(&long));
}

#[test]
fn non_ascii_bmp_name() {
    let c = changes(&chain(&[(FILE_ACTION_ADDED, w("日本語.txt"))]));
    assert_eq!(
        c[0].name.to_os_string(),
        std::ffi::OsString::from("日本語.txt")
    );
}

#[test]
fn unpaired_surrogate_is_preserved_losslessly() {
    // A lone high surrogate is invalid Unicode but legal on NTFS; it must survive
    // both the raw and the OsString views without loss.
    let name = vec![0xD800_u16, u16::from(b'A')];
    let c = changes(&chain(&[(FILE_ACTION_ADDED, name.clone())]));
    assert_eq!(c[0].name.as_wide(), name.as_slice());
    let round_trip: Vec<u16> = c[0].name.to_os_string().encode_wide().collect();
    assert_eq!(round_trip, name);
}

#[test]
fn empty_name_record() {
    let c = changes(&chain(&[(FILE_ACTION_ADDED, Vec::new())]));
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, ChangeKind::Added);
    assert!(c[0].name.as_wide().is_empty());
}

// --- overflow / desync boundary ---

#[test]
fn empty_buffer_is_overflow_desync() {
    assert_eq!(
        decode_batch(&[]),
        DecodedBatch::Desync(DesyncCause::Overflow)
    );
}

// --- malformed buffers must not read out of bounds ---

#[test]
fn truncated_header_yields_no_records() {
    // Non-empty but shorter than a header: decodes to no changes, no panic.
    assert_eq!(changes(&[1, 2, 3, 4, 5]), Vec::new());
}

#[test]
fn name_length_overrunning_the_buffer_stops() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0_u32.to_le_bytes()); // NextEntryOffset = last
    buf.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
    buf.extend_from_slice(&100_u32.to_le_bytes()); // claims 100 name bytes
    buf.extend_from_slice(&[0x41, 0x00]); // but only 2 are present
    assert_eq!(changes(&buf), Vec::new());
}

#[test]
fn next_offset_out_of_bounds_stops_after_current() {
    // First record is valid; its NextEntryOffset points past the buffer, so only
    // the first record is decoded and iteration stops without an OOB read.
    let buf = record(10_000, FILE_ACTION_ADDED, &w("a"));
    let c = changes(&buf);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name.as_wide(), w("a").as_slice());
}

#[test]
fn next_offset_pointing_into_current_record_stops_after_current() {
    // NextEntryOffset = 4 points back inside this record's own header+name span
    // (an overlapping/garbage link). The first record still decodes, then
    // iteration stops rather than re-reading overlapping bytes.
    let buf = record(4, FILE_ACTION_ADDED, &w("ab"));
    let c = changes(&buf);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name.as_wide(), w("ab").as_slice());
}

#[test]
fn unaligned_next_offset_stops_after_current() {
    // A well-formed NextEntryOffset is DWORD-aligned. An offset that clears the
    // current record but is not a multiple of 4 is malformed: the first record
    // decodes, then iteration stops.
    let first = record(0, FILE_ACTION_ADDED, &w("a")); // 12 + 2 = 14 bytes
    let mut buf = record(15, FILE_ACTION_ADDED, &w("a")); // 15 is >= span but unaligned
    buf.resize(15, 0);
    buf.extend(first);
    let c = changes(&buf);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name.as_wide(), w("a").as_slice());
}

#[test]
fn odd_name_length_drops_the_trailing_byte() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0_u32.to_le_bytes());
    buf.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
    buf.extend_from_slice(&3_u32.to_le_bytes()); // odd byte length
    buf.extend_from_slice(&[0x41, 0x00, 0x42]); // 'A' then a stray byte
    let c = changes(&buf);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name.as_wide(), &[u16::from(b'A')]);
}

// --- the internal raw walk ---

#[test]
fn raw_records_expose_the_unmapped_action() {
    let buf = chain(&[(FILE_ACTION_MODIFIED, w("x"))]);
    let recs: Vec<_> = records(&buf).collect();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].action, FILE_ACTION_MODIFIED);
    assert_eq!(recs[0].name.as_wide(), w("x").as_slice());
}
