# Checklist: workspace

Workspace-level and cross-crate work. Completed groups are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). The authoritative cross-component
decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), their rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), and the originating discussion is in
[design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md](design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md).

Work items are dependency-ordered. Implement one item, test it, check it off, and
commit it before starting the next item. The implicit end-of-milestone build, test,
documentation-test, sync, and push gate is standard procedure and is not repeated
as checklist work. Conventional Commit scopes for the new crates are
`impersonation-token` and `file-enumeration`.

> **NEXT ACTIONABLE ITEM: FE-8.** Open the directory on a worker under the
> submitted token.

## M6 -- Native enumeration engine

M5's shell left a latent hazard that M6 had to remove before it could install a
worker: `leave_quantum` and `complete` let a worker mutate the registry and drop
its own thread-pool object from inside its own callback, which self-waits and
frees the executing closure. FE-7 closed that by making the worker a reporter
and the submission-ring servicer the sole registry authority (D-16, D-17).

- [x] **FE-7** -- Make the worker a reporter and the servicer the sole registry authority, and give the session something to run work on. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-7)

- [ ] **FE-8** -- Allocate the fixed native buffer and get one directory open and
  reading. Allocation happens at admission, is fallible rather than aborting, and
  produces a base address aligned to at least 8 bytes (D-19). Open the directory on
  a worker under the submitted `ImpersonationToken`, restoring the worker's exact
  prior token immediately after the open on every path including failure and
  unwind. Query the per-directory volume serial only when `FileIdentityMode` asks
  for it, failing a `Required` request before its first entry. Classify open
  failures as `DirectoryOpen` with their raw code -- `ERROR_FILE_NOT_FOUND` from
  `CreateFileW` is an open failure, never exhaustion. Perform the first
  `FileIdExtdDirectoryRestartInfo` refill and apply the phase-specific exhaustion
  rule: `ERROR_NO_MORE_FILES` from any refill, and `ERROR_FILE_NOT_FOUND` from that
  initial refill before any batch, are `Completed`. Deliver the terminal into the
  enumeration's own reserved slot and then report retirement. Test against real
  directories: empty, missing, not-a-directory, and inaccessible.

- [ ] **FE-9** -- Parse what the buffer returns and deliver entries.
  Parse `FILE_ID_EXTD_DIR_INFO`
  chains alignment-safely, validating record alignment, fixed-field extent,
  next-entry offset advance, name byte length parity, name bounds, and size sign
  before reading any field. Retain the batch and record cursor across callbacks,
  drop `.` and `..` before predicate evaluation, build entries with every field
  FE-2 promised in native units, evaluate predicates without lossy name or
  timestamp conversion, and deliver accepted entries. Use
  `FileIdExtdDirectoryInfo` for every refill after the first; never
  `FindFirstFileExW`, `FindNextFileW`, or a direct `Nt*` API.

- [ ] **FE-10** -- Bound each quantum and make backpressure lossless. Count every
  examined record against the record budget, including dot entries and predicate
  rejects, so a reject-all predicate cannot monopolise a worker; enforce a cheap
  monotonic elapsed-time budget alongside it. Perform at most one refill per
  callback and resubmit rather than refilling twice. Establish completion-ring room
  before parsing the next record, and when there is none, retain the buffer and
  cursor, park, and resume from consumer progress without polling, duplicate
  callbacks, or lost wakeups. Test multi-refill enumeration and a full ring.

- [ ] **FE-11** -- Complete the failure and capability taxonomy the contract
  settled. Classify `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED`, and
  `ERROR_INVALID_PARAMETER` as `UnsupportedExtendedDirectoryInfo` only after
  asserting the stated preconditions -- crate-opened live handle, valid information
  class, non-null 8-byte-aligned base, effective capacity at least 1 KiB that is an
  8-byte multiple and `u32`-representable -- so the crate never reports its own bug
  as a filesystem incapability. Map `ERROR_MORE_DATA`, `ERROR_INSUFFICIENT_BUFFER`,
  and `ERROR_BAD_LENGTH` before one complete record to `RecordTooLarge` carrying the
  effective capacity, parsing no bytes from a failed refill. Report malformed
  records with their detail. A late failure truncates rather than retracts: entries
  already queued stay, followed by one `Failed` terminal.

- [ ] **FE-12** -- Complete cancellation, abandonment, and teardown around the live
  engine. Cancellation cannot preempt an executing refill; once observed it discards
  unparsed native records, preserves already queued entries, and produces one
  ordered cancelled terminal. Receiver abandonment produces no terminal and must not
  wait on a directory query from the servicer. Prove that no entry follows a
  terminal and that every token, handle, buffer, reservation, registry entry, ready-
  set membership, and work object is released exactly once across success, failure,
  cancellation, abandonment, and session teardown.

## M7 -- Verification, Globazog acceptance, and publication

- [ ] **FE-13** -- Build the real-Windows integration suite. Exercise at least ten
  ordinary directories plus empty, single-entry, thousands-of-entries, full
  completion ring, cancellation at each phase, receiver drop, invalid and
  inaccessible paths, long `\?\` paths, native WTF-16 names, reparse points, files
  and directories, every predicate operator and case mode, metadata and
  file-identity fidelity, minimum and default buffers, multi-refill enumeration,
  unsupported-capability behaviour, and the settled oversize-record path.

- [ ] **FE-14** -- Discharge the D-15 Globazog acceptance gate with a real adapter
  demonstration, not a metadata cross-check. Show that Globazog's Windows
  one-directory backend can be replaced without losing native names, entry type,
  reparse status and tag, raw attributes, both sizes, all four times, or
  volume-qualified identity; that its name, type, reparse, attribute, size, and
  timestamp predicate leaves translate without loss; that a failed terminal
  reproduces its error-plus-partial-listing surface; and that no path opens an
  individual entry.

- [ ] **FE-15** -- Complete crate-level API and safety documentation, README
  examples covering ordinary and traversal-style submission, and the changelog
  baseline. Remove the M6 caveats the shell documents while its engine was missing.

- [ ] **FE-16** -- Validate publication: packaged contents, docs.rs metadata,
  release automation, sibling-dependency version ordering against crates.io, and
  `cargo publish --dry-run`.
