// Copyright (c) 2026 Mike Grier
//! Tests for the fixed native staging buffer.

use super::*;
use crate::request::{DEFAULT_BUFFER_CAPACITY, MINIMUM_BUFFER_CAPACITY};

#[test]
fn a_buffer_reports_the_capacity_it_was_asked_for() {
    let buffer = NativeBuffer::try_new(MINIMUM_BUFFER_CAPACITY).expect("a small allocation");
    assert_eq!(buffer.capacity(), MINIMUM_BUFFER_CAPACITY as u32);
    assert_eq!(buffer.as_bytes().len(), MINIMUM_BUFFER_CAPACITY);
}

#[test]
fn the_default_capacity_allocates() {
    let buffer = NativeBuffer::try_new(DEFAULT_BUFFER_CAPACITY).expect("64 KiB");
    assert_eq!(buffer.capacity(), DEFAULT_BUFFER_CAPACITY as u32);
}

#[test]
fn the_base_address_is_eight_byte_aligned() {
    // The directory-information classes reject a misaligned batch outright, and
    // reading the record's i64 fields from one would be undefined behaviour.
    for capacity in [
        MINIMUM_BUFFER_CAPACITY,
        MINIMUM_BUFFER_CAPACITY + RECORD_ALIGNMENT,
        DEFAULT_BUFFER_CAPACITY,
    ] {
        let mut buffer = NativeBuffer::try_new(capacity).expect("allocation");
        let address = buffer.as_mut_ptr() as usize;
        assert_eq!(address % RECORD_ALIGNMENT, 0, "for {capacity} bytes");
    }
}

#[test]
fn a_fresh_buffer_is_zeroed() {
    let buffer = NativeBuffer::try_new(MINIMUM_BUFFER_CAPACITY).expect("allocation");
    assert!(buffer.as_bytes().iter().all(|byte| *byte == 0));
}

#[test]
fn an_impossible_allocation_is_reported_rather_than_aborting() {
    // The ordinary growable-vector path aborts the process here; a caller that
    // asked for too much deserves an answer instead.
    assert!(NativeBuffer::try_new(usize::MAX & !(RECORD_ALIGNMENT - 1)).is_none());
}

#[test]
fn debug_output_names_the_capacity() {
    let buffer = NativeBuffer::try_new(MINIMUM_BUFFER_CAPACITY).expect("allocation");
    let rendered = format!("{buffer:?}");
    assert!(
        rendered.contains(&MINIMUM_BUFFER_CAPACITY.to_string()),
        "{rendered}"
    );
}
