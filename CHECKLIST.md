# Checklist: workspace

Workspace-level and cross-crate work. Per-crate checklists are listed in
[PLANS.md](PLANS.md); completed groups are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). The authoritative cross-component
decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) and their rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The originating discussion for the
M1-M7 work archived below is in
[design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md](design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md).

The decisions the pending items below implement are in
[DESIGN-NOTES.md](DESIGN-NOTES.md#remoting-synchronous-namespace-operations); the session that produced
them, with the full measurement transcripts and the rejected alternatives, is
[design-sessions/DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md](design-sessions/DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md).

## M19 -- Propagate the 2026-08-27 platform measurements

The design session measured nine platform behaviours, several of which contradict what shipped code
currently assumes or what shipped documentation currently says. These items propagate those findings;
they are deliberately separate from building the new facility, which cannot start until they land.

- [ ] **M19.1** -- Discharge `windows-ioring-sys` D-14's unverified registration-index continuity, which
  [its M10.3](crates/windows-ioring-sys/CHECKLIST.md) records as needing either measurement or a plain
  statement that continuity is not guaranteed. It is now measured: a second
  `BuildIoRingRegisterFileHandles` **replaces** the whole table, re-basing indices at zero (a read of index
  0 returned the second batch's first file, and the old count's index reported `ERROR_INVALID_INDEX`);
  capacity reached 65536 handles; and an in-flight read against an old index completed with its full byte
  count after the table was replaced beneath it, so **indices are resolved at submission**. Rewrite D-14
  from an assumption into a measured statement, and record the replace semantics on the public API.

- [ ] **M19.2** -- Relax `Batch::register_files`'s one-registration-per-ring rule, which is now measurably
  stronger than the hazard requires. It refuses a second registration outright to prevent silently
  invalidating handed-out `RegisteredFile` indices; M19.1 shows repeated registration is supported and does
  not disturb in-flight operations. Carry a table generation on the ring and on every `RegisteredFile`,
  validate it where `RingId` is already validated, and permit re-registration. Keeps the safety property
  while removing a restriction that would make a long-lived domain unable to add files. Depends on M19.1.

- [ ] **M19.3** -- State the completion-port/`IoRing` fork on both crates' public surfaces. Associating a
  handle with a completion port -- including via `CreateThreadpoolIo` -- permanently prevents `IoRing` use
  of that handle (`ERROR_INVALID_PARAMETER`), while leaving it fully usable through the port. Document it
  on `UnassociatedEndpoint`'s association transition, on `ThreadpoolIo::new`, and on `Batch`'s file-taking
  pushes, including the derived trap: a ring-destined handle is not port-associated, so an ordinary
  overlapped operation issued on it from a transient worker is thread-bound and dies with that worker.

- [ ] **M19.4** -- Correct the thread-pool growth documentation, which currently describes
  `set_runs_long` as an accounting hint. Measured, it is the difference between reaching 16 concurrent
  blocked callbacks in 1.94 s and in 1 ms: four threads are created immediately and growth beyond that is
  throttled to roughly one thread per 166 ms without it. Also record the measured default maximum of 512,
  which `set_max_threads`' documentation deliberately declines to guess at, and note that both the free
  count and the injection interval are likely processor-count-dependent and were measured only on ARM64.

- [ ] **M19.5** -- Re-measure M19.4's two numeric findings on x64 and record whichever of them is
  architecture-dependent. The semantic results from the session (the fork, thread agnosticism, token
  inheritance, device-map behaviour, `CancelSynchronousIo`'s blocking rule) do not need this; the free
  thread count, the 166 ms injection interval, and the 512 default do. Depends on M19.4.

## M20 -- Decide the session-independent path form

- [ ] **M20.1** -- Decide what the namespace facility does with a session-relative drive letter, and record
  it as a decision rather than leaving the absence of one implicit. Path resolution follows the
  impersonated token's logon session (measured: under a token from another logon session with unchanged
  local access, the global `C:` resolved and a `subst` letter did not), and `GetFullPathNameW` is lexical
  so submission-time canonicalisation does not expand the letter. `QueryDosDeviceW` distinguishes a real
  local volume, a `subst`, and a network mapping cheaply, so detection is settled and only the response is
  open: expand to a session-independent form at submission, or reject at admission with a typed error.
  Expansion is not uniform -- a network mapping becomes a UNC path, a local volume becomes a device path
  needing `\\?\GLOBALROOT\`, and a `subst` becomes another path entirely -- which is the reason this is a
  decision rather than an implementation detail.

## M21 -- Reconcile with the shipped namespace-plane work

PR #44 landed `windows-impersonation-token-sys` and `windows-file-enumeration-sys` while this design
session was in progress. Both are inhabitants of the plane the session scoped, so the relationship has to
be settled rather than discovered later.

- [ ] **M21.1** -- Sweep the workspace design notes for prose that says the impersonation and enumeration
  crates *will be* added, now that both have shipped. The "Captured impersonation is a separate platform
  layer" section still opens "The workspace will add ...". This is the blast-radius half of a correction:
  the fact changed, and the statements of it have to be swept rather than the one site a reader happened
  to notice.

- [ ] **M21.2** -- Settle the two open sub-questions in the context decomposition: whether the caller's
  thread error mode is captured at all (for diagnostics) or not captured as dead weight, given that the
  facility overrides rather than transplants it; and whether the non-dialog error-mode bits
  (`SEM_NOALIGNMENTFAULTEXCEPT`, `SEM_NOGPFAULTERRORBOX`) are transplantable while the dialog-suppressing
  bits stay forced. Note `SEM_NOALIGNMENTFAULTEXCEPT`'s behaviour is architecture-dependent, so this needs
  measurement on both ARM64 and x64 rather than reasoning.

- [ ] **M21.3** -- Replace `windows-file-enumeration-sys`'s inline `open_directory` with the general
  facility's catalogue operation. The direction is settled, not open: that open is the committed first
  consumer, and the inline path exists only until the general one is proven. The replacement must preserve
  everything the shipped open already gets right, each of which is a constraint on the catalogue rather
  than an implementation detail: arbitrary access, share mode and flags (it opens with
  `FILE_LIST_DIRECTORY`, not `GENERIC_READ`, and requires `FILE_FLAG_BACKUP_SEMANTICS` to obtain a
  directory handle at all); an **unassociated** handle, since `GetFileInformationByHandleEx` is
  synchronous; the raw Win32 code unaltered, because `ERROR_FILE_NOT_FOUND` means three different things
  across the open and the first and later queries and only the consumer can disambiguate; and
  `GetLastError` captured *before* the context is restored, which the shipped code does deliberately.
  Its D-15 Globazog acceptance gate must still pass afterwards.

- [ ] **M21.4** -- Add the `FileBasicInfo` query as its own catalogue entry, and have the enumeration
  crate sequence it after the open rather than performing it inline. It is a blocking namespace call, so
  leaving it inline would keep a blocking call on the consumer's worker and only half-solve the problem
  this facility exists for. One entry per Win32 call: this is **not** a compound open-and-classify
  operation, and the sequencing is logical and client-side -- the crate submits the open, observes its
  completion, then submits the query. A compound entry is reserved for a measured performance argument
  and would be a fusion of these two entries rather than a capability they lack. Depends on M21.3.

## M34 -- Tooling

Numbered M34 rather than M22 because the three root-level checklists share one milestone space:
[CHECKLIST.md](CHECKLIST.md) holds M19-M21, [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md)
M22-M29, and [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M30-M33.

- [x] **M34.1** -- Promote the ad-hoc sabotage harness into a reusable tool. -> [completed 2026-08-31](COMPLETED-CHECKLIST.md#m341)

- [ ] **M34.3** -- **Archive the completed bodies in
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md)**, which holds twelve checked items still
  carrying their full write-ups. The completed-item rule moves a large one to
  [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) immediately and leaves a one-line anchored stub, so
  the active file stays a list of what is *left*. Raised in review 5072735803 on pull request #56,
  where it was noted that the problem recurs throughout that file rather than at the one line cited.
  [M34.1](COMPLETED-CHECKLIST.md#m341) is the worked example of the shape: `### <a id="..."></a>` in
  the archive under a dated group, a stub with a completion link in its place.
  Bookkeeping with no bearing on correctness, which is why it is queued rather than folded into a
  branch already under review -- but it is 757 lines of checklist that a reader currently has to scan
  past to find the open work, so it is not cosmetic either.

- [ ] **M34.2** -- **Route every tool's output through one sink, per the repository's own rule**: never
  call `println!`/`eprintln!` from more than one site in a tool; introduce a writer trait, sink or
  formatter at the first occurrence and route everything through it.
  **Updated 2026-09-04: the conversion is done; what remains is the capture test.** The item said
  "seven binaries violate this today" and named them, from review 5072622803 on pull request #56.
  Five had already been converted when a later review round re-checked, and the last two --
  [queue_contention.rs](crates/windows-platform-probes/src/bin/queue_contention.rs) and
  [peer_index_cache.rs](crates/windows-platform-probes/src/bin/peer_index_cache.rs) -- were fixed in
  that pull request, so all seven now compose their whole report as text and hand it to a sink at one
  place. Verified by counting, not by reading: no probe binary contains a direct `println!`/
  `eprintln!` at all. The two survivors were the *banner*, which those two rendered by calling a
  helper that wrote to stdout itself -- so a captured report was missing the one line naming the
  machine that produced it, and the banner also emitted mid-`render`, ahead of the body, making the
  order on a terminal luck rather than construction.
  **The stdout-writing banner helpers are gone rather than documented against.** `print_banner` and
  `print_banner_with` were removed and `banner_lines_with` returns the string instead, because three
  call sites had each grown a comment warning about them -- a rule restated three times instead of a
  hazard removed once. The defect class is now unreachable by construction: there is no
  banner helper that writes to a stream.
  **The PowerShell tools are NOT part of this item, because they are already done.** A later review
  round on the same pull request observed that the inventory above named only Rust binaries while five
  scripts emitted from many sites, so those were converted in that pull request rather than queued
  here: [inject-mutant.ps1](tools/inject-mutant.ps1),
  [check-publishable.ps1](tools/check-publishable.ps1),
  [run-numa-spikes.ps1](tools/run-numa-spikes.ps1), [run-mutants.ps1](tools/run-mutants.ps1) and
  [run-sabotage.ps1](tools/run-sabotage.ps1) each now route everything through one `Write-Report`
  sink. They were small enough to convert in place, which is exactly why they did not need deferring.
  (`run-sabotage.ps1`'s `Exit-WithMessage` is deliberately outside its sink: that path writes to
  stderr and exits, and there the destination is part of the meaning.)
  **What remains is the capture test, and it has a structural obstacle worth naming.**
  The point of the rule is that output becomes testable, so this item is not checked off on the
  refactor alone -- an abstraction introduced without a capture-based test spends the cost and skips
  the benefit. `Captured` exists in [report.rs](crates/windows-platform-probes/src/report.rs) for
  exactly that purpose, and `banner_line` is already asserted directly.
  **The obstacle: each probe's `render()` lives in its own `bin` target, which nothing can import.**
  That is precisely why the two banner defects survived every test -- there was no reachable seam to
  assert against. Closing it means moving each `render()` into the crate's library and leaving `main`
  as the one place that names the stream, which is a real refactor rather than a test to write.
  Decide the seam once and apply it uniformly.
  A PowerShell sink is a function whose destination can be swapped, but this workspace runs no
  PowerShell test harness in which to assert against it, and inventing one to cover five diagnostic
  scripts is not a cost this item is willing to spend without deciding to adopt such a harness first.
  Start with `placement_probe`: its output is a published artifact that strangers paste into a
  discussion thread, so "can this be captured and asserted end to end?" has real value there rather
  than being architectural tidiness.

## M35 -- Measure what the long-path opt-in actually does

- [x] **M35.1** -- **Measure whether the long-path opt-in lifts `MAX_PATH` for a *relative* path, and
  whether it does so without changing how the path is parsed.**
  **Done 2026-09-04, and it settles a question that had produced three wrong answers from reading.**
  `probe-long-path-aware` and `probe-long-path-unaware` in
  [windows-platform-probes](crates/windows-platform-probes/src/long_path.rs) are the same code
  differing only in whether their manifest declares `longPathAware`; `build.rs` embeds it into that
  one binary via `rustc-link-arg-bin`, so the other thirteen probes are unaffected.
  **Result, on a host with `LongPathsEnabled=1`.** With the opt-in, a relative path of 429 characters
  opens in every shape -- plain, containing `b\..`, and forward-slash separated. Without it, all three
  are refused with `ERROR_PATH_NOT_FOUND` while the same shapes at 78 characters open. The targets are
  created first, so a not-found from a file that provably exists is the length refusal.
  **So the documented reading was right and the review finding was wrong**: the opt-in covers relative
  paths, and `MAX_PATH` binds them only in a process that has not opted in.
  **And the regularize-then-prefix hypothesis is falsified.** If the opt-in worked by prepending
  `\\?\`, that prefix would disable `.`, `..` and forward-slash translation, so those shapes would
  have failed past the ceiling while working below it. Both resolve at both lengths. The opt-in lifts
  the length check without re-parsing, so there is no discontinuity at `MAX_PATH` for a caller of
  `windows-file-watcher` to fall into.
  The measurement is recorded where the claim lives, in `Session::subscribe`'s note.
## M-inf -- Parked

Ungated work with no identified predecessor deliverable.

- [ ] **M-inf.1** -- Root-cause the process death when impersonating the UAC-linked token. The device-map
  probe reached a marker immediately before `ImpersonateLoggedOnUser` on a token obtained via
  `TokenLinkedToken` and never the marker immediately after, with no panic message. It was removed from the
  probe because a `LOGON32_LOGON_NEW_CREDENTIALS` token answered the question with a passing control, so
  the fallback was redundant -- not because the crash was understood. Parked rather than dropped so the
  unexplained result is not mistaken for a tested one.
