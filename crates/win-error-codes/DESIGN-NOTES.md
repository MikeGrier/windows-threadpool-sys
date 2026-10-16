# Design notes: win-error-codes (Tier 1)

Current canonical decisions. The plan is in [CHECKLIST.md](CHECKLIST.md). The crate does not exist
as code yet.

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

## Working positions

Not decisions: nothing binds to them, and `WE-1.1` is where each is settled and given an ID.

- **Whether the crate builds on every target.** It holds numbers and `io::Error` conversions, so
  nothing in it needs `cfg(windows)`. The lean is to let it build everywhere, so a portable crate
  such as durable-ioring's core could use it. That would add it to the repository's portable CI job,
  which has not been read for this.
- **`NTSTATUS`.** A third signed numbering space with a severity and facility layout different from
  `HRESULT`'s. It is not in the first cut. The engineer was asked and has not asked for it; nothing
  in the repository receives one from a documented call, and converting one to a Win32 code is an
  `Rtl*` call the repository's rule on documented interfaces excludes.
