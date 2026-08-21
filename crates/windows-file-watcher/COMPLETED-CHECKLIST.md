# Completed checklist: windows-file-watcher

Append-only archive of completed milestones moved out of [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-19 -- M1: crate scaffold and notification decode

- [x] **M1.1** -- Scaffold `crates/windows-file-watcher`: [Cargo.toml](Cargo.toml) with a `cfg(windows)`-gated `lib`,
  path+version dependencies on [windows-overlapped-io-sys](../windows-overlapped-io-sys/README.md) and
  [windows-threadpool-sys](../windows-threadpool-sys/README.md), and `windows-sys` with the needed feature
  groups; [src/lib.rs](src/lib.rs) crate-doc skeleton; add the crate to the workspace members. Everything is
  `cfg(windows)`, so the crate resolves to an empty crate elsewhere.

- [x] **M1.2** -- Seed Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md) and Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
  from [the design session](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md) (D-1...D-20), and
  wire the crate into CI so the default and `--all-features` configurations both build, test, and lint.

- [x] **M1.3** -- `FILE_NOTIFY_INFORMATION` record-walk decoder: follow the `NextEntryOffset` chain and
  extract `Action` plus the UTF-16 `FileName` (`FileNameLength` bytes, not NUL-terminated) into a lossless
  relative-name type exposing both `OsString`/`Path` (WTF-8) and raw `&[u16]` (D-8). Malformed offsets are
  handled without out-of-bounds reads.

- [x] **M1.4** -- Change-record surface: map raw `FILE_ACTION_*` to a `ChangeKind` that keeps
  `RenamedOldName` / `RenamedNewName` distinct (D-9); a batch type; and recognition of the zero-byte
  completion as overflow -> `Desync { Overflow }` at the decode boundary (D-12).

- [x] **M1.5** -- Tests: >=10 normal decode cases plus edge cases (empty buffer, single record, multi-record
  chains, long names past `MAX_PATH` and a name filling the record to the buffer edge -- the crate imposes no
  maximum, `FileNameLength` being a `u32` bounded only by the completion buffer -- empty and unpaired-surrogate
  names, and malformed buffers: truncated/overrunning/odd-length and unaligned or out-of-range
  `NextEntryOffset`, all surfacing as `Desync`). Integration: decode a buffer produced by a real overlapped
  `ReadDirectoryChangesW` on a temp-directory mutation.

**Later addition -- D-21.** Review after this group was archived found the decoder accepted a trailing
remainder it should not have: it bounded the tail rather than requiring an exact length. Because a name is a
whole number of UTF-16 units, a record always ends on an even offset, so its DWORD alignment padding is
exactly 0 or 2 bytes -- never 1 or 3. Anything else is undescribed data and is now reported as a desync
rather than silently dropped, which would understate the batch. Fixed in M1.3/M1.5 above (a failing
regression test was written first) and recorded as `D-21` in [DESIGN-NOTES.md](DESIGN-NOTES.md); the
rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Noted here so the archive reflects that this
milestone's decoder gained a binding decision after it closed.

## Moved 2026-08-21 -- M2: the detailed single-directory watcher

The arm / complete / re-arm loop against `ReadDirectoryChangesW`, from opening the directory handle to
tearing it down. Added decisions D-22 (open-failure classification), D-23 (the arm gate is a lock held
across the submission), D-24 (the completion buffer is a heap-indirected `Box<[u32]>`), D-26 (an empty
completion is not a notification), D-34 (the gate names why it is closed; teardown is one idempotent
operation) and D-35 (how the loop is tested, and why its overflow is forced rather than raced).

- [x] **M2.1** -- Owned directory handle: `CreateFileW(FILE_LIST_DIRECTORY, FILE_SHARE_READ|WRITE|DELETE,
  OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)`; classify open errors (retryable vs
  not-found vs unsupported).

- [x] **M2.2** -- Arm and complete: issue `ReadDirectoryChangesW` through `windows-threadpool-sys`
  `ThreadpoolIo` (the overlapped seam with the generation-stamped identity, D-3/D-4); decode the completion
  into a batch (M1) and re-arm around processing to minimise the inherent loss window.

- [x] **M2.3** -- Deliver batches into a crate-owned queue endpoint (the interim, entirely in-crate delivery
  target for this milestone; the session/receiver split lands in M3, D-11) so the crate never calls into
  client code on its cadence path; tag records with a `WatchId`; emit `Desync { Overflow }` on a zero-byte
  completion.

- [x] **M2.4** -- Teardown: cancel the outstanding read, drain the pool I/O, and free the context via
  owned-object `Drop` (D-20), with re-arm suppression inherited from `ThreadpoolIo` rundown. The arm gate of
  D-23 already provides the suppression; formalise it, and note that the same not-re-arming state is what
  D-29 reuses for backpressure and D-28 for faults.

- [x] **M2.5** -- Integration: create/modify/delete/rename in a temp directory and assert raw actions and
  relative names; force a burst overflow and assert `Desync { Overflow }`; assert clean teardown with an
  operation outstanding.

**Carried forward.** M2 left one deliberate piece of scaffolding: the `unstable-internals` feature and the
`crate::unstable` module of D-35, which exist only so this milestone's integration test can reach a
`pub(crate)` loop. Its removal is queued as part of **M3.8** in [CHECKLIST.md](CHECKLIST.md), together with
the three `#![allow(dead_code)]` suppressions it stands in for. It is noted here so the archive does not
read as if M2 closed with nothing outstanding.
