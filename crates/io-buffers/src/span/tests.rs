// Copyright (c) 2026 Mike Grier
//! Unit tests for `RegisteredSpan`.

use super::RegisteredSpan;

fn span(buffer_index: u32, offset: u32, len: u32) -> RegisteredSpan {
    RegisteredSpan {
        buffer_index,
        offset,
        len,
    }
}

#[test]
fn a_span_holds_the_three_numbers_it_was_given() {
    let span = span(3, 4096, 512);
    assert_eq!(span.buffer_index, 3);
    assert_eq!(span.offset, 4096);
    assert_eq!(span.len, 512);
}

#[test]
fn a_span_is_copy_and_the_copy_is_independent() {
    let original = span(1, 2, 3);
    let mut copy = original;
    copy.offset = 99;
    assert_eq!(original.offset, 2, "changing a copy changed the original");
    assert_eq!(copy.offset, 99);
}

#[test]
fn a_span_may_be_zero_length_and_at_the_ends_of_each_range() {
    // A zero-length operation is legal, and the fields are full-width `u32`s: nothing here narrows
    // them, so the extremes are representable and kept apart.
    let empty = span(0, 0, 0);
    assert_eq!((empty.buffer_index, empty.offset, empty.len), (0, 0, 0));
    let widest = span(u32::MAX, u32::MAX, u32::MAX);
    assert_eq!(widest.buffer_index, u32::MAX);
    assert_eq!(widest.offset, u32::MAX);
    assert_eq!(widest.len, u32::MAX);
}

#[test]
fn a_spans_debug_form_names_its_fields() {
    let text = format!("{:?}", span(7, 8, 9));
    for expected in ["RegisteredSpan", "buffer_index: 7", "offset: 8", "len: 9"] {
        assert!(text.contains(expected), "{text} lacks {expected}");
    }
}

#[test]
fn a_span_is_send_and_sync() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<RegisteredSpan>();
}
