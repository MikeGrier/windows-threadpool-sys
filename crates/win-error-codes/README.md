# win-error-codes

The numbers Windows reports failure in, as types.

Windows reports a failure as a number, and which number space it is in is not in the number. A
`DWORD` from `GetLastError` is one; a `HRESULT` is another. Crates here each kept a bare `u32` or
wrote a `Win32Error` of their own, and the two that did disagreed about a code of 2^31 or more,
which exist: the virtdisk errors, such as `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`, are `WIN32_ERROR`s of
3225026581. This crate gives each number space its own type, and the conversion to and from
`std::io::Error` is written once and is exact for every code.

## What it gives

- **`Win32Error`**, a `u32`: the code unaltered, `is_success` and `into_result` (zero is
  `ERROR_SUCCESS`), and on Windows `to_io_error`, `from_io` and `last`.
- **`LStatus`**, an `i32`: the same number space in the signed `LONG` that registry calls return,
  converting to and from `Win32Error` by a bit cast that loses nothing, a code above 2^31 being a
  negative value.

```rust
use win_error_codes::{LStatus, Win32Error};

// A call that returns its code directly: zero is success.
assert_eq!(Win32Error::from_code(0).into_result(), Ok(()));

// A registry call returns the same thing as a signed LONG.
assert_eq!(LStatus::from_raw(2).into_result(), Err(Win32Error::from_code(2)));

// ERROR_VIRTDISK_NOT_VIRTUAL_DISK, a code above 2^31, keeps its value through io::Error.
# #[cfg(windows)] {
let error = Win32Error::from_code(0xC03A_0015);
assert_eq!(Win32Error::from_io(&error.to_io_error()), Some(error));
# }
```

## What it does not do

- **No named constants.** `windows-sys` has them.
- **No mapping onto a friendlier taxonomy.** A type carries what Windows said.
- **No `unsafe` and no native API surface.** The numbers and their conversions build on every
  target. The bridge to `io::Error` is Windows-only, because off Windows a raw OS error is an
  `errno`, and reading one as a Win32 code would be a wrong answer.

The decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) and the plan is in
[CHECKLIST.md](CHECKLIST.md).
