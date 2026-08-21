// Copyright (c) 2026 Mike Grier
//! Windows integration: lossless `OsStr` <-> WTF-16 round-trips (including
//! unpaired surrogates and interior NULs) and a real wide (`*W`) Win32 call fed
//! straight from our pointer with no conversion.
// The interop under test is gated on `std` as well as Windows (D-11), so this
// whole target compiles away in an `alloc`-only build.
#![cfg(all(windows, feature = "std"))]

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use wtf_string::Wtf16String;

// A real kernel32 wide entry point: it returns the length of a NUL-terminated
// `LPCWSTR`, exercising the zero-conversion pointer hand-off directly.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn lstrlenW(lpstring: *const u16) -> i32;
}

#[test]
fn os_str_roundtrip_is_lossless_including_unpaired_surrogates() {
    // Well-formed and ill-formed (unpaired-surrogate) OsStrings alike must survive
    // OsStr -> Wtf16String -> OsString unchanged.
    let cases: Vec<OsString> = vec![
        OsString::from(""),
        OsString::from("simple"),
        OsString::from("café 日本語 😀"),
        OsString::from("interior\u{0}nul"),
        OsString::from_wide(&[0x61, 0xD800, 0x62]), // lone high in the middle
        OsString::from_wide(&[0xDC00]),             // lone low
        OsString::from_wide(&[0xD83D, 0xDE00, 0xDBFF]), // valid pair then lone high
        OsString::from_wide(&[0x41, 0x00, 0xD800, 0x00, 0x42]), // NULs + surrogate
    ];
    for os in cases {
        let wtf = Wtf16String::from_os_str(&os);
        // Units equal the OsStr's own wide encoding (no information change).
        let expected: Vec<u16> = os.encode_wide().collect();
        assert_eq!(wtf.as_units(), expected.as_slice(), "units for {os:?}");
        // Round-trip back to OsString is byte-identical.
        assert_eq!(wtf.to_os_string(), os, "roundtrip {os:?}");
    }
}

#[test]
fn bulk_roundtrip_is_lossless() {
    // A larger, deterministic sweep: hundreds of wide sequences mixing valid
    // scalars, unpaired surrogates, and interior NULs must all survive.
    let alphabet_a = [
        0x41u16, 0x00, 0xD800, 0xDC00, 0x00E9, 0x65E5, 0xDFFF, 0xD83D,
    ];
    let alphabet_b = [
        0x42u16, 0x00, 0xDC00, 0xD800, 0xDE00, 0xFFFD, 0x07FF, 0xDBFF,
    ];
    let alphabet_c = [0x43u16, 0xD800, 0x00, 0xDFFF, 0x001F, 0xD83D];
    let mut count = 0usize;
    for &a in &alphabet_a {
        for &b in &alphabet_b {
            for &c in &alphabet_c {
                let wide = [a, b, c, a, b];
                let os = OsString::from_wide(&wide);
                let wtf = Wtf16String::from_os_str(&os);
                assert_eq!(wtf.to_os_string(), os);
                let expected: Vec<u16> = os.encode_wide().collect();
                assert_eq!(wtf.as_units(), expected.as_slice());
                count += 1;
            }
        }
    }
    assert!(count >= 256, "expected hundreds of cases, got {count}");
}

#[test]
fn real_wide_win32_call_fed_from_our_pointer() {
    // A deterministic wide call: lstrlenW counts code units up to the terminator,
    // fed straight from our terminated pointer with no conversion.
    for s in ["", "a", "hello world", "café-日本語-😀"] {
        let wtf = Wtf16String::from(OsString::from(s).as_os_str());
        // The wide string is a valid C string only with no interior NUL; none here.
        assert!(!wtf.has_interior_nul(), "{s:?}");
        // SAFETY: `as_terminated_ptr` is a valid NUL-terminated `LPCWSTR` (no
        // interior NUL), exactly what `lstrlenW` expects; it reads only to the NUL.
        let len = unsafe { lstrlenW(wtf.as_terminated_ptr()) };
        assert_eq!(len as usize, wtf.len(), "lstrlenW length for {s:?}");
    }
}

#[test]
fn counted_pointer_matches_a_wide_apis_view() {
    // The counted (`as_ptr` + `len`) hand-off reproduces exactly what a `*W` API
    // would read, with no re-encoding at the call boundary.
    let os = OsString::from("mixed é 日 😀 text");
    let wtf = Wtf16String::from_os_str(&os);
    // SAFETY: `as_ptr` is valid for `len()` reads of the content.
    let via_ptr = unsafe { std::slice::from_raw_parts(wtf.as_ptr(), wtf.len()) };
    let via_os: Vec<u16> = os.encode_wide().collect();
    assert_eq!(via_ptr, via_os.as_slice());
}
