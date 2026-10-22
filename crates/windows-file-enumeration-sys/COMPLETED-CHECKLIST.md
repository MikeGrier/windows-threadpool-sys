# Completed checklist: windows-file-enumeration-sys

Append-only. Active work is in [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-10-15 23:24:12 -04:00 -- ERR-1: the shared `Win32Error`

### <a id="err-1"></a>ERR-1 -- `Win32Error` is `win_error_codes::Win32Error`, re-exported under the same name. *(completed 2026-10-15 23:24:12 -04:00)*

This crate's own type panicked converting a code of 2^31 or more to an `io::Error` (the virtdisk
errors are such codes), and its `from_io` returned the code `0`, which is `ERROR_SUCCESS`, for an
error with no OS code behind it, so a fabricated error read as success. The shared type does
neither. The decisions are [WE-D-1](../win-error-codes/DESIGN-NOTES.md#we-d-1) and
[WE-D-4](../win-error-codes/DESIGN-NOTES.md#we-d-4). The code is the commit that adds the dependency
and deletes the type from `src/error.rs`.

The re-export keeps every path, and the commit is a breaking one because a caller could see this
change: `from_io` returns `Option<Win32Error>`, `None` for an error with no OS code, where it returned
a `Win32Error` of `0`. Also visible: `to_io_error` no longer panics and the code survives it; `Display`
reads `Win32 error N (0xHHHHHHHH): message` where it read `Win32 error N (message)`; and the type now
implements `std::error::Error` and converts into `io::Error`.

The item said each caller of `from_io` would decide what an error with no code means to it. The one
caller was this crate's own `last`, which reads the thread's last error and so always has a code, and
the shared type has `last` of its own; nothing in the workspace outside the crate used the function.
No sabotage manifest exists for this crate, so the tests carry the change: a code of 2^31 or more is
converted and read back, and an error with no code is `None`.

## Moved 2026-10-18 20:49:59 -04:00 -- PATH: one path contract across the namespace crates

### <a id="path-1"></a>PATH-1 -- This crate takes the request crate's path contract, `AnchoredPath`, and its older copy of the preparation is deleted. *(completed 2026-10-18 20:49:59 -04:00)*

The item, as it was written: consume the namespace request crate's path contract, and delete this crate's older copy
of the preparation. It was the merge-or-delete decision of the workspace checklist's `M26+.3`
([CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md)) and of
[D-9](../windows-namespace-request-sys/DESIGN-NOTES.md#d-9) in the request crate, made on 2026-10-15 at the engineer's
request as: this crate depends on `windows-namespace-request-sys` and its `path.rs` preparation is deleted. What
it named as changing: `D-7`'s `MAX_PATH` cap on an ordinary path goes; a request captures the current directory when
it is built and grafts a clearly safe child onto it; `EnumerationRequest::path()` says what it returns, the path as
given, with a second accessor for the path handed to Win32; an interior NUL is refused. A `feat!` of a published
crate.

What was done. The request crate's `AnchoredPath` was made public to be consumed (its
[D-30](../windows-namespace-request-sys/DESIGN-NOTES.md#d-30)). A request holds one; `path()` is the path as given and
`win32_path()`, new, is what the worker opens. `RequestFailure` loses `EmptyPath`, `PathTooLong`, `NotFullyQualified`
and `PathResolution` and gains `PathCapture`, a failure to read the current directory, with the code when Windows
gave one; so an empty path, a long one and an unqualified `\\?\` path reach the open, and its error is the terminal
`DirectoryOpen`. `src/path.rs` and its tests are deleted. Recorded as [D-22](DESIGN-NOTES.md#d-22), with the rationale
in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The crate's first [sabotage.json](sabotage.json) covers the binding:
the worker opening the path as given is caught only by a child-process test that moves the current directory after
the request is built.

## Moved 2026-10-19 20:10:23 -04:00 -- SAFE-1: the metadata test hands the request crate a SharedHandle

### <a id="safe-1"></a>SAFE-1 -- The metadata test builds its `QueryFileIdInfo` from a `SharedHandle`, not a `CapturedHandle`. *(completed 2026-10-19 20:10:23 -04:00)*

The item as written: "**The metadata test takes a `SharedHandle`.**
`a_required_identity_is_what_the_request_crate_reads_from_the_file_itself` in
[metadata.rs](tests/integration/metadata.rs) builds its `QueryFileIdInfo` from a `SharedHandle` rather than a
`CapturedHandle`." Prerequisite: the request crate's `NR-6.1`.

What was done. `QueryFileIdInfo::new` takes `impl Into<SharedHandle>` since `NR-6.1`, and `SharedHandle` is
`From<File>`, so the test hands it the opened file: the file is the handle's one owner, and nothing is
captured or duplicated. No dependency was added. That was this crate's last use of `CapturedHandle`. Evidence, in
[sabotage.json](sabotage.json): reading the identity from a different file than the one enumerated is caught by the
test, and a control opening the same file another way survives.

## Moved 2026-10-20 15:42:27 -04:00 -- SAFE-2: the directory's calls are the request crate's entries

### <a id="safe-2"></a>SAFE-2 -- The directory's open and queries are the request crate's entries, the buffer an `AlignedBuf` and the handle a `SharedHandle`. *(completed 2026-10-20 15:42:27 -04:00)*

The item as written, with the paragraph that tied it to the workspace's `M21.3` and `M21.4`:

**The directory's calls through the request crate.** The engine holds the directory as a
  `SharedHandle`. [native.rs](src/native.rs)'s four calls become entries: the open is `OpenFile` built from the
  request's `AnchoredPath` and performed under the captured token; the directory check is `QueryFileBasicInfo`;
  the volume serial is `QueryFileIdInfo`, on a clone of the directory's handle; a refill is `QueryFileInformation`
  into the crate's buffer, which becomes an `io_buffers::AlignedBuf` in place of `NativeBuffer`. The typed
  outcomes `native.rs` derives (the end of a directory, `UnsupportedExtendedDirectoryInfo`, and the rest) are kept
  and read from the entries' codes. Tests: the existing suite unchanged; an enumeration makes no handle beyond its
  open.

  This discharges the workspace's [M21.3 and M21.4](../../CHECKLIST.md#m21----reconcile-with-the-shipped-namespace-plane-work),
  which are checked off in the same commit, and their constraints are this item's acceptance conditions: the open
  keeps its access (`FILE_LIST_DIRECTORY`), share mode and flags (`FILE_FLAG_BACKUP_SEMANTICS`); the directory's
  handle stays unassociated; every code reaches the crate unaltered; the open's code is captured before the
  impersonation context is restored; and the D-15 Globazog acceptance gate still passes.

  > **CROSS-COMPONENT PREREQUISITE:** the request crate's release carrying `NR-6.1` to `NR-6.5`
  > ([COMPLETED-CHECKLIST.md](../windows-namespace-request-sys/COMPLETED-CHECKLIST.md#nr-65-into)).

What was done. [native.rs](src/native.rs) makes no call of its own: the open is `OpenFile` built from the request's
`AnchoredPath` and performed under the captured token, with listing access, every share, an existing directory and
backup semantics; the directory check is `QueryFileBasicInfo`; the volume serial is `QueryFileIdInfo`; and a refill is
`QueryFileInformation`, the restart class for the first and the continuing class for the rest, each on a clone of the
directory's `SharedHandle`. The staging buffer is an `io_buffers::AlignedBuf` made at 8, which replaced `NativeBuffer`
and its `buffer.rs`; the engine holds the directory as a `SharedHandle`. The typed outcomes are kept and read from the
entries' codes. `perform_into` now returns the request crate's `PerformIntoError`, whose refusal ahead of the call
cannot apply to the extended-directory classes, so that arm reports a failure with no code. Two dependencies were
added, `io-buffers` and `win-shared-os-owned-handle`; no public item changed. Recorded as
[D-24](DESIGN-NOTES.md#d-24), with the rationale in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Tests: the existing
suites unchanged apart from the buffer's constructor, including the Globazog acceptance gate; new unit tests of the
open's parameters, the refill's class and handle, a held directory not stopping a rename, and the engine and a whole
read leaving the directory with no other holder. Evidence, in [sabotage.json](sabotage.json): nine entries, the open's
delete sharing, access, backup semantics and overlapped flag, the first refill's class, the directory check, and the
volume serial, all caught, one declared survivor for the arm that cannot be reached, and a control.

## Moved 2026-10-20 16:05:00 -04:00 -- SAFE-3: the doorbell is a shared event

### <a id="safe-3"></a>SAFE-3 -- The doorbell is a `win-sync-sys` `Event`, and a `ThreadpoolWait` is handed a waitable handle that shares it, not a duplicate. *(completed 2026-10-20 16:05:00 -04:00)*

The item as written:

**The doorbell is a shared event.** The completion ring holds the doorbell as a `win-sync-sys`
  `Event`, set and reset through it, and hands a `ThreadpoolWait` a waitable handle that shares the event rather
  than a duplicate; `completion_ring.rs`'s `DuplicateHandle`, `SetEvent` and `ResetEvent` are gone. The unit tests'
  raw waits move to `win-sync-sys`'s `wait_for_single_object`.

  > **CROSS-COMPONENT PREREQUISITE:** [WS-2.2](../win-sync-sys/COMPLETED-CHECKLIST.md#ws-22) and
  > [M-T15.1](../windows-threadpool-sys/CHECKLIST.md).

What was done. [completion_ring.rs](src/completion_ring.rs) holds the doorbell as a `win_sync_sys::Event` and sets and
resets it through that type; it makes no `SetEvent`, `ResetEvent` or `DuplicateHandle` call and its `duplicate`
helper is gone. `Receiver::doorbell_owned` is replaced by `Receiver::doorbell_waitable`, a
`windows_threadpool_sys::wait::WaitableHandle` made from a clone of the ring's event, so the wait shares the ring's
handle; a client cannot set or reset it without `unsafe`. `Receiver::doorbell` is unchanged. The removal is a breaking
change to the public surface. `win-sync-sys` was added as a dependency. The unit tests' raw waits and the model's
check use `wait_for_single_object`, and the model no longer duplicates the handle. Recorded as
[D-25](DESIGN-NOTES.md#d-25), with the rationale in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Tests: the owned-handle
test is replaced by tests that the target's raw handle is the ring's and sees what the ring signals, that the ring and
the target are each other's other holder, and that the event outlives the ring; and a real `ThreadpoolWait` armed on
the target is woken by an entry, drains, and is woken again. Evidence, in [sabotage.json](sabotage.json): four
entries, the target made a second event, set and reset swapped, the initial state ignored, and the refresh removed, all
caught, and a control. The engine tests' raw byte write is not a wait and remains for `SAFE-5`.

## Moved 2026-10-20 16:55:17 -04:00 -- SAFE-4: the name comparison is win-text-sys's

### <a id="safe-4"></a>SAFE-4 -- Case-insensitive name matching is `win-text-sys`'s `compare_string_ordinal`. *(completed 2026-10-20 16:55:17 -04:00)*

The item as written:

**Case-insensitive name matching through `win-text-sys`.** [pattern.rs](src/pattern.rs)'s
  `CompareStringOrdinal` call is that crate's wrapper.

  > **CROSS-COMPONENT PREREQUISITE:** [M43.1](../../COMPLETED-CHECKLIST.md#m431) (the crate is created).

What was done. [pattern.rs](src/pattern.rs) compares an equal-length pair of runs through `compare_string_ordinal` with
`Case::Insensitive`, and reads the result in `equal_from`: equal is a match, a failure of the call is not, and a run too
long for the call's count is compared exactly. The empty-run shortcut is gone, since the wrapper always passes the
count. `win-text-sys` is a dependency, and the `Win32_Globalization` and `Win32_System_Threading` features of
`windows-sys` are removed, the second having been unused since `SAFE-3`. Recorded as [D-26](DESIGN-NOTES.md#d-26), with
the rationale in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Tests: the existing pattern tests unchanged, and new tests
of the empty run and of each result, including the too-long arm that the earlier code could only argue. Evidence, in
[sabotage.json](sabotage.json): four entries, all caught, and a control.

## Moved 2026-10-20 17:03:19 -04:00 -- SAFE-5: the crate forbids unsafe code

### <a id="safe-5"></a>SAFE-5 -- `#![forbid(unsafe_code)]` is on the crate root, and D-23 is current. *(completed 2026-10-20 17:03:19 -04:00)*

The item as written:

**`#![forbid(unsafe_code)]`.** On the crate root, so the build refuses any `unsafe` in the
  crate or its unit tests. Whatever `unsafe` remains after `SAFE-2` to `SAFE-4` is found by the build, and each
  is moved behind a wrapper or brought to the engineer. D-23 is then current rather than a target.

What was done. [lib.rs](src/lib.rs) carries `#![forbid(unsafe_code)]` in place of `forbid(unsafe_op_in_unsafe_fn)`,
which a crate with no `unsafe` has no use for, and its safety section says the crate holds none. The one `unsafe` the
build found was in a test, a four-byte write that corrupted a record's `NextEntryOffset`; it is now a write through the
engine buffer's `as_mut_slice`, so nothing was moved behind a wrapper or brought to the engineer. [D-23](DESIGN-NOTES.md#d-23)
is stated as current, and the earlier steps' "scheduled work" lines say there is none. Recorded in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Evidence, in [sabotage.json](sabotage.json): an `unsafe` block added to the
library is refused by the build, and a control adding the same item without `unsafe` survives. With this
the `SAFE` group is complete, and its stubs left the checklist with it.

## Moved 2026-10-21 21:46:17 -04:00 -- windows-file-enumeration-sys: milestones PATH and ERR are complete

### <a id="path----one-path-contract-across-the-namespace-crates"></a>PATH -- One path contract across the namespace crates

  **Next:** `REVIEW-1`, in [PARKED.md](PARKED.md), is re-read against the new contract. After that nothing further in this
  crate; the order across the crates is at the top of
  [durable-ioring's CHECKLIST.md](../durable-ioring/COMPLETED-CHECKLIST.md#open-surface-order).

Items, each archived above under its own heading: [PATH-1](#path-1).

### <a id="err----one-error-code-type-across-the-crates"></a>ERR -- One error code type across the crates

Items, each archived above under its own heading: [ERR-1](#err-1).
