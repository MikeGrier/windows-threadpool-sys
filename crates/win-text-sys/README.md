# win-text-sys

Memory-safe Rust over the Win32 text-handling API, the area Microsoft Learn files `CompareStringOrdinal` under:
*Internationalization for Windows Applications > Globalization Services > National Language Support > National
Language Support Reference > National Language Support Functions*. Thin over Win32, adding no policy.

- `compare_string_ordinal` is `CompareStringOrdinal`: it compares two slices of UTF-16 code units, case-sensitively or
  not as a `Case` says, and returns the first's place against the second's as an `Ordering`. Each slice is passed
  with its length and never as the NUL-terminated string the page allows, so a NUL is a unit like any other and an
  unpaired surrogate is a unit. The comparison is the call's, which "tests for binary equality, not linguistic
  equality".
- A slice of more than `i32::MAX` units has no count to be passed with, because the page gives each count as an `int`,
  so it is refused as `CompareStringOrdinalError::TooLong` and the call is not made. That is this crate's refusal and
  not the call's. A failure the call reports is `CompareStringOrdinalError::Call`, with the code as a `Win32Error`.
  See [WT-D-2](DESIGN-NOTES.md#wt-d-2).

```rust
use std::cmp::Ordering;

use win_text_sys::{Case, compare_string_ordinal};

let name: Vec<u16> = "README.md".encode_utf16().collect();
let other: Vec<u16> = "readme.MD".encode_utf16().collect();

assert_eq!(compare_string_ordinal(&name, &other, Case::Insensitive)?, Ordering::Equal);
assert_eq!(compare_string_ordinal(&name, &other, Case::Sensitive)?, Ordering::Less);
# Ok::<(), win_text_sys::CompareStringOrdinalError>(())
```

The crate was made so that the sites of this workspace that each call `CompareStringOrdinal` with an `unsafe`
block of their own have one safe wrapper to share. See [WT-D-1](DESIGN-NOTES.md#wt-d-1) for the name and where the
function is filed.
