// Copyright (c) 2026 Mike Grier
//! Tests for the entry metadata surface.

use super::*;
use crate::testing::{
    ATTR_DIRECTORY, ATTR_HIDDEN, ATTR_READONLY, ATTR_REPARSE_POINT, EntryBuilder, named_directory,
    named_file,
};

#[test]
fn an_entry_without_the_directory_bit_is_a_file() {
    assert_eq!(named_file("readme.txt").entry_type(), EntryType::File);
}

#[test]
fn an_entry_with_the_directory_bit_is_a_directory() {
    assert_eq!(named_directory("logs").entry_type(), EntryType::Directory);
}

#[test]
fn raw_attributes_are_preserved_exactly() {
    let attributes = ATTR_READONLY | ATTR_HIDDEN | ATTR_DIRECTORY;
    let entry = EntryBuilder::file("x").attributes(attributes).build();
    assert_eq!(entry.attributes(), attributes);
}

#[test]
fn a_reparse_point_carries_its_tag() {
    let entry = EntryBuilder::file("link").reparse(0xA000_000C).build();
    assert!(entry.is_reparse_point());
    assert_eq!(entry.reparse_tag(), Some(0xA000_000C));
    assert_ne!(entry.attributes() & ATTR_REPARSE_POINT, 0);
}

#[test]
fn a_tag_without_the_attribute_bit_is_suppressed() {
    // The record's tag field is meaningless unless the attributes say the entry
    // is a reparse point, so it must not reach a caller.
    let entry = EntryBuilder::file("plain")
        .bare_reparse_tag(0xDEAD_BEEF)
        .build();
    assert!(!entry.is_reparse_point());
    assert_eq!(entry.reparse_tag(), None);
}

#[test]
fn sizes_and_extended_attributes_round_trip() {
    let entry = EntryBuilder::file("data.bin")
        .logical_size(4097)
        .allocation_size(8192)
        .extended_attribute_size(64)
        .build();
    assert_eq!(entry.logical_size(), 4097);
    assert_eq!(entry.allocation_size(), 8192);
    assert_eq!(entry.extended_attribute_size(), 64);
}

#[test]
fn all_four_times_are_kept_distinct() {
    let entry = EntryBuilder::file("x").times(1, 2, 3, 4).build();
    assert_eq!(entry.creation_time().ticks(), 1);
    assert_eq!(entry.last_access_time().ticks(), 2);
    assert_eq!(entry.last_write_time().ticks(), 3);
    assert_eq!(entry.change_time().ticks(), 4);
}

#[test]
fn a_name_keeps_its_native_code_units() {
    // An unpaired high surrogate is not expressible in a `str`, and must not be
    // replaced on the way through.
    let units = [0x0061, 0xD800, 0x0062];
    let entry = EntryBuilder::file_units(&units).build();
    assert_eq!(entry.name().as_units(), units);
    assert_eq!(entry.into_name().as_units(), units);
}

#[test]
fn an_identity_without_a_volume_is_not_qualified() {
    let identity = FileIdentity::new([7; 16], None);
    assert!(!identity.is_volume_qualified());
    assert_eq!(identity.volume_serial(), None);
    assert_eq!(identity.id_bytes(), [7; 16]);
}

#[test]
fn an_identity_with_a_volume_is_qualified() {
    let identity = FileIdentity::new([1; 16], Some(0x1234_5678_9ABC_DEF0));
    assert!(identity.is_volume_qualified());
    assert_eq!(identity.volume_serial(), Some(0x1234_5678_9ABC_DEF0));
}

#[test]
fn identity_equality_covers_the_whole_pair() {
    // The same id on two volumes names two different objects.
    let left = FileIdentity::new([1; 16], Some(1));
    let right = FileIdentity::new([1; 16], Some(2));
    assert_ne!(left, right);
}

#[test]
fn an_entry_carries_the_identity_it_was_built_with() {
    let identity = FileIdentity::new([9; 16], Some(42));
    let entry = EntryBuilder::file("x").identity(identity).build();
    assert_eq!(entry.identity(), identity);
}

#[test]
fn only_the_querying_identity_modes_cost_a_second_call() {
    assert!(!FileIdentityMode::Omit.queries_volume());
    assert!(FileIdentityMode::BestEffort.queries_volume());
    assert!(FileIdentityMode::Required.queries_volume());
}

#[test]
fn omitting_identity_is_the_default() {
    assert_eq!(FileIdentityMode::default(), FileIdentityMode::Omit);
}
