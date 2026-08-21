// Copyright (c) 2026 Mike Grier
use windows_core::{PCWSTR, Param, ParamValue};

use crate::Wtf16String;

/// Stand-in for a generated `windows` binding: it accepts the same
/// `impl Param<PCWSTR>` bound those functions use and recovers the raw pointer
/// the callee would receive. If `&Wtf16String` did not satisfy the bound, this
/// would not compile -- which is half of what these tests assert.
fn as_callee_ptr<P: Param<PCWSTR>>(param: P) -> *const u16 {
    // SAFETY: this is exactly what a generated binding does with a `Param` -- take
    // the `ParamValue` and read its `abi`. The returned pointer is only compared
    // and read below, while the owning string is still borrowed and alive.
    let value = unsafe { param.param() };
    match value {
        ParamValue::Owned(pcwstr) => pcwstr.0,
        ParamValue::Borrowed(pcwstr) => pcwstr.0,
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn lstrlenW(lpstring: *const u16) -> i32;
}

#[test]
fn param_hands_over_the_terminated_pointer_itself() {
    // The whole point of the seam: the callee receives *our* buffer pointer, with
    // no conversion, no allocation and no copy.
    let owned = Wtf16String::from("zero-conversion");
    assert_eq!(
        as_callee_ptr(&owned),
        owned.as_terminated_ptr(),
        "the callee must receive the string's own terminated pointer"
    );
}

#[test]
fn param_pointer_is_nul_terminated_and_reads_back() {
    for s in [
        "",
        "a",
        "C:\\Windows\\System32",
        "caf\u{E9} \u{65E5}\u{672C}",
    ] {
        let owned = Wtf16String::from(s);
        let ptr = as_callee_ptr(&owned);

        // Walk it as a C string, exactly as a wide Win32 callee would.
        let mut read = Vec::new();
        let mut i = 0isize;
        loop {
            // SAFETY: the buffer is `[content.., NUL]` and none of these cases has
            // an interior NUL, so the scan stops at the terminator, in bounds.
            let unit = unsafe { *ptr.offset(i) };
            if unit == 0 {
                break;
            }
            read.push(unit);
            i += 1;
        }
        assert_eq!(
            read,
            owned.as_units(),
            "{s:?} round-trips through the pointer"
        );
    }
}

#[test]
fn param_survives_a_real_wide_win32_call() {
    // End-to-end: a genuine `*W` entry point, fed straight from the `Param`
    // conversion, agrees with our content length.
    #[cfg(windows)]
    for s in ["", "a", "a longer path-like value"] {
        let owned = Wtf16String::from(s);
        let ptr = as_callee_ptr(&owned);
        // SAFETY: `ptr` is our NUL-terminated buffer and none of these cases has
        // an interior NUL, so `lstrlenW` stops at the terminator.
        let len = unsafe { lstrlenW(ptr) };
        assert_eq!(len as usize, owned.len(), "lstrlenW disagrees for {s:?}");
    }
}

#[test]
fn param_truncates_at_an_interior_nul_like_any_c_string() {
    // Documented caveat (D-7), pinned so it cannot change silently: the conversion
    // is infallible, so a value with an interior NUL is seen truncated by the
    // callee -- the same behaviour `&HSTRING`'s own impl has.
    let owned = Wtf16String::from("visible\u{0}hidden");
    assert!(owned.has_interior_nul());
    let ptr = as_callee_ptr(&owned);

    #[cfg(windows)]
    {
        // SAFETY: the buffer is terminated; the callee stops at the interior NUL.
        let len = unsafe { lstrlenW(ptr) };
        assert_eq!(len as usize, "visible".len(), "the callee stops at the NUL");
        assert!(
            (len as usize) < owned.len(),
            "the truncation is what makes has_interior_nul worth checking"
        );
    }
    #[cfg(not(windows))]
    let _ = ptr;
}
