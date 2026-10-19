# Checklist: windows-file-enumeration-sys

The crate's implementation milestones are M5 through M7 in the workspace
[CHECKLIST.md](../../CHECKLIST.md). This file holds work owned by the crate
itself. Status is tracked in [PLANS.md](PLANS.md).

## PATH -- One path contract across the namespace crates

- [x] **PATH-1** -- This crate takes the request crate's path contract, `AnchoredPath`, and its older copy of the preparation is deleted. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#path-1)

  **Next:** `REVIEW-1` below is re-read against the new contract. After that nothing further in this
  crate; the order across the crates is at the top of
  [durable-ioring's CHECKLIST.md](../durable-ioring/CHECKLIST.md#where-the-open-surface-work-goes-next-read-this-first-when-picking-the-work-back-up).

## ERR -- One error code type across the crates

- [x] **ERR-1** -- `Win32Error` is `win_error_codes::Win32Error`, re-exported under the same name. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#err-1)

## SAFE -- Every Win32 call through a safe wrapper, then `forbid(unsafe_code)`

The target is [D-23](DESIGN-NOTES.md#d-23): this crate reaches Windows only through safe wrappers in other crates,
holds its directory as a `SharedHandle`, and duplicates no handle. The order across the crates is `M43` in the
workspace [CHECKLIST.md](../../CHECKLIST.md).

- [ ] **SAFE-1** -- **The metadata test takes a `SharedHandle`.** `a_required_identity_is_what_the_request_crate_reads_from_the_file_itself`
  in [metadata.rs](tests/integration/metadata.rs) builds its `QueryFileIdInfo` from a `SharedHandle` rather than
  a `CapturedHandle`.

  > **CROSS-COMPONENT PREREQUISITE:** [NR-6.1](../windows-namespace-request-sys/CHECKLIST.md). This is one of the
  > moves `NR-6.2` waits on, so it lands between the two.

- [ ] **SAFE-2** -- **The directory's calls through the request crate.** The engine holds the directory as a
  `SharedHandle`. [native.rs](src/native.rs)'s four calls become entries: the open is `OpenFile` built from the
  request's `AnchoredPath` and performed under the captured token; the directory check is `QueryFileBasicInfo`;
  the volume serial is `QueryFileIdInfo`, on a clone of the directory's handle; a refill is `QueryFileInformation`
  into the crate's buffer, which becomes an `io_buffers::AlignedBuf` in place of `NativeBuffer`. The typed
  outcomes `native.rs` derives (the end of a directory, `UnsupportedExtendedDirectoryInfo`, and the rest) are kept
  and read from the entries' codes. Tests: the existing suite unchanged; an enumeration makes no handle beyond its
  open.

  > **CROSS-COMPONENT PREREQUISITE:** the request crate's release carrying `NR-6.1` to `NR-6.5`
  > ([CHECKLIST.md](../windows-namespace-request-sys/CHECKLIST.md)).

- [ ] **SAFE-3** -- **The doorbell is a shared event.** The completion ring holds the doorbell as a `win-sync-sys`
  `Event`, set and reset through it, and hands a `ThreadpoolWait` a waitable handle that shares the event rather
  than a duplicate; `completion_ring.rs`'s `DuplicateHandle`, `SetEvent` and `ResetEvent` are gone. The unit tests'
  raw waits move to `win-sync-sys`'s `wait_for_single_object`.

  > **CROSS-COMPONENT PREREQUISITE:** [WS-2.1](../win-sync-sys/CHECKLIST.md) and
  > [M-T15.1](../windows-threadpool-sys/CHECKLIST.md).

- [ ] **SAFE-4** -- **Case-insensitive name matching through `win-text-sys`.** [pattern.rs](src/pattern.rs)'s
  `CompareStringOrdinal` call is that crate's wrapper.

  > **CROSS-COMPONENT PREREQUISITE:** [M43.1](../../CHECKLIST.md) (the crate is created).

- [ ] **SAFE-5** -- **`#![forbid(unsafe_code)]`.** On the crate root, so the build refuses any `unsafe` in the
  crate or its unit tests. Whatever `unsafe` remains after `SAFE-2` to `SAFE-4` is found by the build, and each
  is moved behind a wrapper or brought to the engineer. D-23 is then current rather than a target.

## M-inf -- Horizon (ungated)

- [ ] **REVIEW-1** -- Review the request path contract against a traversal layer, before one is built.
  **Re-read after [PATH-1](#path----one-path-contract-across-the-namespace-crates):** that item
  replaces the contract below (no `MAX_PATH` cap, no `\\?\` rewriting, a graft onto a captured root),
  so most of its questions are about a contract that no longer exists, and this review may close
  with a note of which still stand.
  **This is a review item: it schedules no change.** Its output is an answer -- possibly "no change
  needed" -- and any work it turns out to imply becomes its own item afterwards. Raised from the
  `windows-file-watcher` side while recording
  [D-85](../windows-file-watcher/DESIGN-NOTES.md#d-85) and the shared principle in
  [the workspace design notes](../../DESIGN-NOTES.md#path-contracts-follow-path-construction);
  flagged here because that principle's *building* half lives in this crate and nothing else would
  bring it up.
  **Context, not conclusions.** [lib.rs](src/lib.rs) states that recursive traversal belongs in a
  separate layer composing this one, so that layer does not exist yet and the contract has never been
  exercised by the consumer it was designed for. Facts noted while reading, each of which the review
  should confirm rather than take on trust:
    - [D-7](DESIGN-NOTES.md#d-7) accepts an *ordinary* path only if both the input and the
      `GetFullPathNameW`-resolved form fit `MAX_PATH`, deliberately, so acceptance does not depend on
      the host executable's `longPathAware` manifest.
    - Win32 has no relative open, so descending means appending to an absolute path -- from a path
      already at or near that cap.
    - The contract's stated remedy for an over-limit path is "supply a fully qualified `\\?\` path",
      which for a traversal layer means converting partway down rather than at the caller's boundary.
    - `EnumerationRequest::path()` is `pub` and returns the stored resolved form, so a traversal layer
      has access to a Win32-normalised base rather than only the caller's original string.
  **Questions the review should answer.** Does D-7's contract still hold once descent is a real
  consumer, or does the `MAX_PATH` cap on ordinary paths become reachable in normal use? If a
  traversal layer must move into `\\?\` form mid-descent, is that conversion specified anywhere, and
  is it the same conversion for every namespace the crate accepts -- a UNC path needs `\\?\UNC\...`
  rather than a bare prefix, and a `\\.\` device form does not take one at all? Does
  manifest-independence still mean what it should for requests a traversal layer *derives* rather than
  receives? And is `path()`'s resolved form the intended base for that, stated as such, rather than
  something a traversal layer is left to infer?
  **The trap worth naming.** Converting *before* Win32 has normalised under the caller's own parsing
  mode silently reinterprets the path; converting *after* preserves it. Both crates reached that rule
  independently, which is why it is recorded once at the workspace level -- but reaching for the
  prefix at the wrong moment is exactly what a traversal layer under length pressure would do.
