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
  local access, the global `C:` resolved and a `subst` letter did not), and `GetFullPathNameW` never
  expands a drive letter, so submission-time canonicalisation does not expand it either. `QueryDosDeviceW` distinguishes a real
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

Numbered M34 rather than M22 because three of the root-level checklists share one milestone space:
[CHECKLIST.md](CHECKLIST.md) opened M19-M21, [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md)
took M22-M29, and [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M30-M33. The feature-scoped
checklists -- [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md),
[CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md) and
[CHECKLIST-mutation-survivors.md](CHECKLIST-mutation-survivors.md) -- number from M1 independently and
are not part of that space. M30 is currently used twice inside it, by this file and by io-domains;
M34.4 owns that.
- [x] **M34.1** -- Promote the ad-hoc sabotage harness into a reusable tool. -> [completed 2026-08-31](COMPLETED-CHECKLIST.md#m341)

- [ ] **M34.3** -- **Archive the completed bodies in the three root checklists that still carry
  them**: [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md),
  [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) and
  [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md). Not one of their
  checked items is a stub. The completed-item rule moves a large one to
  [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) immediately and leaves a one-line anchored stub, so
  the active file stays a list of what is *left*. Raised in review 5072735803 on pull request #56,
  where it was noted that the problem recurs throughout that file rather than at the one line cited.
  [M34.1](COMPLETED-CHECKLIST.md#m341) is the worked example of the shape: `### <a id="..."></a>` in
  the archive under a dated group, a stub with a completion link in its place.
  Bookkeeping with no bearing on correctness, which is why it is queued rather than folded into a
  branch already under review -- but a reader currently has to scan past every completed write-up to
  reach the open work, so it is not cosmetic either.

- [ ] **M34.2** -- **Route every tool's output through one sink, per the repository's own rule**: never
  call `println!`/`eprintln!` from more than one site in a tool; introduce a writer trait, sink or
  formatter at the first occurrence and route everything through it.
  **Updated 2026-09-04 (second pass): every probe now leads its report with the host line.** The
  banner had reached only the seven binaries this item named plus the shared long-path renderer,
  which left **eight** probes -- `cancel_io`, `completion_port`, `device_map`, `error_mode`,
  `handle_state`, `ioring`, `pool_growth`, `worker_context` -- composing a report that named no
  machine. `pool_growth` was the sharpest case: it printed "every number here is from this host and
  this Windows build" while giving a reader no way to tell which host that was. The rest are
  behavioural findings about what *this* Windows does, which is equally uninterpretable unattributed.
  All eight now emit `banner_line()` as the first line of the returned text, verified by running each
  binary rather than by reading the diff. Raised by Copilot at review `5118237348`.
  **Updated 2026-09-04: the conversion is done; what remains is the capture test.** The item said
  "seven binaries violate this today" and named them, from review 5072622803 on pull request #56.
  Five had already been converted when a later review round re-checked, and the last two --
  [queue_contention/main.rs](crates/windows-platform-probes/src/bin/queue_contention/main.rs) and
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

- [x] **M34.4** -- Share the native-command guard through a dot-sourced `tools/common.ps1`, route
  every capture site through it, and prove it on both PowerShell hosts.
  -> [completed 2026-09-07](COMPLETED-CHECKLIST.md#m344)

- [ ] **M34.5** -- **Validate that every workflow file is well-formed YAML**, which nothing currently
  does. [check-workflow-refs.ps1](tools/check-workflow-refs.ps1) checks that 62 *references* resolve
  across 5 files, by regex; it does not parse the document, so a file GitHub Actions would reject
  outright passes it.

  **Measured, not supposed.** A conflict resolution in merge `1abcaaf` welded a step's `if:` and
  `run:` onto one line in [ci.yml](.github/workflows/ci.yml) -- `if: '!cancelled()'        run: cargo
  run ...` -- which is not valid YAML. It survived the merge, survived the workflow gate (re-run
  against the damaged file: exit 0, same 62 references), and would have been caught only by pushing
  and watching Actions refuse the workflow. It was found by eye, three commits later, while editing
  the same step for an unrelated reason.

  The gap is the gate's shape rather than a bug in it: a regex over lines cannot notice that two keys
  share one. The fix wants a real parser, and the choice is a decision rather than a detail --
  `actionlint` validates workflow *semantics* (expression syntax, context availability, `needs`
  graphs) and not merely YAML, but is another CI dependency; `js-yaml` or a PowerShell YAML module
  parses the document and nothing more. Prefer `actionlint`: the same merge could equally have
  produced a syntactically valid file with a broken `if:` expression, which a YAML parser would pass.

  Whatever is chosen must be verified by **re-injecting this exact weld** and confirming the gate
  goes red, since the point of the item is that the current one does not.

- [ ] **M34.4** -- **Resolve the `M30` collision inside the shared milestone space.** This file's
  `M30` (machine-checkable correctness, M30.1-M30.5 open) and
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md)'s archived `M30` (the queue crate's name,
  skeleton and SPSC shape) are different work under one number, and their sub-items collide too:
  this file defines `M30.1`-`M30.5`, while io-domains refers to an `M30.2`-`M30.5` of its own.
  The collision was created when io-domains arrived
  alongside a file that already held `M30` on `main`, and it is invisible from either file alone.
  Renumbering this file's `M30` is the cheaper side, since io-domains' is already archived and
  cited from [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md); but the choice is the engineer's,
  because the number is referenced from the M37 preamble's "first free number" argument and from
  [PLANS.md](PLANS.md). Decide, then sweep every reference to whichever `M30` moves.

## M37 -- Discharge the failable-call standard across the workspace

Numbered M37, not M22. This section arrived from PR #84, which numbered it M22 without knowing
that the root checklists share one milestone space and that
[CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md) already holds M22-M29. The collision
was invisible on `main` -- where this file's highest number is low and the sibling was not being
edited -- and only surfaced when this branch, which carries M34 and M35, merged it. M36 belongs to
[CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md), so M37 is the first free number.
The standard is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md#a-failable-call-has-its-failure-handled-always): a call that
can fail has its failure handled, with no per-site analysis. These items apply it to code
written before it was stated.

- [ ] **M37.1** -- Audit every bare `unsafe { Call(...) };` statement in the workspace and handle
  the failure of each one that is failable.

  **Scope, re-measured against main at `dc2b463` (2026-09-09, after PR #83 merged):** 119
  single-line `unsafe`-block statement discards across 11 crates -- `windows-threadpool-sys` 57,
  `windows-platform-probes` 23, `windows-file-watcher` 9, `windows-ioring-sys` 9,
  `windows-overlapped-io-sys` 5, `windows-thread-ambient-sys` 5, `windows-namespace-request-sys` 4,
  `windows-guard-alloc` 3, `windows-impersonation-token-sys` 2, `windows-file-enumeration-sys` 1,
  `windows-placement-probe` 1.

  **Most are not violations.** `SubmitThreadpoolWork`, `SetThreadpoolWait`, `GetSystemInfo`,
  `SetLastError` and the `WaitForThreadpool*Callbacks` family return `void`, and `SetErrorMode`
  returns the previous mode rather than a status. Discarding those is correct.

  The failable set is **30 sites**, each confirmed against its `windows-sys` signature rather than
  assumed:

  | call | returns | discarded sites |
  |---|---|---|
  | `CloseHandle` | `BOOL` | 24 |
  | `SetEvent` | `BOOL` | 2 |
  | `CancelIoEx` | `BOOL` | 1 |
  | `RevertToSelf` | `BOOL` | 1 |
  | `SetCurrentDirectoryW` | `BOOL` | 1 |
  | `CloseIoRing` | `HRESULT` | 1 |

  **Treat every number here as stale on arrival and re-measure.** These figures moved between two
  measurements a few hours apart (121 -> 119 raw, and `SetEvent` 4 -> 2) purely because PR #83
  landed in between. Re-run the lint below rather than trusting the table; it is a description of
  the shape of the work, not an inventory to tick off.

  **Use the compiler, not a regex: `RUSTFLAGS="-W unused_results"`.** `unused_results` is a
  rustc lint, allow-by-default, that fires on any expression statement discarding a non-unit
  value -- which is exactly this rule's shape, and it does not care how many lines the statement
  spans or whether the callee is `unsafe`.

  (Either spelling works: rustc normalises `_` and `-` in lint names on the command line, and then
  echoes the hyphenated form back -- a run of `-W unused_results` reports "requested on the command
  line with `-W unused-results`". Verified on 1.98.0; noted only because that echo reads like a
  correction and is not one.) Measured on `windows-platform-probes`: it flagged
  every raw Win32 discard the regex found, plus four in `doorbell_cost.rs` the regex had counted
  but nobody had looked at, in a file already believed fixed. A per-crate total is not a list.

  It is too noisy to deny workspace-wide, which is why it is an audit tool rather than a CI gate.
  Measured on `windows-platform-probes` at `dc2b463`: **104 warnings, of which 62 are ordinary
  Rust** -- `HashMap::insert`, `HashSet::remove`, `Vec::pop`, `fetch_add`, `black_box` -- and 42
  name a raw Win32 call. Not even those 42 are all violations, since several of the calls return
  `void`. Triage is required at every step, and `SetErrorMode`'s previous mode and `fetch_add`'s
  prior value are the standing examples of a discarded return that is not discarded failure
  information.

  Do not reach for `#[must_use]` here: it cannot be applied to `windows-sys`'s `extern` block, so
  it enforces nothing at the sites that matter. It becomes available only after M37.2, on our own
  wrappers -- which is the durable end state, because a `#[must_use]` wrapper gives permanent
  enforcement with none of the lint's noise.

- [ ] **M37.2** -- Introduce a checked owning handle type and route the `CloseHandle` sites through
  it, so the rule is discharged by construction rather than by 24 written-out checks.

  This is the type-embedding half of the decision, and `CloseHandle` is its clearest case: one
  `Drop` that checks once removes every visible check at every use site and cannot be forgotten at
  a new one. Note that `std`'s `OwnedHandle` is not a discharge on its own -- it closes on drop but
  discards the `BOOL`.

  Settle two questions while doing it, because both determine whether the type is usable at all.
  What a failing close should do in `Drop`, given that panicking in a drop during unwind aborts --
  the honest options are abort, a debug assertion, or a recorded counter, and they are not
  equivalent. And whether teardown paths that legitimately expect a close to fail exist in this
  workspace; `windows-threadpool-sys` owns wait targets whose close routine is a caller-supplied
  function pointer, which is exactly where such a path would be. Depends on M37.1's
  classification.

## M30 -- Find out how much of this workspace's algorithm correctness can be machine-checked

Rationale: [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#machine-checking-what-is-argued).

- [ ] **M30.1** -- Survey the workspace for algorithms whose correctness is currently argued rather
  than checked, and match each to the class of tool that could check it.

  Candidates, not exhaustive: `reserving_mpsc`'s packed claim word and its reservation admission rule;
  `slotwise_mpsc`'s per-slot sequence protocol; the experimental permit claim; the doorbell's mirror
  flag against `SetEvent`/`ResetEvent`; `windows-file-watcher`'s contract state machine;
  `windows-ioring-sys`' submission/completion ring.

  Tool classes to match against: TLA+/PlusCal, loom, a bounded proof such as kani, and `const`
  assertions. What each can and cannot see is in the rationale.

  Done when: a table exists with one row per algorithm, each naming a tool class or "none fits" and
  why. The "none fits" rows count as output, not as gaps in the survey.

- [ ] **M30.2** -- Pilot exactly one: `reserving_mpsc`'s claim-position recurrence (`SH-14.1`).

  **`SH-14.1` is a live defect in the shipping protocol, not a hypothetical**, and that inverts the
  usual shape of a success criterion. A faithful model of the shipping claim protocol, at a position
  width small enough to wrap, *must* find it. A green run on that model is therefore evidence the
  model is **unfaithful**, not evidence the protocol is sound.

  Done when all three hold:

  1. **The refinement is stated.** Which constants were shrunk, why the protocol is uniform in each,
     and what a counterexample at the reduced width implies at the shipping width. If it cannot be
     argued, the result is reported as a counterexample *in a toy model* and nothing more.
  2. **Faithfulness: the model of the shipping protocol reproduces `SH-14.1`.** Configured so the
     wrap is reachable and a producer can stall across it, it must yield the known counterexample --
     a claim succeeding against a numerically identical but generations-later position. A model that
     cannot produce a defect the crate already documents is not modelling this protocol.
  3. **Anti-vacuity: the same model satisfies its invariant where no violation is possible.**
     Configured so the wrap is unreachable -- total pushes bounded below it, or a single producer,
     which has no race to lose -- it must come back green. A model that reports a violation there is
     over-permissive, and its counterexample in (2) proved nothing.

  Not a candidate: `capacity == 1`, which belongs to `slotwise_mpsc`
  ([D-12](crates/windows-waitable-queues/DESIGN-NOTES.md#d-12)) and is already resolved.

- [ ] **M30.3** -- Write down what the pilot could NOT reach, by name. This is the item the milestone
  exists for.

  Done when both hold:

  1. **The list names only what the *selected* tool cannot model**, not what tools in general cannot.
     Expect the syscall boundary -- including the doorbell's `SetEvent`/`ResetEvent` -- real-time
     behaviour, and the model-to-code
     gap that no tool closes. Do **not** preclassify scheduler-dependence: loom explores scheduler
     interleavings deliberately, so recording it as unreachable would be a false gap. Whether the
     memory orderings are reachable likewise depends on which tool M30.1 selected.
  2. **Both copies of the disclosure are updated** -- the "How far the memory orderings are verified,
     and how far they are not" section exists in
     [that crate's README.md](crates/windows-waitable-queues/README.md) and
     [its src/lib.rs](crates/windows-waitable-queues/src/lib.rs). Deriving one from the other is not
     available today: the `include_str!` there is `#[cfg(all(doctest, windows))]` on a private item,
     so it compiles the README's code as doctests and does not render its prose. Making one derive
     would mean a shared fragment both include, which is separate work.

- [ ] **M30.4** -- Re-home `M31.6`, which is currently orphaned.

  Done when all three hold:

  1. **It has a live item in a live checklist**, scoped as
     [D-29](crates/windows-waitable-queues/DESIGN-NOTES.md#d-29) requires -- the loom verification
     covers both MPSC shapes or neither is verified. (D-29 owns that obligation;
     [D-31](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31) owns only the release timing, that
     verification gates 1.0 rather than 0.1.0.)
  2. **All five references point at it.** Three in
     [that crate's DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md), one in
     [src/doorbell.rs](crates/windows-waitable-queues/src/doorbell.rs), one in
     [sabotage.json](crates/windows-waitable-queues/sabotage.json).
  3. **The component tracker row stays accurate** as the work proceeds. The row itself was added
     when the work was queued, not deferred to this item.

- [ ] **M30.5** -- Decide what, if anything, the workspace adopts, and record the decision with its
  cost.

  Done when all three hold:

  1. **The cost is stated, not implied.** What running it costs in wall-clock and in whose time,
     what tooling it adds and who maintains that, and the standing cost the workspace already knows
     it pays here -- a specification is a second statement of the contract and can drift from the
     code with nothing to detect it, so a stale model that still passes is worse than no model. An
     adoption recorded without its cost is the half of this item that is easiest to skip and the
     half the milestone exists to surface.
  2. **The decision answers three questions**: what keeps the model and the code in step, who
     re-runs it, and what happens when they disagree. "Adopt nothing, and say why" is a legitimate
     outcome.
  3. **The contract sweep is done, whichever way it went.** Three public places in
     `windows-waitable-queues` promise machine-checked verification before 1.0:
     [that crate's README.md](crates/windows-waitable-queues/README.md),
     [its src/lib.rs](crates/windows-waitable-queues/src/lib.rs), and
     [D-31](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31). A no-adoption outcome obliges this
     most: left unswept it would recreate, in the same crate, the orphaned commitment M30.4 exists
     to fix.

## M-inf -- Parked

Ungated work with no identified predecessor deliverable.

- [ ] **M-inf.1** -- Root-cause the process death when impersonating the UAC-linked token. The device-map
  probe reached a marker immediately before `ImpersonateLoggedOnUser` on a token obtained via
  `TokenLinkedToken` and never the marker immediately after, with no panic message. It was removed from the
  probe because a `LOGON32_LOGON_NEW_CREDENTIALS` token answered the question with a passing control, so
  the fallback was redundant -- not because the crash was understood. Parked rather than dropped so the
  unexplained result is not mistaken for a tested one.

- [x] **M-inf.2** -- Archived the eight completed milestone groups in [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md), leaving only the parked `M26+`. -> [completed 2026-09-17](COMPLETED-CHECKLIST.md#m-inf2)