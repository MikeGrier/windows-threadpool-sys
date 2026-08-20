// Copyright (c) 2026 Mike Grier
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use crate::{Wtf16Str, Wtf16String};

#[test]
fn os_str_roundtrip_wellformed() {
    for s in ["", "a", "café", "日本語", "😀", "a\u{0}b"] {
        let os = OsString::from(s);
        let wtf = Wtf16String::from_os_str(&os);
        assert_eq!(wtf.to_os_string(), os, "{s:?}");
    }
}

#[test]
fn os_str_roundtrip_unpaired_surrogate_is_lossless() {
    // An OsString carrying an unpaired surrogate (legal on Windows) must survive.
    let wide = [0x61u16, 0xD800, 0x62];
    let os = OsString::from_wide(&wide);
    let wtf = Wtf16String::from_os_str(&os);
    assert_eq!(wtf.as_units(), &wide);
    assert_eq!(wtf.to_os_string(), os);
}

#[test]
fn from_os_str_matches_encode_wide() {
    let os = OsString::from("mix é 日 😀");
    let wtf = Wtf16String::from_os_str(&os);
    let expected: Vec<u16> = os.encode_wide().collect();
    assert_eq!(wtf.as_units(), expected.as_slice());
    // Our own encode_wide reproduces the same units.
    let ours: Vec<u16> = wtf.encode_wide().collect();
    assert_eq!(ours, expected);
}

#[test]
fn from_wide_matches_from_units() {
    let units = [0x48u16, 0x69, 0xD800];
    assert_eq!(
        Wtf16String::from_wide(&units).as_units(),
        Wtf16String::from_units(&units).as_units()
    );
}

#[test]
fn from_and_into_conversions_round_trip() {
    let os = OsString::from("data");
    let wtf = Wtf16String::from(os.as_os_str());
    // Owned `OsString` -> `Wtf16String` (`os.into()`), symmetric with the owned
    // reverse below.
    let from_owned_os: Wtf16String = os.clone().into();
    assert_eq!(from_owned_os.as_units(), wtf.as_units());
    let from_ref: OsString = (&wtf).into();
    assert_eq!(from_ref, os);
    let borrowed: &Wtf16Str = &wtf;
    let from_borrowed: OsString = borrowed.into();
    assert_eq!(from_borrowed, os);
    let from_owned: OsString = wtf.into();
    assert_eq!(from_owned, os);
}
