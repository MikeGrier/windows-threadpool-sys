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

> **NEXT ACTIONABLE ITEM: IT-5.** Finish M4 before beginning any
> `windows-file-enumeration-sys` implementation.

## M4 -- Publishable captured-impersonation platform layer

- [x] **IT-1** -- Scaffold and register the publishable `windows-impersonation-token-sys` workspace crate. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#it-1)

- [x] **IT-2** -- Implement the opaque, owned, clonable `ImpersonationToken` capture type. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#it-2)

- [x] **IT-3** -- Implement scoped application of an `ImpersonationToken` with exact prior-token restoration. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#it-3)

- [x] **IT-4** -- Add deterministic capture, application, restoration, and failure-path tests. -> [completed 2026-08-27](COMPLETED-CHECKLIST.md#it-4)

- [ ] **IT-5** -- Complete the crate-level API documentation, safety/invariant
  documentation, README examples, changelog baseline, and publication validation.
  Verify packaged contents, Windows docs.rs configuration, dependency versions,
  release-please recognition, and `cargo publish --dry-run` through the repository's
  Cargo tooling before declaring the reusable layer ready.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component
  > `crates/windows-file-enumeration-sys` -> M5 -> **FE-1** (publishable enumeration
  > crate scaffold). See [CHECKLIST.md](CHECKLIST.md).

## M5 -- Publishable file-enumeration API and two-ring session

- [ ] **FE-1** -- Scaffold `crates/windows-file-enumeration-sys` as a publishable
  Windows-only workspace crate with the same complete crates.io, docs.rs,
  release-please, publish-workflow, local design/plans, changelog, README, and
  copyright setup required by IT-1. Declare path-plus-version dependencies on
  `windows-impersonation-token-sys`, `windows-threadpool-sys`, and
  `wtf-string`, and select only the `windows-sys` features used by directory
  enumeration and its doorbells.

  > **CROSS-COMPONENT PREREQUISITE:** component
  > `crates/windows-impersonation-token-sys` -> M4 -> **IT-5** must be
  > complete first. See [CHECKLIST.md](CHECKLIST.md).

- [ ] **FE-2** -- Close and record the remaining v1 public-contract decisions before
  implementing them: path and long-path inputs; native ordering; exact result and
  terminal/error taxonomy; always-present versus selected metadata; native timestamp
  representation; name matching and the extensible query-by-example predicate;
  attribute all-set/all-clear validation; comparison/range operators; behavior when
  `FileIdExtdDirectoryInfo` is unsupported; and behavior when one record exceeds the
  configured buffer. Update Tier 1 and Tier 2 together and keep Globazog replacement
  as a mandatory acceptance criterion.

- [ ] **FE-3** -- Implement the public request, predicate, result, error, terminal,
  `EnumerationId`, session, submission, receiver, and affine enumeration-handle
  types. Preserve native Microsoft value types where they express the contract,
  retain names and paths as native-width WTF-16, make the predicate extensible
  without replacing the request API, default the native buffer to 64 KiB, and clamp
  requested capacities below 1 KiB.

- [ ] **FE-4** -- Implement the bounded multi-producer SQ and single-receiver CQ
  session shell. Every begin and control operation enters through the SQ; every
  entry and terminal carries its `EnumerationId`. Use one coalesced
  `ThreadpoolWork` SQ doorbell and one logical FIFO drain authority. Give the CQ a
  lazily created manual-reset event whose signaled state exactly matches observable
  receiver work. The SQ servicer mutates the registry and schedules per-enumeration
  work but never performs directory refills itself.

- [ ] **FE-5** -- Implement begin and cancellation admission. The ordinary
  `try_begin` helper captures the caller's current `ImpersonationToken` before the SQ
  entry becomes visible; an explicit-token form lets traversal capture once and
  reuse that context for child enumerations. Accepted requests reserve an
  exactly-once CQ terminal slot and an infallible future SQ cancellation slot;
  ordinary full-SQ submission is rejected synchronously without acceptance. Reserve
  a session-level abandon message so receiver drop rejects future starts and
  asynchronously cancels all attached enumerations without blocking.

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
