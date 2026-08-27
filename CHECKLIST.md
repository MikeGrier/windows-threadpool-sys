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

> **NEXT ACTIONABLE ITEM: FE-6.** Build the deterministic model test suite for
> the two rings and the session state machine.

## M5 -- Publishable file-enumeration API and two-ring session

- [x] **FE-1** -- Scaffold and register the publishable `windows-file-enumeration-sys` workspace crate. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-1)

- [x] **FE-2** -- Close and record the remaining v1 public-contract decisions before implementing them. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-2)

- [x] **FE-3** -- Implement the public request, predicate, result, error, terminal, and `EnumerationId` types. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-3)

- [x] **FE-4** -- Implement the bounded two-ring session shell with its `Session`, submission, and receiver types. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-4)

- [x] **FE-5** -- Implement begin and cancellation admission and the affine enumeration handle. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#fe-5)

- [ ] **FE-6** -- Build a deterministic state-machine/model test suite for the two
  rings, reservations, registry, per-enumeration ordering, shared backpressure,
  affine handle drop, explicit cancellation, receiver abandonment, cancel-before-
  start servicing, cancel/refill races, terminal uniqueness, and lost-wakeup
  resistance. Cover at least ten normal interleavings plus all boundary capacities,
  including rejection of a CQ too small to retain one unreserved data slot.

## M6 -- Native enumeration engine, integration, and publication readiness

- [ ] **FE-7** -- Implement worker-side directory opening under the submitted
  `ImpersonationToken`, restoring the worker's exact prior token immediately after
  the open on every path. Query any per-directory volume identity required by the
  settled result contract, then perform caller-buffered enumeration exclusively with
  documented `GetFileInformationByHandleEx` calls using
  `FileIdExtdDirectoryRestartInfo` for the first refill and
  `FileIdExtdDirectoryInfo` thereafter. Do not use `FindFirstFileExW`,
  `FindNextFileW`, or direct `Nt*` APIs.

- [ ] **FE-8** -- Implement alignment-safe parsing of
  `FILE_ID_EXTD_DIR_INFO` chains from one fixed reusable native buffer per request.
  Validate every record boundary, next-entry offset, name length, and terminal
  condition before reading; retain the current batch and record cursor across
  callbacks; expose every metadata field promised by FE-2; and evaluate predicates
  without lossy name or timestamp conversion.

- [ ] **FE-9** -- Implement single-flight finite work quanta and lossless CQ
  backpressure. Count every examined record, including predicate rejects; enforce
  both record and cheap monotonic elapsed-time budgets; perform at most one
  synchronous refill per callback; mark refill work potentially long-running; stop
  before parsing an accepted record when no CQ data slot is available; and resume
  from consumer progress without polling, duplicate callbacks, or lost wakeups.

- [ ] **FE-10** -- Complete cancellation, exhaustion, failure, and teardown behavior
  around the native engine. Cancellation cannot preempt an executing synchronous
  refill, but once observed it discards unparsed native records, preserves already
  queued entries, and emits one ordered cancelled terminal. Receiver abandonment
  emits no terminal. Ensure no entry follows a terminal and every token, handle,
  buffer, reservation, registry entry, and work object is released exactly once
  across success, failure, cancellation, and session teardown.

- [ ] **FE-11** -- Complete crate-level API/safety documentation, README examples,
  changelog baseline, and publication validation, then add real-Windows integration
  coverage. Exercise at least ten ordinary directories plus empty, single-entry,
  thousands-of-entries, full-CQ, cancellation at each phase, receiver drop, invalid
  and inaccessible paths, long paths, native WTF-16 names, reparse points, files and
  directories, all predicate operators, metadata/file-identity fidelity, minimum
  and default buffers, multi-refill enumeration, unsupported-capability behavior,
  and the settled oversize-record path. Cross-check the observable metadata needed
  by Globazog and verify packaged contents, release automation, and
  `cargo publish --dry-run`.
