# Design notes: win-shared-os-owned-handle (Tier 1)

Current decisions for this crate. See [README.md](README.md) for what it is and
[CHECKLIST.md](CHECKLIST.md) for what is planned.

Repository-level decisions this crate sits under: the `win-` prefix and what `-sys` promises, both in
the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix).

## Decision index

| ID | Decision |
|---|---|
| <a id="sh-d-1"></a>SH-D-1 | **One type, for shared ownership only.** std already covers owning one handle alone and lending one; the missing case is shared ownership. |
| <a id="sh-d-2"></a>SH-D-2 | **What `SharedHandle` gives, and what it does not.** Four abilities; no I/O, no claim about how the handle was opened, only `CloseHandle` handles. |
| <a id="sh-d-3"></a>SH-D-3 | **The names.** Crate `win-shared-os-owned-handle`, type `SharedHandle`, both the engineer's. |

## SH-D-1: one type, for shared ownership only

*The engineer, 2026-10-06 and 07, during the durable-ioring design session, while choosing the file
type that crate's contract names.*

There are three ways to hold a handle, and std provides two:

- **Owning alone** -- `std::os::windows::io::OwnedHandle` (and `std::fs::File` for files).
- **Lending** -- `BorrowedHandle<'a>`, through the `AsHandle` trait.
- **Shared ownership** -- not in std.

Without a type for the third, crates in this workspace each wrapped `Arc<OwnedHandle>` under a name
of their own, with different and incomplete surfaces: `windows-ioring-sys`' `SharedFile` could not lend
its handle at all. This crate is the one shared form, so those crates can use it instead.

**Not wanted: a unified owning wrapper for every handle.** std's `OwnedHandle` already owns a
`CloseHandle` handle, and handles closed some other way (`CloseIoRing`, `FindClose`, the
thread-pool closes) need their own owners because the close differs. A single wrapper across them
would be wrong, not just redundant.

**Not now: a second, file-specific type.** A type that could prove a file was opened for
overlapped I/O would carry a real guarantee, but only if the one open call that sets
`FILE_FLAG_OVERLAPPED` were its only constructor. No consumer needs that guarantee in the type
today.

## SH-D-2: what `SharedHandle` gives, and what it does not

It gives:

1. **Many holders.** Cloning is a reference-count increment, never a new handle; the handle closes
   when the last clone drops.
2. **Lending.** `AsHandle` and `AsRawHandle`, so a holder can pass the handle to a Windows call
   without taking ownership.
3. **Conversions.** In from `OwnedHandle` or `std::fs::File`; back to `OwnedHandle` through
   `try_into_owned`, which succeeds only for the last holder and otherwise returns the value
   unchanged.
4. **A same-handle test.** `same_handle` says whether two values share one handle. Two handles
   opened separately on the same file are not the same handle.

It is also `Send + Sync`, `Clone` and `Debug`.

It does not:

- **Do I/O.** Reading, writing and flushing belong to whoever uses the handle.
- **Say how the handle was opened.** Nothing checks or promises overlapped or unbuffered access.
- **Cover handles closed other than by `CloseHandle`** (see SH-D-1).

It has no `unsafe` code (`#![forbid(unsafe_code)]`), no dependencies, and no native API surface.

## SH-D-3: the names

*The engineer, 2026-10-06: "I prefer win-shared-os-owned-handle", and for the type, "how about
just SharedHandle, I think that'll be unique enough".*

- **`win-`** per the workspace's prefix decision.
- **`os-owned-handle`** mirrors `std::os::windows::io::OwnedHandle`, which is all the crate wraps.
- **No `-sys`**: there is no native API surface and no `unsafe`.
- **`SharedHandle`**: the crate path carries the rest. std has no type of that name, and nothing
  else in the workspace uses it.
