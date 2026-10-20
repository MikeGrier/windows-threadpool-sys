// Copyright (c) 2026 Mike Grier
//! Tests for `wide_name` and `from_wide_until_nul`.

use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::os::windows::ffi::OsStringExt;

use super::{from_wide_until_nul, wide_name};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn a_name_is_its_units_and_a_terminator() {
    for text in [
        "",
        "a",
        "ab",
        r"\\.\pipe\x",
        r"\\.\pipe\a b",
        "caf\u{e9}",
        "\u{1f600}",
    ] {
        let mut expected = wide(text);
        expected.push(0);
        assert_eq!(
            wide_name(OsStr::new(text)).expect("a name"),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn a_name_with_an_interior_nul_is_refused_wherever_the_nul_is() {
    for text in ["\0", "\0a", "a\0", "a\0b", "a\0\0", "\\\\.\\pipe\\a\0b"] {
        let error = wide_name(OsStr::new(text)).expect_err("a NUL");
        assert_eq!(error.kind(), ErrorKind::InvalidInput, "{text:?}");
        assert_eq!(error.raw_os_error(), None);
    }
}

#[test]
fn a_name_that_is_not_valid_utf16_is_passed_as_it_is() {
    let units = [0x5c, 0xd800, 0x61];
    let name = OsString::from_wide(&units);
    assert_eq!(wide_name(&name).expect("a name"), [0x5c, 0xd800, 0x61, 0]);
}

#[test]
fn a_string_ends_at_its_first_nul() {
    for (units, expected) in [
        (&[][..], ""),
        (&[0][..], ""),
        (&[0x61][..], "a"),
        (&[0x61, 0][..], "a"),
        (&[0x61, 0x62, 0, 0x63][..], "ab"),
        (&[0, 0x61][..], ""),
        (&[0x61, 0, 0, 0][..], "a"),
        (&[0x61, 0x62, 0x63][..], "abc"),
        (&[0xe9, 0x41, 0][..], "\u{e9}A"),
    ] {
        assert_eq!(
            from_wide_until_nul(units),
            OsString::from(expected),
            "{units:?}"
        );
    }
}

#[test]
fn a_buffer_with_no_nul_is_taken_whole() {
    let units = [0x61u16; 300];
    assert_eq!(from_wide_until_nul(&units).len(), 300);
}

#[test]
fn a_unit_that_is_not_valid_utf16_is_kept() {
    let units = [0x61, 0xd800, 0x62, 0];
    assert_eq!(
        from_wide_until_nul(&units),
        OsString::from_wide(&[0x61, 0xd800, 0x62])
    );
}

#[test]
fn a_string_read_from_a_buffer_and_a_name_made_from_it_round_trip() {
    for text in ["", "x", r"\\.\pipe\round-trip", "caf\u{e9}"] {
        let name = wide_name(OsStr::new(text)).expect("a name");
        assert_eq!(from_wide_until_nul(&name), OsString::from(text));
    }
}
