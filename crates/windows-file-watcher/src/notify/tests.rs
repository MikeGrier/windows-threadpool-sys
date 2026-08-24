// Copyright (c) 2026 Mike Grier
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
    FILE_ACTION_RENAMED_OLD_NAME,
};

use super::{Change, ChangeKind, DecodedBatch, DesyncCause, decode_batch, records};

/// A synthetic `FILE_ACTION_*` code the decoder does not recognise, used to check
/// it is preserved verbatim as `ChangeKind::Unknown`. Not a real Windows action.
const UNRECOGNISED_ACTION: u32 = 4242;

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
    (super::HEADER_LEN + name_units * super::UNIT_LEN).next_multiple_of(super::RECORD_ALIGNMENT)
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

/// Decode `buf`, asserting it signalled a desync rather than changes, and return
/// the cause.
fn desync(buf: &[u8]) -> DesyncCause {
    match decode_batch(buf) {
        DecodedBatch::Desync(cause) => cause,
        DecodedBatch::Changes(c) => panic!("expected a desync, got changes {c:?}"),
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
    let c = changes(&chain(&[(UNRECOGNISED_ACTION, w("mystery"))]));
    assert_eq!(c[0].kind, ChangeKind::Unknown(UNRECOGNISED_ACTION));
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
fn name_filling_the_record_to_the_buffer_edge() {
    // Upper boundary of the in-bounds check: a single record whose name runs
    // exactly to the end of the buffer (name_end == rec.len(), no trailing
    // padding) must decode cleanly rather than desync. The crate imposes no
    // maximum name length; `FileNameLength` is bounded only by the buffer.
    let name = w("boundary-name");
    let buf = record(0, FILE_ACTION_ADDED, &name);
    assert_eq!(
        buf.len(),
        super::HEADER_LEN + name.len() * super::UNIT_LEN,
        "name must reach the buffer edge"
    );
    let c = changes(&buf);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name.as_wide(), name.as_slice());
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

// --- malformed buffers desync rather than silently claiming success ---

#[test]
fn truncated_header_is_desync() {
    // Non-empty but shorter than a header: a partial buffer the crate cannot
    // parse must desync (re-scan), not decode to an empty "synchronized" batch.
    assert_eq!(desync(&[1, 2, 3, 4, 5]), DesyncCause::Overflow);
}

#[test]
fn name_length_overrunning_the_buffer_is_desync() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0_u32.to_le_bytes()); // NextEntryOffset = last
    buf.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
    buf.extend_from_slice(&100_u32.to_le_bytes()); // claims 100 name bytes
    buf.extend_from_slice(&[0x41, 0x00]); // but only 2 are present
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn next_offset_out_of_bounds_is_desync() {
    // A dangling NextEntryOffset (points past the buffer) is a corrupt chain: the
    // decode desyncs rather than returning the valid-looking prefix as success.
    let buf = record(10_000, FILE_ACTION_ADDED, &w("a"));
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn next_offset_pointing_into_current_record_is_desync() {
    // NextEntryOffset = 4 points back inside this record's own header+name span
    // (an overlapping/garbage link): a corrupt chain, so the decode desyncs.
    let buf = record(4, FILE_ACTION_ADDED, &w("ab"));
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn unaligned_next_offset_is_desync() {
    // A well-formed NextEntryOffset is DWORD-aligned. An offset that clears the
    // current record but is not a multiple of 4 is a corrupt chain, so the decode
    // desyncs rather than following it.
    let first = record(0, FILE_ACTION_ADDED, &w("a"));
    let mut buf = record(15, FILE_ACTION_ADDED, &w("a")); // 15 >= span but unaligned
    buf.resize(15, 0);
    buf.extend(first);
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn odd_name_length_is_desync() {
    // FileNameLength is the byte length of a UTF-16 sequence, so an odd value is
    // malformed: rather than silently drop the stray byte and claim success, the
    // decode desyncs.
    let mut buf = Vec::new();
    buf.extend_from_slice(&0_u32.to_le_bytes());
    buf.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
    buf.extend_from_slice(&3_u32.to_le_bytes()); // odd byte length
    buf.extend_from_slice(&[0x41, 0x00, 0x42]); // 'A' then a stray byte
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn name_overrunning_next_entry_offset_is_desync() {
    // A non-last record whose FileNameLength reaches past its NextEntryOffset into
    // the following record is malformed: the decoder must desync rather than read
    // the next record's bytes as this record's name.
    let mut buf = Vec::new();
    // Record 0: NextEntryOffset = 16 (so its span is 16 bytes), but a name length
    // claiming 8 bytes would end at 20 — four bytes into record 1.
    buf.extend_from_slice(&16_u32.to_le_bytes());
    buf.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
    buf.extend_from_slice(&8_u32.to_le_bytes()); // claims 8 name bytes
    buf.extend_from_slice(&[0x41, 0x00, 0x42, 0x00]); // only 4 fit before offset 16
    // Record 1 at offset 16: a valid last record whose bytes must not be consumed.
    buf.extend_from_slice(&0_u32.to_le_bytes());
    buf.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
    buf.extend_from_slice(&2_u32.to_le_bytes());
    buf.extend_from_slice(&[0x43, 0x00]);
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn zero_offset_with_trailing_record_is_desync() {
    // A record that zeroes NextEntryOffset (claiming to be last) while a whole
    // further record follows must desync: the tail is data the chain does not
    // describe, and silently treating it as the final record would drop those
    // trailing changes while falsely reporting success.
    let mut buf = record(0, FILE_ACTION_ADDED, &w("a"));
    buf.resize(aligned(1), 0); // pad the "last" record to its DWORD boundary
    // A second, self-consistent record the zeroed link hides.
    buf.extend(record(0, FILE_ACTION_ADDED, &w("b")));
    assert_eq!(desync(&buf), DesyncCause::Overflow);
}

#[test]
fn zero_offset_trailing_alignment_padding_decodes_cleanly() {
    // The complement of the case above: up to RECORD_ALIGNMENT-1 padding bytes may
    // follow the final record, and must not be mistaken for a hidden tail.
    let mut buf = record(0, FILE_ACTION_ADDED, &w("a")); // 14 bytes
    buf.resize(aligned(1), 0); // pad to 16 (two padding bytes)
    let c = changes(&buf);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name.as_wide(), w("a").as_slice());
}

#[test]
fn zero_offset_misaligned_trailing_tail_is_desync() {
    // A name is a whole number of UTF-16 units, so a record always ends on an even
    // offset and its DWORD padding is exactly 0 or 2 bytes -- never 1 or 3. A tail
    // of any other length is undescribed data (a truncated or corrupt completion),
    // not padding, and must desync rather than be silently discarded.
    for extra in [1usize, 3] {
        let mut buf = record(0, FILE_ACTION_ADDED, &w("a")); // 14 bytes
        buf.resize(buf.len() + extra, 0);
        assert_eq!(
            desync(&buf),
            DesyncCause::Overflow,
            "a {extra}-byte tail is not valid DWORD padding"
        );
    }
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

// --- native WTF-16 storage (M8.1) ---

// A real kernel32 wide entry point, declared inline: it counts code units up to
// the terminator, so it proves the pointer we hand over really is a valid
// `LPCWSTR` rather than merely being typed as one.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn lstrlenW(lpstring: *const u16) -> i32;
}

#[test]
fn a_decoded_name_feeds_a_wide_win32_call_with_no_conversion() {
    // The point of storing names as `Wtf16String`: the units the kernel gave us
    // go straight back to Windows, with no re-encode and no allocation. An
    // `OsString` would have had to be converted at this boundary.
    let buf = record(0, FILE_ACTION_ADDED, &w("report.txt"));
    let c = changes(&buf);
    let name = &c[0].name;

    assert!(
        !name.has_interior_nul(),
        "a valid C string needs no NUL inside"
    );
    // SAFETY: `as_terminated_ptr` is NUL-terminated (the owned string always
    // carries a terminator) and has no interior NUL, which is exactly what
    // `lstrlenW` requires; it reads only as far as that terminator.
    let counted = unsafe { lstrlenW(name.as_wtf16().as_terminated_ptr()) };
    assert_eq!(counted as usize, name.len());
    assert_eq!(name.len(), "report.txt".len());
}

#[test]
fn the_deref_surface_exposes_the_borrowed_units() {
    let buf = record(0, FILE_ACTION_ADDED, &w("caf\u{e9}.txt"));
    let c = changes(&buf);
    let name = &c[0].name;

    // Deref to `Wtf16Str`: counted FFI, length, and lossy display for free.
    assert_eq!(name.as_units(), name.as_wide());
    assert_eq!(name.len(), name.as_wide().len());
    assert_eq!(name.to_string_lossy(), "caf\u{e9}.txt");
    assert!(!name.as_ptr().is_null());
}

#[test]
fn debug_escapes_a_lone_surrogate_rather_than_replacing_it() {
    // The previous `OsString`-backed Debug rendered through `to_string_lossy`,
    // collapsing an unpaired surrogate to U+FFFD and making two different
    // malformed names print identically. WTF-16's Debug escapes it instead, so
    // the value stays distinguishable in a diagnostic.
    let high = record(0, FILE_ACTION_ADDED, &[0x61, 0xD800, 0x62]);
    let low = record(0, FILE_ACTION_ADDED, &[0x61, 0xDC00, 0x62]);

    let high_debug = format!("{:?}", changes(&high)[0].name);
    let low_debug = format!("{:?}", changes(&low)[0].name);

    assert!(
        high_debug.contains("d800"),
        "expected an escape, got {high_debug}"
    );
    assert!(
        low_debug.contains("dc00"),
        "expected an escape, got {low_debug}"
    );
    assert_ne!(
        high_debug, low_debug,
        "two distinct malformed names must not render identically"
    );
    assert!(
        !high_debug.contains('\u{fffd}'),
        "a lone surrogate must not be replaced"
    );
}

#[test]
fn an_interior_nul_in_a_name_is_preserved_and_reported() {
    // The kernel is not expected to produce one, but the storage is lossless and
    // the crate must not silently truncate. Content keeps it; the type says so.
    let buf = record(0, FILE_ACTION_ADDED, &[0x61, 0x0000, 0x62]);
    let c = changes(&buf);
    let name = &c[0].name;

    assert_eq!(name.as_wide(), &[0x61, 0x0000, 0x62]);
    assert_eq!(name.len(), 3, "the NUL is content, not a terminator");
    assert!(
        name.has_interior_nul(),
        "the terminated pointer would be truncated here, and the type reports it"
    );
}
