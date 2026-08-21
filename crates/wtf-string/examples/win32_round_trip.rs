// Copyright (c) 2026 Mike Grier
//! A wide (`*W`) Win32 round trip with no string conversion in either
//! direction.
//!
//! Run it with:
//!
//! ```text
//! cargo run --example win32_round_trip
//! ```
//!
//! It exercises all three halves of the FFI surface against real kernel32
//! entry points:
//!
//! * **terminated input** -- `as_terminated_ptr()` into `GetFullPathNameW`'s
//!   `LPCWSTR` parameter;
//! * **buffer-fill output** -- `with_capacity` / `as_mut_ptr` /
//!   `set_len_from_ffi` to receive that call's result;
//! * **counted input** -- `as_ptr()` + `len()` into `CompareStringOrdinal`,
//!   which takes explicit lengths and so works on borrowed slices that carry
//!   no terminator at all.
//!
//! The entry points are declared inline rather than pulled from `windows-sys`,
//! purely to keep the crate dependency-free.

#[cfg(windows)]
fn main() {
    windows::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("win32_round_trip is Windows-only; nothing to demonstrate here.");
}

#[cfg(windows)]
mod windows {
    use wtf_string::{Wtf16Str, Wtf16String};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// Expands `lpfilename` to a full path. With `nbufferlength == 0` it
        /// reports the required size *including* the terminator; on success it
        /// returns the count written *excluding* it. Zero means failure.
        fn GetFullPathNameW(
            lpfilename: *const u16,
            nbufferlength: u32,
            lpbuffer: *mut u16,
            lpfilepart: *mut *mut u16,
        ) -> u32;

        /// Ordinal comparison of two *counted* wide strings: neither pointer
        /// needs to be NUL-terminated.
        fn CompareStringOrdinal(
            lpstring1: *const u16,
            cchcount1: i32,
            lpstring2: *const u16,
            cchcount2: i32,
            bignorecase: i32,
        ) -> i32;
    }

    /// `CompareStringOrdinal` returns these rather than the usual -1/0/1.
    const CSTR_LESS_THAN: i32 = 1;
    const CSTR_EQUAL: i32 = 2;
    const CSTR_GREATER_THAN: i32 = 3;

    pub fn run() {
        let input = Wtf16String::from(r"C:\Windows\System32\..\Temp");
        println!("input : {input}");

        match full_path(&input) {
            Some(expanded) => println!("expanded: {expanded}"),
            None => println!("expanded: <GetFullPathNameW failed>"),
        }

        compare(&Wtf16String::from("alpha"), &Wtf16String::from("beta"));
        compare(&Wtf16String::from("beta"), &Wtf16String::from("alpha"));
        compare(&Wtf16String::from("same"), &Wtf16String::from("same"));

        // The counted pair works on a borrowed slice, which has no terminator
        // of its own -- the length is what makes it well-defined.
        let units: Vec<u16> = "borrowed".encode_utf16().collect();
        let borrowed = Wtf16Str::from_units(&units);
        println!(
            "borrowed slice of {} units compares equal to itself: {}",
            borrowed.len(),
            ordinal(borrowed, borrowed) == CSTR_EQUAL
        );
    }

    /// Terminated input, then buffer-fill output -- both without converting.
    fn full_path(input: &Wtf16String) -> Option<Wtf16String> {
        // Pass 1: ask for the size. The terminator is already in the buffer, so
        // handing over an `LPCWSTR` costs nothing.
        // SAFETY: `as_terminated_ptr` is NUL-terminated and valid while
        // `input` is borrowed; a zero length asks for the required size only.
        let needed = unsafe {
            GetFullPathNameW(
                input.as_terminated_ptr(),
                0,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        if needed == 0 {
            return None;
        }

        // `needed` counts the terminator; our capacity is a *content* length,
        // and `with_capacity` reserves the terminator slot itself.
        let mut out = Wtf16String::with_capacity(needed as usize - 1);

        // Pass 2: let the API write straight into our buffer.
        // SAFETY: the buffer has room for `needed` units (content + the
        // reserved terminator slot), which is exactly what pass 1 asked for.
        let written = unsafe {
            GetFullPathNameW(
                input.as_terminated_ptr(),
                needed,
                out.as_mut_ptr(),
                core::ptr::null_mut(),
            )
        };
        if written == 0 || written >= needed {
            // Failed, or raced a directory change and now wants more room.
            // `out`'s invariant is still broken here, so republish an empty
            // string before dropping it (see `as_mut_ptr`'s contract).
            // SAFETY: publishing zero content units is always in bounds.
            unsafe { out.set_len_from_ffi(0) };
            return None;
        }

        // `written` excludes the terminator, which is precisely the content
        // length `set_len_from_ffi` wants -- no guessing about conventions.
        // SAFETY: the API initialized `written` units and `written < needed`,
        // so the appended terminator still fits.
        unsafe { out.set_len_from_ffi(written as usize) };
        Some(out)
    }

    /// Counted input: pointer + length, no terminator required.
    fn ordinal(a: &Wtf16Str, b: &Wtf16Str) -> i32 {
        // SAFETY: each pointer is valid for exactly its own `len()` units while
        // borrowed, which is the contract `CompareStringOrdinal` expects.
        unsafe {
            CompareStringOrdinal(
                a.as_ptr(),
                a.len() as i32,
                b.as_ptr(),
                b.len() as i32,
                0, // case-sensitive
            )
        }
    }

    fn compare(a: &Wtf16String, b: &Wtf16String) {
        let verdict = match ordinal(a, b) {
            CSTR_LESS_THAN => "<",
            CSTR_EQUAL => "==",
            CSTR_GREATER_THAN => ">",
            _ => "?(failed)",
        };
        println!("compare: {a} {verdict} {b}");
    }
}
