// Copyright (c) 2026 Mike Grier
//! Tests for the native record parser, against synthetic batches.
//!
//! Every malformed variant is exercised by corrupting exactly the one field
//! that variant is named for, leaving everything else well-formed -- so a
//! failing assertion here points at one specific validation, not "something
//! about this batch was wrong".

use super::*;

/// A record's fields, in a form a test can build and selectively corrupt.
///
/// Defaults describe an ordinary, well-formed record so a test only has to
/// name what it is deliberately breaking.
struct RecordSpec {
    next_entry_offset: u32,
    name: Vec<u16>,
    name_length_override: Option<u32>,
    attributes: u32,
    ea_size: u32,
    reparse_tag: u32,
    creation_time: i64,
    last_access_time: i64,
    last_write_time: i64,
    change_time: i64,
    end_of_file: i64,
    allocation_size: i64,
    file_id: [u8; 16],
}

impl RecordSpec {
    fn named(name: &str) -> Self {
        Self {
            next_entry_offset: 0,
            name: name.encode_utf16().collect(),
            name_length_override: None,
            attributes: 0,
            ea_size: 0,
            reparse_tag: 0,
            creation_time: 1,
            last_access_time: 2,
            last_write_time: 3,
            change_time: 4,
            end_of_file: 100,
            allocation_size: 4096,
            file_id: [7; 16],
        }
    }

    #[must_use]
    fn with_next_entry_offset(mut self, value: u32) -> Self {
        self.next_entry_offset = value;
        self
    }

    #[must_use]
    fn with_name_length_override(mut self, value: u32) -> Self {
        self.name_length_override = Some(value);
        self
    }

    #[must_use]
    fn with_end_of_file(mut self, value: i64) -> Self {
        self.end_of_file = value;
        self
    }

    #[must_use]
    fn with_allocation_size(mut self, value: i64) -> Self {
        self.allocation_size = value;
        self
    }

    /// Serialise to exactly `FIXED_FIELDS_LEN + name.len() * 2` bytes, laid out
    /// field-for-field the way [`field`] declares them.
    fn to_bytes(&self) -> Vec<u8> {
        let name_bytes: Vec<u8> = self
            .name
            .iter()
            .flat_map(|unit| unit.to_ne_bytes())
            .collect();
        let name_length = self.name_length_override.unwrap_or(name_bytes.len() as u32);

        let mut out = Vec::with_capacity(FIXED_FIELDS_LEN + name_bytes.len());
        out.extend_from_slice(&self.next_entry_offset.to_ne_bytes());
        out.extend_from_slice(&0u32.to_ne_bytes()); // FileIndex: never read.
        out.extend_from_slice(&self.creation_time.to_ne_bytes());
        out.extend_from_slice(&self.last_access_time.to_ne_bytes());
        out.extend_from_slice(&self.last_write_time.to_ne_bytes());
        out.extend_from_slice(&self.change_time.to_ne_bytes());
        out.extend_from_slice(&self.end_of_file.to_ne_bytes());
        out.extend_from_slice(&self.allocation_size.to_ne_bytes());
        out.extend_from_slice(&self.attributes.to_ne_bytes());
        out.extend_from_slice(&name_length.to_ne_bytes());
        out.extend_from_slice(&self.ea_size.to_ne_bytes());
        out.extend_from_slice(&self.reparse_tag.to_ne_bytes());
        out.extend_from_slice(&self.file_id);
        out.extend_from_slice(&name_bytes);
        assert_eq!(out.len(), FIXED_FIELDS_LEN + name_bytes.len());
        out
    }
}

fn name_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn a_well_formed_record_parses() {
    let batch = RecordSpec::named("a.txt").to_bytes();
    let (record, next) = parse_record(&batch, 0).expect("well-formed");
    assert_eq!(record.name.as_units(), name_units("a.txt").as_slice());
    assert_eq!(next, None);
    assert!(!record.is_dot_or_dotdot());
}

#[test]
fn every_field_survives_into_entry_fields() {
    let spec = RecordSpec::named("report.csv");
    let batch = spec.to_bytes();
    let (record, _) = parse_record(&batch, 0).expect("well-formed");
    let fields = record.into_fields(Some(0xABCD));

    assert_eq!(fields.name.as_units(), name_units("report.csv").as_slice());
    assert_eq!(fields.logical_size, 100);
    assert_eq!(fields.allocation_size, 4096);
    assert_eq!(fields.creation_time, WindowsFileTimestamp::from_ticks(1));
    assert_eq!(fields.last_access_time, WindowsFileTimestamp::from_ticks(2));
    assert_eq!(fields.last_write_time, WindowsFileTimestamp::from_ticks(3));
    assert_eq!(fields.change_time, WindowsFileTimestamp::from_ticks(4));
    assert_eq!(fields.identity.id_bytes(), [7; 16]);
    assert_eq!(fields.identity.volume_serial(), Some(0xABCD));
}

#[test]
fn a_volume_serial_of_none_survives_unqualified() {
    let batch = RecordSpec::named("a.txt").to_bytes();
    let (record, _) = parse_record(&batch, 0).expect("well-formed");
    let fields = record.into_fields(None);
    assert_eq!(fields.identity.volume_serial(), None);
}

#[test]
fn the_current_directory_is_recognised() {
    let batch = RecordSpec::named(".").to_bytes();
    let (record, _) = parse_record(&batch, 0).expect("well-formed");
    assert!(record.is_dot_or_dotdot());
}

#[test]
fn the_parent_directory_is_recognised() {
    let batch = RecordSpec::named("..").to_bytes();
    let (record, _) = parse_record(&batch, 0).expect("well-formed");
    assert!(record.is_dot_or_dotdot());
}

#[test]
fn a_name_that_merely_starts_with_a_dot_is_not_the_directory_itself() {
    let batch = RecordSpec::named("...").to_bytes();
    let (record, _) = parse_record(&batch, 0).expect("well-formed");
    assert!(!record.is_dot_or_dotdot());
}

#[test]
fn a_two_record_batch_chains_by_its_next_entry_offset() {
    let mut first = RecordSpec::named("a.txt").to_bytes();
    // Pad to an 8-byte boundary the way the API keeps every record on one.
    while !first.len().is_multiple_of(RECORD_ALIGNMENT) {
        first.push(0);
    }
    let first_len = first.len();
    first[0..4].copy_from_slice(&(first_len as u32).to_ne_bytes());

    let second = RecordSpec::named("b.txt").to_bytes();
    let mut batch = first;
    batch.extend_from_slice(&second);

    let (record, next) = parse_record(&batch, 0).expect("well-formed");
    assert_eq!(record.name.as_units(), name_units("a.txt").as_slice());
    let next = next.expect("a second record follows");
    assert_eq!(next, first_len);

    let (record, next) = parse_record(&batch, next).expect("well-formed");
    assert_eq!(record.name.as_units(), name_units("b.txt").as_slice());
    assert_eq!(next, None);
}

#[test]
fn a_misaligned_record_start_is_rejected() {
    let batch = RecordSpec::named("a.txt").to_bytes();
    let error = parse_record(&batch, 1).expect_err("1 is not a multiple of 8");
    assert_eq!(error, MalformedRecord::Alignment);
}

#[test]
fn a_batch_shorter_than_the_fixed_fields_is_rejected() {
    let mut batch = RecordSpec::named("a.txt").to_bytes();
    batch.truncate(FIXED_FIELDS_LEN - 1);
    let error = parse_record(&batch, 0).expect_err("too short to hold the fixed fields");
    assert_eq!(error, MalformedRecord::TruncatedFixedFields);
}

#[test]
fn an_offset_equal_to_the_batch_length_is_also_too_short() {
    let mut batch = RecordSpec::named("a.txt").to_bytes();
    while !batch.len().is_multiple_of(RECORD_ALIGNMENT) {
        batch.push(0);
    }
    let error =
        parse_record(&batch, batch.len()).expect_err("no bytes at all remain at this offset");
    assert_eq!(error, MalformedRecord::TruncatedFixedFields);
}

#[test]
fn a_next_entry_offset_that_overlaps_this_records_own_extent_is_rejected() {
    // A record whose fixed fields plus name reach well past where its own
    // next-entry offset claims the next record starts.
    let batch = RecordSpec::named("a.txt")
        .with_next_entry_offset(RECORD_ALIGNMENT as u32)
        .to_bytes();
    let error = parse_record(&batch, 0).expect_err("the offset lands inside this record");
    assert_eq!(error, MalformedRecord::NextEntryOffset);
}

#[test]
fn a_next_entry_offset_past_the_end_of_the_batch_is_rejected() {
    let batch = RecordSpec::named("a.txt")
        .with_next_entry_offset(1_000_000)
        .to_bytes();
    let error = parse_record(&batch, 0).expect_err("nothing that large fits the batch");
    assert_eq!(error, MalformedRecord::NextEntryOffset);
}

#[test]
fn an_odd_name_length_is_rejected() {
    let batch = RecordSpec::named("a.txt")
        .with_name_length_override(3)
        .to_bytes();
    let error = parse_record(&batch, 0).expect_err("a UTF-16 name is never an odd byte count");
    assert_eq!(error, MalformedRecord::OddNameLength);
}

#[test]
fn a_name_length_past_the_end_of_the_batch_is_rejected() {
    let batch = RecordSpec::named("a.txt")
        .with_name_length_override(1_000_000)
        .to_bytes();
    let error = parse_record(&batch, 0).expect_err("no batch holds a name that long");
    assert_eq!(error, MalformedRecord::NameOutOfBounds);
}

#[test]
fn a_negative_logical_size_is_rejected() {
    let batch = RecordSpec::named("a.txt").with_end_of_file(-1).to_bytes();
    let error = parse_record(&batch, 0).expect_err("a size cannot be negative");
    assert_eq!(error, MalformedRecord::NegativeSize);
}

#[test]
fn a_negative_allocation_size_is_rejected() {
    let batch = RecordSpec::named("a.txt")
        .with_allocation_size(-1)
        .to_bytes();
    let error = parse_record(&batch, 0).expect_err("a size cannot be negative");
    assert_eq!(error, MalformedRecord::NegativeSize);
}

#[test]
fn a_zero_length_name_is_well_formed() {
    // Not a real directory record, but the parser has no reason to reject it:
    // a zero-length name is structurally valid, just never delivered because
    // it would fail every name-based predicate clause trivially.
    let batch = RecordSpec::named("").to_bytes();
    let (record, _) = parse_record(&batch, 0).expect("an empty name is not malformed");
    assert!(record.name.as_units().is_empty());
    assert!(!record.is_dot_or_dotdot());
}
