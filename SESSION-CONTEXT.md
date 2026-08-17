# Session context

Updated: August 16, 2026

## Objective

Build `windows-threadpool-sys`, a Rust crate providing memory-safe and useful access to the object-based Windows
thread pool API. Do not expose raw completeness for its own sake: a public capability must have a defined model
for callback execution, cancellation, ownership, native destruction, and rundown.

The crate uses Rust edition 2024 with MSRV 1.97 and `windows-sys` 0.61.2 with default features disabled.

## Repository state

The repository was specialized from its template and committed as:

- `0051acc707043fa76c0b6c26f30937e2a7002759` (`chore: specialize repository for windows-threadpool-sys`)

Current design and dependency changes are uncommitted. Important modified files are:

- `DESIGN-NOTES.md`: SDK constraints, binding boundary, downstream directory-notification evaluation scenario,
  and the decision to design an overlapped-I/O foundation first.
- `PLANS.md`: near-term implementation order.
- `CHECKLIST.md`: outstanding design and implementation work.
- `crates/windows-threadpool-sys/Cargo.toml`: `windows-sys` features now include `Win32_System_Threading` and
  `Win32_System_IO`.
- `Cargo.lock`: contains `windows-sys` 0.61.2 and `windows-link` 0.2.1.

`crates/windows-threadpool-sys/src/lib.rs` remains a minimal crate skeleton. No safe API has been implemented.

## Established Windows contracts

- `StartThreadpoolIo` must be called before every overlapped operation submitted on a `TP_IO` handle.
- Every start must be balanced by exactly one eventual I/O callback or by `CancelThreadpoolIo` when submission
  fails immediately and no completion packet will arrive.
- `CancelThreadpoolIo` only reverses thread-pool notification accounting. It does not cancel kernel I/O.
- `CancelIoEx` requests cancellation but does not establish completion. The matching `OVERLAPPED` and payload
  cannot be freed or reused until the operation actually completes.
- `WaitForThreadpoolIoCallbacks(..., TRUE)` only discards callbacks that have not started; it does not cancel
  underlying I/O. The normal ownership path should drain with `FALSE` so completion remains the reclamation path.
- `CloseThreadpoolIo` must follow prevention of new submissions, completion or cancellation of native I/O, and
  callback drain.
- A separate stable `OVERLAPPED` is required for every simultaneous request.
- Callback code runs on process-managed threads, must not unwind through FFI, and should not block on downstream
  consumer progress.
- A file-handle instance can be associated with only one IOCP until closed. IOCP association is not removable.
  Operations issued through duplicated handles also generate notifications for the association.
- Do not enable `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` in a model that relies on one uniform callback-owned
  completion path.

## `windows-sys` boundary

`Win32_System_Threading` exposes the object-based thread pool functions and opaque `PTP_*` values. Its
`PTP_WIN32_IO_CALLBACK` represents the operation pointer as `*mut c_void`.

`Win32_System_IO` exposes `OVERLAPPED`, `OVERLAPPED_ENTRY`, `CancelIoEx`, `GetOverlappedResult`, raw IOCP
creation, packet post, and packet dequeue functions. This feature is now enabled because the overlapped-I/O
foundation is a prerequisite for public thread-pool I/O.

`SetFileCompletionNotificationModes` is under `Win32_Storage_FileSystem` and remains deferred. Filesystem- or
operation-specific dependencies should not be pulled into the core until required.

The SDK callback-environment helpers are header-only and absent from `windows-sys`. Narrow equivalents are still
needed for initialization and mutation of `TP_CALLBACK_ENVIRON_V3`; zeroed/default storage is not equivalent to
SDK initialization.

## Current architecture decision

Design and implement a reusable owned overlapped-I/O foundation before exposing `TP_IO`. This is a layer above
`windows-sys`, not a general filesystem library and not a directory-notification adapter.

The foundation should separate:

1. Typed ownership and the resource's documented destructor.
2. A consuming transition from an unassociated overlapped endpoint to one completion backend.
3. Pinned per-request `OVERLAPPED` storage coupled to its stable payload and explicit operation state.
4. Backend-specific completion, cancellation, and rundown.

Reuse `std::os::windows::io::OwnedHandle`, `OwnedSocket`, and their borrowing traits where the resource uses
`CloseHandle` or `closesocket`. Add typed owners only for resources with specialized destruction. Do not create
an untyped universal Windows handle owner.

Raw IOCP and thread-pool I/O may share endpoint and operation storage, but remain distinct backends:

- Raw IOCP owns the completion-port handle, completion keys, packet dequeue, and explicit worker policy. This
  project does not need to create or manage IOCP worker threads.
- `TP_IO` uses a system-managed internal IOCP and adds exact `StartThreadpoolIo` accounting. Its internal port is
  not exposed.

Moving an arbitrary `OwnedHandle` into an associated-endpoint wrapper is not enough for a completely safe
constructor: the handle may already be associated, may not have been opened for overlapped I/O, or may have been
duplicated before transfer. A safe constructor needs controlled provenance, either by creating the endpoint or
by consuming a sealed type whose creator established overlapped mode and exclusive completion routing. An
associated endpoint must not expose cloning or an unrestricted raw-handle escape hatch.

## Unresolved safety boundary

Generic overlapped submission is the decisive open question. A safe API cannot accept an arbitrary raw handle,
`OVERLAPPED` pointer, payload, and caller-reported submission result while proving all of the following:

- exactly one native operation was issued;
- the operation used the supplied storage;
- a completion packet will or will not arrive;
- `StartThreadpoolIo` accounting is balanced; and
- storage is reclaimed only after actual completion.

Prototype a constrained owned-operation model before committing a public `TP_IO` API. The prototype must cover:

- immediate submission failure;
- immediate success with a completion packet;
- pending completion;
- targeted and whole-endpoint cancellation races;
- completion identity;
- endpoint shutdown with operations outstanding; and
- result payloads retained after native endpoint shutdown.

The result will determine whether safe generic submission is possible or whether operation-specific safe
adapters, potentially in downstream crates, are required. A narrow unsafe extensibility boundary remains an
alternative only if a safe model cannot express useful operations.

## Directory notification evaluation scenario

A future directory-notification project informs this design but is not a deliverable of this crate. Its expected
capabilities are:

- dynamically add and remove watched paths;
- monitor subtrees recursively;
- use `ReadDirectoryChangesExW` where available;
- fall back to `FindFirstChangeNotification` where necessary;
- rotate two or more stable notification buffers; and
- use SQ/CQ-style rings for submissions and completions.

This scenario tests independently removable `TP_IO` and `TP_WAIT` registrations, object-local rundown, stable
operation storage, prompt rearming, and nonblocking callback dispatch into a caller-selected bounded ring. Path
registries, recursive-watch policy, fallback selection, ring layout, capacity, overflow policy, parsing, and
buffer recycling remain downstream concerns.

Directory notification path components must remain opaque sequences of 16-bit units. They commonly contain
UTF-16LE but are not guaranteed to be well-formed UTF-16. A higher layer may validate records and publish offsets
into retained immutable buffers without copying or decoding names. This crate must not force a payload
representation that prevents that design.

## Immediate plan

1. Finalize shared ownership, cancellation, callback, and destruction invariants. The toolchain and platform
   baseline is fixed: Rust 1.97 / edition 2024, minimum Windows Server 2025 / Windows 11 (validated by GitHub
   CI's `windows-latest` runner).
2. Prototype owned overlapped endpoints, pinned operations, completion identity, and IOCP association over
   `Win32_System_IO`.
3. Exercise the prototype against raw IOCP and `TP_IO` accounting across all completion and cancellation cases.
4. Implement SDK-equivalent callback-environment initialization and mutation.
5. Implement one end-to-end work submission abstraction and use it to validate callback ownership before
   extending to timers, waits, and I/O.

Do not implement the directory watcher in this crate and do not expose public thread-pool I/O until the owned
operation prototype resolves the generic-submission boundary.

## Validation status

The following checks passed after enabling `Win32_System_IO` and updating the design documents:

```text
cargo build --workspace --all-targets --locked
tools/check-encoding.ps1
git diff --check -- DESIGN-NOTES.md CHECKLIST.md PLANS.md \
    crates/windows-threadpool-sys/Cargo.toml Cargo.lock
```

Earlier repository validation also passed formatting, build/check, Clippy with warnings denied, tests,
packaging, encoding, and whitespace checks. No implementation exists yet, so there are no behavioral tests for
the new foundation.

## Primary references

- <https://learn.microsoft.com/windows/win32/procthread/thread-pools>
- <https://learn.microsoft.com/windows/win32/procthread/thread-pool-api>
- <https://learn.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-createthreadpoolio>
- <https://learn.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-startthreadpoolio>
- <https://learn.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-cancelthreadpoolio>
- <https://learn.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-waitforthreadpooliocallbacks>
- <https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-createiocompletionport>
- <https://learn.microsoft.com/windows/win32/fileio/i-o-completion-ports>
- <https://learn.microsoft.com/windows/win32/api/minwinbase/ns-minwinbase-overlapped>
- <https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-cancelioex>
- <https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-readdirectorychangesexw>
- <https://docs.rs/windows-sys/0.61.2/windows_sys/Win32/System/Threading/>
