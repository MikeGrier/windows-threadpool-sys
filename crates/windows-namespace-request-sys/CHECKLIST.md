# Checklist: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), and how they were
reached in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). This crate's *creation* is
tracked separately, in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestones
M24-M26; that file is feature-scoped and is deleted when its feature completes,
so durable follow-up work for the crate belongs here instead.

Completed work is in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## NR-6 -- A request shares the caller's handle

`NR-inf.1`, graduated 2026-10-18 by the engineer's ruling recorded as [D-32](DESIGN-NOTES.md#d-32): a request
holds a `SharedHandle`, and nothing in this crate duplicates a handle. The parked item's text is archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#nr-inf1-graduated), and D-32 answers each of its questions.
The order across crates is `M43` in the workspace [CHECKLIST.md](../../CHECKLIST.md). Here: `NR-6.1`, then the
consumers move (the handoff below), then `NR-6.2`. `NR-6.3` to `NR-6.5` are additive and independent of those and
of each other. Then this crate is released, a new minor.

- [x] **NR-6.1** -- Every entry that takes a handle holds a `SharedHandle`, built from `impl Into<SharedHandle>`, and all but the two carrying security attributes derive `Clone`. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#nr-61-shared)

  > **-> CROSS-COMPONENT HANDOFF:** the consumers move off `CapturedHandle` before `NR-6.2`:
  > [DI-16.1](../durable-ioring/CHECKLIST.md) (`win-durable-ioring`),
  > [M7.1](../windows-platform-probes/COMPLETED-CHECKLIST.md#m71) (`windows-platform-probes`), and
  > [SAFE-1](../windows-file-enumeration-sys/CHECKLIST.md) (`windows-file-enumeration-sys`' metadata test).

- [ ] **NR-6.2** -- **Capture is deleted.** `CapturedHandle`, `capture`, `capture_raw`, `into_owned_handle`,
  `HandleCaptureError`, `HandleCaptureFailure`, the transition `From`, the crate-private `duplicate` helper, each
  `Clone` entry's `try_clone`, and the `handle` module's tests of D-7's refusals. `OpenFile`'s and
  `OpenFileByIdentifier`'s `try_clone` stay, for their security attributes, and share the handle instead of
  duplicating it; their error type then reports only the attributes' copy. Whether `new` keeps `impl Into<SharedHandle>` (which takes an `OwnedHandle` or a
  `File` as it is) or takes `SharedHandle` is settled here and recorded in D-32. [D-4](DESIGN-NOTES.md#d-4) is
  marked superseded and [D-7](DESIGN-NOTES.md#d-7) withdrawn, both by D-32. The crate documentation's "A path is
  copied; a handle is duplicated" and the README are rewritten for sharing; sweep `duplicat` and `captur` across
  the crate's `src`, `tests`, `README.md` and design notes. Breaking. Needs every consumer moved (the handoff
  above).

- [ ] **NR-6.3** -- **`OpenFile` and `WatchDirectory` from an `AnchoredPath`.** An infallible constructor on each
  taking an `AnchoredPath` the caller already holds, so a crate that anchored its path when its own request was
  built (the enumeration crate's `SAFE-2`) does not anchor it a second time. Both path entries get it, so they
  stay one shape. Tests: the Win32 path is the anchored path's; a relative path anchored before the current
  directory changes opens what it named when it was anchored.

- [ ] **NR-6.4** -- **A typed read of `FileBasicInfo`.** `QueryFileBasicInfo`, shaped as
  [D-31](DESIGN-NOTES.md#d-31)'s `QueryFileIdInfo` is: one `GetFileInformationByHandleEx` call into a
  `FILE_BASIC_INFO`, the class from `FileInformationClass::BASIC`, and the five fields carried as the call wrote
  them (the four times as `i64`, the attributes as their raw `u32`), with nothing said beyond the page. Its
  decision is recorded beside D-31's. Tests: a directory reports the directory attribute and a file does not; the
  fields agree with the untyped entry's bytes; a refused handle reports the untyped entry's code.

- [ ] **NR-6.5** -- **`QueryFileInformation` into a buffer the caller owns.** A form that performs the call into
  an `AlignedBuf` the caller passes and keeps, so a consumer reading a directory batch after batch (the
  enumeration crate's refill) allocates once and not once per call. What it requires of that buffer (alignment,
  a size that fits the call's `u32`) follows the call's page, with the citation, per the workspace's
  [documented-contract rule](../../DESIGN-NOTES.md#refusals-rest-on-the-documented-contract). Tests: a directory
  read batch by batch into one buffer lists what the allocating form lists; the buffer is the same allocation
  after every call; the end of the directory is the call's code, as now.

  > **-> CROSS-COMPONENT HANDOFF:** after this crate's release, the next work is in component
  > `crates/windows-file-enumeration-sys` -> `SAFE` -> `SAFE-2`. See
  > [CHECKLIST.md](../windows-file-enumeration-sys/CHECKLIST.md).
