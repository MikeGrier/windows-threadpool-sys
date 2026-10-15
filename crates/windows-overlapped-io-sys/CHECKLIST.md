# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M14 -- Finish the contract audit against the ten specification-gap categories

[DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Specifying this contract" audits this crate against
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract) and reaches five of them: category
3 (`Issued`), category 10 (`post`/`post_raw`), categories 4/5 (`OperationId` generations, handled correctly),
and the previously-unstated completion-ordering rule. Categories 1, 2, 6, 8, and 9 were **not examined**, and
that absence is recorded as "not looked at" rather than "does not apply" -- the distinction the taxonomy
exists to keep visible.

- [ ] **M14.1** -- Audit the remaining five categories against the endpoint/association/submission surface:
  independent options that read as one concept (category 1, a strong candidate given `NotificationModes`'
  two independent flags and their asymmetric handle-versus-socket treatment); unconditional-read-as-
  probabilistic (2); which state a transition is entered from (6); values deliberately never correlated (8);
  and boundary-type fidelity at the consumer (9). State each answer, including "unspecified, deliberately".

- [ ] **M14.2** -- Sweep for the [`has_room`](../../DESIGN-NOTES.md#the-has_room-finding) shape: any advisory
  predicate this crate exposes that another subsystem's reliability gate depends on, checked for whether its
  stated contract holds under the condition its caller actually uses it in. `outstanding()` is the obvious
  candidate -- its meaning already changed once (a dequeued-but-held completion no longer counts), and
  `run_down` is a reliability gate that reads it.

## M16 -- More observer queries

M15 shipped the observer with one query, `file_len`. This milestone widens it to the other read-only
handle queries ([DESIGN-NOTES.md](DESIGN-NOTES.md) -> "observers share the handle", the bullet on what
qualifies as an observer query). Each query is behind `fs`, returns a structured result rather than one
method per field, and lands with tests of its own result. Independent of M14; it may be taken first.

- [x] **M16.1** -- `standard_info()` reads the whole `FileStandardInfo`; `file_len()` is its shortcut. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m161)

- [x] **M16.2** -- `file_id()` reads the volume serial and 128-bit file ID. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m162)

- [ ] **M16.3** -- **`file_type()`**: `GetFileType` -- disk, pipe, character, or unknown, with its error
  path (`FILE_TYPE_UNKNOWN` plus a last error). It classifies any handle, not only files, so whether it
  belongs behind `fs` or in the core is part of this item; working position (not a decision): core.

- [ ] **M16.4** -- **`storage_info()` and `alignment_info()`**: `FileStorageInfo` (logical and physical
  sector sizes) and `FileAlignmentInfo` (the buffer alignment the device requires). M16.5 builds on both.

- [ ] **M16.5** -- **The unbuffered adapters ask for their alignment instead of assuming it.** Decided by
  the engineer: [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "unbuffered I/O asks for its alignment". Today
  [`PAGE_SIZE`](src/fs.rs) (a fixed 4096) stands in for the system page size scatter/gather segments must
  be (ask the OS), the device's buffer alignment, and the sector size offsets and lengths must be multiples
  of (ask the file system, M16.4). Covers `PageBuffers` (allocate at the alignment asked for), the
  scatter/gather validation, and the public `PAGE_SIZE` constant (changing it is breaking). Open, to settle
  with the engineer when taken: what scatter/gather does when the device alignment exceeds a page, which
  page-sized segments cannot honour (working position: refuse with an error naming both values); and
  whether the answers are asked once per endpoint and kept, or per operation. No local volume requires
  more than a page, so the above-a-page path needs a seam to inject the answer, built with this item.

- [ ] **M16.6** -- **`remote_protocol_info()`**: `FileRemoteProtocolInfo`, answering whether the file is on
  SMB or another remote protocol. On a local file the query fails or reports no protocol; record which,
  measured, and test that case. A remote case needs a share, so state whether the test can reach one.

- [ ] **M16.7** -- **`basic_info()`**: `FileBasicInfo` -- creation, last access, last write and change times,
  and attributes. **Measure first** whether the times read through the handle are current while the
  directory entry lags, as the length is (the watcher's measurement); record what was observed, then expose
  it, documenting only what was measured.

- [ ] **M16.8** -- **`final_path()`**: `GetFinalPathNameByHandleW` -- the open file's current path, after any
  renames, returned as `Wtf16String` (decided: [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "observers share the
  handle", the bullet on strings). Adds the [wtf-string](../wtf-string/) dependency, with `fs`. Test a
  rename while the observer holds the handle.

- [ ] **M16.9** -- **`volume_info()`**: `GetVolumeInformationByHandleW` -- file system name (a
  `Wtf16String`), volume serial, and file system flags (sparse files and others). Test on the temp
  directory's volume, asserting only what the call reports, not which file system it is.

- [ ] **M16.10** -- **Every query against every backend, with an operation pending.** One table of the
  queries M16.1-M16.9 added, run through an observer while an operation pends on `AssociatedEndpoint`,
  `ThreadpoolIo` and `BlockingEndpoint`, as M15.5 did for `file_len`; each pending operation must still
  complete or cancel as it would without the queries.
