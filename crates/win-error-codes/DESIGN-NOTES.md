# Design notes: win-error-codes (Tier 1)

Current canonical decisions. The plan is in [CHECKLIST.md](CHECKLIST.md).

**Why the crate exists.** Three crates hold a Win32 error code, and two of them had grown their own
`Win32Error`: [windows-namespace-request-sys](../windows-namespace-request-sys/src/outcome.rs) and
[windows-file-enumeration-sys](../windows-file-enumeration-sys/src/error.rs). They differ, and both
convert a code to `io::Error` wrongly for a code of 2^31 or more. The namespace crate falls back to
`io::Error::other(String)`, so `raw_os_error()` is `None` and its documentation's claim that "the
raw code survives" is false for those codes. The enumeration crate `expect`s that a code fits in an
`i32`, which would panic. Codes that large exist: `windows-sys` defines
`ERROR_VIRTDISK_NOT_VIRTUAL_DISK` as a `WIN32_ERROR` of 3225026581, and a real call returned it. A
third crate, `win-volume-sys`, has none and uses `io::Error`. The same situation produced
[win-shared-os-owned-handle](../win-shared-os-owned-handle/README.md), and this is the same remedy.

## Decision index

| ID | Decision |
|---|---|
| <a id="we-d-1"></a>WE-D-1 | **Three types, one per numbering space, and no distinction between "status" and "code".** The engineer, 2026-10-15. `Win32Error` is a `u32`, the `DWORD` `GetLastError` returns and a call such as `GetStorageDependencyInformation` returns directly. `LStatus` is an `i32`, the `LONG` that registry calls return; it is the same number space and converts to `Win32Error` and back by a bit cast, and it is its own type because it is the signed carrier. `HResult` is an `i32`, signed by definition (`FAILED(hr)` is `hr < 0`), with severity and facility bits. The names "status" and "code" are historical and mean the same thing, so no type models a difference between them, and `ERROR_SUCCESS` (0) is an ordinary `Win32Error`. **Not assumed: that a Win32 code fits in 16 bits.** The virtdisk errors do not, so no conversion in this crate masks to 16 bits except `HRESULT_FROM_WIN32`'s own rule. **The `io::Error` boundary is the only place a code becomes an `i32`**, because std's `io::Error::from_raw_os_error` takes and `raw_os_error` returns an `i32`; the cast there is bit-preserving and exact in both directions. **No named constants are defined here**: `windows-sys` carries them. |
| <a id="we-d-2"></a>WE-D-2 | **The name is the engineer's: `win-error-codes`.** The workspace rule on prefixes ([DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix)) gives `win-` to a crate that raises Windows into safe Rust. This crate calls no Windows API; it holds the vocabulary Windows' calls report in, which is the reason it takes the prefix and not a facility name. It carries no `-sys` suffix, because that half of the rule means thin-over-Win32, and this layers over no call. |
| <a id="we-d-3"></a>WE-D-3 | **The numbers build on every target; the `io::Error` bridge is `cfg(windows)`.** Settled in `WE-1.1`, overruleable. The types hold numbers, so nothing about them is Windows-specific and a portable crate (durable-ioring's core is one, [DI-D-49](../durable-ioring/DESIGN-NOTES.md#di-d-49)) can use them. The bridge is not portable in meaning: off Windows `io::Error::raw_os_error` is an `errno`, so `from_io` would read an `errno` as a Win32 code, and that is a wrong answer, which is the ground for removing an option, where a missing feature is not. So `from_io`, `to_io_error`, `last` and the `From` impl are `cfg(windows)`, and the crate is in the repository's portable CI job. A Linux `check` and `clippy` against the installed target pass locally; the portable job is where its tests run there. |
| <a id="we-d-4"></a>WE-D-4 | **An `io::Error` with no OS code is `None`, never zero, and `last` needs no `unsafe`.** Settled in `WE-1.1`, overruleable. Zero is `ERROR_SUCCESS`, so `from_io` returning it for an error that carries no code would make a fabricated error read as a success; the enumeration crate's copy did this. `last` reads `io::Error::last_os_error`, which on Windows is `GetLastError`, so the crate has no `unsafe` and no `windows-sys` dependency outside its tests. The namespace crate's own snapshot-after-the-call guarantee for `perform` stays in that crate and builds the value with `from_code`. |
| <a id="we-d-5"></a>WE-D-5 | **`LStatus` converts to and from `Win32Error` two ways: a `const` inherent method each, and a `From` impl each; it is not an `Error`, and its `Display` is the `Win32Error`'s.** Settled in `WE-1.2`, overruleable. The two are one number space ([WE-D-1](#we-d-1)), so the conversion is a bit cast and loses nothing in either direction, and a negative `LONG` is a code above 2^31, not its magnitude. The inherent methods (`LStatus::to_win32`, `Win32Error::to_lstatus`) are `const` because a `From` impl cannot be; the `From` impls are the form the rest of the workspace will write. `LStatus` does not implement `std::error::Error`, because a value of zero is success and an error type that can be success is wrong: `into_result` is how it becomes a `Win32Error`, which is. Its `Display` delegates, since the signed carrier has no rendering of its own. |

## Working positions

Not decisions: nothing binds to them.

- **`NTSTATUS`.** A third signed numbering space with a severity and facility layout different from
  `HRESULT`'s. It is not in the first cut. The engineer was asked and has not asked for it; nothing
  in the repository receives one from a documented call, and converting one to a Win32 code is an
  `Rtl*` call the repository's rule on documented interfaces excludes.
