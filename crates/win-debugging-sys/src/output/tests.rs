// Copyright (c) 2026 Mike Grier
//! What is built from a string before the call, apart from the call (WD-D-2), and that each function
//! reaches it. What a debugger receives is `tests/debugger.rs`.

use super::{NUL, OutputDebugStringError, output_debug_str, output_debug_string, terminate};

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn terminated(mut input: Vec<u16>) -> Result<Vec<u16>, OutputDebugStringError> {
    terminate(&mut input)?;
    Ok(input)
}

#[test]
fn an_empty_string_is_just_the_terminator() {
    assert_eq!(terminated(Vec::new()), Ok(vec![NUL]));
}

#[test]
fn an_ordinary_string_keeps_its_units_and_gains_the_terminator() {
    let mut expected = units("hello");
    expected.push(NUL);
    assert_eq!(terminated(units("hello")), Ok(expected));
}

#[test]
fn a_one_unit_string_is_two_units() {
    assert_eq!(terminated(vec![0x61]), Ok(vec![0x61, NUL]));
}

#[test]
fn spaces_punctuation_and_line_breaks_are_not_terminators() {
    let text = "a b,c;d\r\ne\tf\u{7f}";
    let mut expected = units(text);
    expected.push(NUL);
    assert_eq!(terminated(units(text)), Ok(expected));
}

#[test]
fn a_string_outside_the_basic_plane_is_its_surrogate_pairs() {
    let mut expected = units("a\u{1f600}b");
    assert_eq!(expected.len(), 4);
    expected.push(NUL);
    assert_eq!(terminated(units("a\u{1f600}b")), Ok(expected));
}

#[test]
fn an_unpaired_surrogate_is_a_unit_like_any_other() {
    assert_eq!(
        terminated(vec![0xd800, 0x61, 0xdc00]),
        Ok(vec![0xd800, 0x61, 0xdc00, NUL])
    );
}

#[test]
fn a_unit_with_only_the_low_byte_clear_is_not_a_terminator() {
    assert_eq!(terminated(vec![0x0100]), Ok(vec![0x0100, NUL]));
}

#[test]
fn a_unit_with_only_the_high_byte_clear_is_not_a_terminator() {
    assert_eq!(terminated(vec![0x0001]), Ok(vec![0x0001, NUL]));
}

#[test]
fn a_nul_alone_is_refused_at_unit_zero() {
    assert_eq!(
        terminated(vec![NUL]),
        Err(OutputDebugStringError::EmbeddedNul { index: 0 })
    );
}

#[test]
fn a_nul_in_the_middle_is_refused_at_its_unit() {
    assert_eq!(
        terminated(vec![0x61, 0x62, NUL, 0x63]),
        Err(OutputDebugStringError::EmbeddedNul { index: 2 })
    );
}

#[test]
fn a_nul_at_the_end_is_refused_though_it_would_have_been_a_terminator() {
    assert_eq!(
        terminated(vec![0x61, NUL]),
        Err(OutputDebugStringError::EmbeddedNul { index: 1 })
    );
}

#[test]
fn the_first_of_several_nuls_is_the_one_named() {
    assert_eq!(
        terminated(vec![0x61, NUL, 0x62, NUL, NUL]),
        Err(OutputDebugStringError::EmbeddedNul { index: 1 })
    );
}

#[test]
fn a_refused_string_is_left_as_it_was_given() {
    let mut input = vec![0x61, NUL, 0x62];
    assert!(terminate(&mut input).is_err());
    assert_eq!(input, [0x61, NUL, 0x62]);
}

#[test]
fn a_long_string_is_every_unit_it_was_given_and_one_more() {
    let input: Vec<u16> = (0..1_048_576_u32)
        .map(|n| (n % 0xfffe) as u16 + 1)
        .collect();
    let out = terminated(input.clone()).expect("no unit is a NUL");
    assert_eq!(out.len(), input.len() + 1);
    assert_eq!(&out[..input.len()], &input[..]);
    assert_eq!(out[input.len()], NUL);
}

#[test]
fn a_nul_at_the_very_end_of_a_long_string_is_found() {
    let mut input = vec![0x61_u16; 1_048_576];
    input.push(NUL);
    assert_eq!(
        terminated(input),
        Err(OutputDebugStringError::EmbeddedNul { index: 1_048_576 })
    );
}

#[test]
fn a_str_with_a_nul_is_refused_at_the_unit_not_the_byte() {
    // The first character is two units, and three bytes in UTF-8, before the NUL.
    assert_eq!(
        output_debug_str("\u{1f600}\u{e9}\0z"),
        Err(OutputDebugStringError::EmbeddedNul { index: 3 })
    );
}

#[test]
fn a_slice_with_a_nul_is_refused_and_the_call_is_not_made() {
    assert_eq!(
        output_debug_string(&[0x61, NUL]),
        Err(OutputDebugStringError::EmbeddedNul { index: 1 })
    );
}

#[test]
fn the_empty_string_and_ordinary_text_are_sent_with_no_failure() {
    // With no debugger the call "does nothing", so this shows the call is made and returns, and that
    // neither function refuses what it should not.
    assert_eq!(output_debug_string(&[]), Ok(()));
    assert_eq!(output_debug_str(""), Ok(()));
    assert_eq!(output_debug_str("win-debugging-sys: a unit test"), Ok(()));
    assert_eq!(output_debug_string(&units("\u{3042}\u{1f600}")), Ok(()));
    assert_eq!(output_debug_string(&[0xd800]), Ok(()));
}

#[test]
fn a_very_long_string_is_sent_with_no_failure() {
    assert_eq!(output_debug_str(&"x".repeat(300_000)), Ok(()));
}

#[test]
fn the_refusal_reads_as_a_sentence_naming_the_unit() {
    let error = OutputDebugStringError::EmbeddedNul { index: 7 };
    assert_eq!(
        error.to_string(),
        "the string has a NUL at unit 7, and OutputDebugStringW's string ends at its first NUL"
    );
}

#[test]
fn the_refusal_is_an_error_with_no_source() {
    let error = OutputDebugStringError::EmbeddedNul { index: 0 };
    assert!(std::error::Error::source(&error).is_none());
}
