# win-shared-os-owned-handle

A shared owner of a Windows handle.

std covers two of the three ways to hold a handle: `OwnedHandle` owns one alone, and
`BorrowedHandle` lends one. This crate is the third, **shared ownership**. `SharedHandle` is an
`OwnedHandle` behind a reference count: several holders keep one handle open, and it closes when
the last of them drops.

## What it gives

- **Many holders.** Cloning is a reference-count increment, never a new handle.
- **Lending.** `AsHandle` and `AsRawHandle`, so a holder can pass the handle to a Windows call
  without taking ownership of it.
- **Conversions.** In from an `OwnedHandle` or a `std::fs::File`; back out to an `OwnedHandle`
  through `try_into_owned`, for the last holder.
- **A same-handle test.** `same_handle` says whether two values share one handle.

## What it does not do

- **No I/O.** It carries a handle; reading, writing and flushing belong to whoever uses it.
- **No claim about how the handle was opened.** Overlapped or not, buffered or not: nothing here
  checks or promises it.
- **Only the handles `OwnedHandle` is for** -- those closed with `CloseHandle`. Handles with another
  close function keep their own owners.

There is no `unsafe` code and no native API surface: the crate is std's `OwnedHandle` and `Arc` and
nothing more. It builds only on Windows.

Why it exists, and why it is named as it is: [DESIGN-NOTES.md](DESIGN-NOTES.md).
