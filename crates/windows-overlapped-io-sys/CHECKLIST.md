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

- [ ] **M16.1** -- **`standard_info()`**: the whole `FileStandardInfo` -- length, allocation size, link
  count, `DeletePending`, directory. `file_len()` stays, as a shortcut that calls `standard_info()` rather
  than a second copy of the query. Test `DeletePending` by deleting the file while the observer holds it.

- [ ] **M16.2** -- **`file_id()`**: `FileIdInfo` -- the 64-bit volume serial and the 128-bit file ID. Test
  that two observers of one handle agree, that another file differs, and that a rename keeps the ID.

- [ ] **M16.3** -- **`file_type()`**: `GetFileType` -- disk, pipe, character, or unknown, with its error
  path (`FILE_TYPE_UNKNOWN` plus a last error). It classifies any handle, not only files, so whether it
  belongs behind `fs` or in the core is part of this item; working position (not a decision): core.

- [ ] **M16.4** -- **`storage_info()` and `alignment_info()`**: `FileStorageInfo` (logical and physical
  sector sizes) and `FileAlignmentInfo` (the buffer alignment unbuffered I/O requires). The unbuffered adapters
  currently assume [`PAGE_SIZE`](src/fs.rs) rather than asking; whether they should consult this is a
  question for the engineer, raised with this item, not scheduled by it.

- [ ] **M16.5** -- **`remote_protocol_info()`**: `FileRemoteProtocolInfo`, answering whether the file is on
  SMB or another remote protocol. On a local file the query fails or reports no protocol; record which,
  measured, and test that case. A remote case needs a share, so state whether the test can reach one.

- [ ] **M16.6** -- **`basic_info()`**: `FileBasicInfo` -- creation, last access, last write and change times,
  and attributes. **Measure first** whether the times read through the handle are current while the
  directory entry lags, as the length is (the watcher's measurement); record what was observed, then expose
  it, documenting only what was measured.

- [ ] **M16.7** -- **`final_path()`**: `GetFinalPathNameByHandleW` -- the open file's current path, after any
  renames. Needs a decision on the return type: `OsString`, or this crate taking a dependency on
  [wtf-string](../wtf-string/) for `Wtf16String`. Raise it with the engineer before building; M16.8 takes
  the same type. Test a rename while the observer holds the handle.

- [ ] **M16.8** -- **`volume_info()`**: `GetVolumeInformationByHandleW` -- file system name, volume serial,
  and file system flags (sparse files and others). The name uses M16.7's string type. Test on the temp
  directory's volume, asserting only what the call reports, not which file system it is.

- [ ] **M16.9** -- **Every query against every backend, with an operation pending.** One table of the queries
  M16.1-M16.8 added, run through an observer while an operation pends on `AssociatedEndpoint`,
  `ThreadpoolIo` and `BlockingEndpoint`, as M15.5 did for `file_len`; each pending operation must still
  complete or cancel as it would without the queries.
