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

## M22 -- Discharge the failable-call standard across the workspace

The standard is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md#a-failable-call-has-its-failure-handled-always): a call that
can fail has its failure handled, with no per-site analysis. These items apply it to code
written before it was stated.

- [ ] **M22.1** -- Audit every bare `unsafe { Call(...) };` statement in the workspace and handle
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
  it enforces nothing at the sites that matter. It becomes available only after M22.2, on our own
  wrappers -- which is the durable end state, because a `#[must_use]` wrapper gives permanent
  enforcement with none of the lint's noise.

- [ ] **M22.2** -- Introduce a checked owning handle type and route the `CloseHandle` sites through
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
  function pointer, which is exactly where such a path would be. Depends on M22.1's
  classification.

## M30 -- Find out how much of this workspace's algorithm correctness can be machine-checked

Why this milestone exists, what it is not, and how it relates to
[D-31](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31) are recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md#machine-checking-what-is-argued) rather than here, per the
action-only rule for checklist files.

- [ ] **M30.1** -- Survey the workspace for algorithms whose correctness is currently argued rather
  than checked, and match each to the class of tool that could check it.

  Candidates, not exhaustive: `reserving_mpsc`'s packed claim word and its reservation admission rule;
  `slotwise_mpsc`'s per-slot sequence protocol; the experimental permit claim; the doorbell's mirror
  flag against `SetEvent`/`ResetEvent`; `windows-file-watcher`'s contract state machine;
  `windows-ioring-sys`' submission/completion ring.

  For each, record which of these fits and why: **TLA+/PlusCal** (protocol-level, exhaustive over a
  small configuration, no memory model -- its actions are atomic and interleaved, which is sequential
  consistency); **loom** (actual Rust under the C11 memory model, which is where the measured
  weakened-`Acquire` blind spot lives); **kani or similar bounded proof** (Rust, memory-safety and
  assertion checking); **`const` assertions** (arithmetic relationships between constants, already
  used here and the cheapest of the four, because they fail the build rather than a run somebody
  chose to make).

  The output is a table, and the "no tool fits this" rows are as valuable as the rest.

- [ ] **M30.2** -- Pilot exactly one, chosen because parameter shrinking turns an untestable property
  into an exhaustive one.

  `reserving_mpsc`'s claim-position recurrence (`SH-14.1`) is the strongest candidate, and the reason
  is the interleaving rather than the count. Reaching the wrap takes 2^32 pushes -- about 37 seconds
  of sustained maximum-rate pushing on the host the crate publishes, which is far outside a unit
  suite budgeted in milliseconds but is not in itself beyond a long integration test. What is beyond
  any test is the rest of the condition:
  [crates/windows-waitable-queues/README.md](crates/windows-waitable-queues/README.md) records that
  reaching the wrap is necessary but *not sufficient* -- a producer must also be stalled inside a
  window a few instructions wide, and no test can schedule that deliberately. A model whose position
  wraps at 8 makes the whole interleaving reachable in seconds and *exhaustive* rather than sampled,
  and yields a counterexample trace rather than a suspicion.

  An earlier version of this item said the defect "needs 2^32 pushes to manifest and is therefore
  beyond any test", which overstated the limitation and named the wrong reason for it. It also
  offered `capacity == 1` as a second candidate; that is
  [D-12](crates/windows-waitable-queues/DESIGN-NOTES.md#d-12), an edge case of `slotwise_mpsc`'s slot
  *sequence* protocol and unrelated to `reserving_mpsc`'s claim position -- and one already resolved
  by that shape refusing a capacity below two, with a sabotage entry holding it. Pointing the pilot
  at it would have sent it after a different shape's settled property.

  **Write down what makes the wrap-at-8 model evidence about `Balanced`, or record that it is not.**
  Shrinking the position to 3 bits is a claim that the protocol's correctness does not depend on the
  field's width -- that the shipping code refines the model. State that correspondence explicitly:
  which constants were shrunk, why the protocol is uniform in each, and what a counterexample at 8
  therefore implies at 2^32. If it cannot be argued, the pilot's result is a counterexample *in a toy
  model* and must be reported as exactly that. A reduced model that nobody has tied to the code can
  pass and mean nothing, which is the "measures the model, not the thing" trap in a second costume.

  **Success needs both halves.** The unmodified model must satisfy its invariant, *and* a
  deliberately broken variant must produce a counterexample. Neither alone is enough: a green run on
  the correct model says nothing if the model is too permissive or the invariant vacuous, and a
  counterexample from the broken variant can also be produced by a malformed model that would find
  one anywhere. The first is the property check; the second is the anti-vacuity check, and it is the
  same sabotage discipline the test suites here already follow. An earlier version of this item asked
  only for the second, which overcorrected.

- [ ] **M30.3** -- Write down what the pilot could NOT reach, by name.

  This is the item the milestone exists for. Expect the list to include: the memory orderings, if the
  tool has no memory model; every syscall boundary, including the doorbell's; anything whose
  correctness depends on the allocator, the scheduler, or real time; and the gap between the model
  and the code, which no tool closes.

  Put it where a reader deciding how much to trust the crate will meet it -- beside the existing
  "How far the memory orderings are verified, and how far they are not" section. **That section has
  two copies**, [README.md](crates/windows-waitable-queues/README.md) and
  [src/lib.rs](crates/windows-waitable-queues/src/lib.rs), and updating one would leave the other
  telling an adopter something the crate no longer believes. Update both, or make one derive from the
  other -- the README is already a build input via `#[doc = include_str!]`, so the second option is
  available and is the one that cannot drift.

- [ ] **M30.4** -- Re-home `M31.6`, which is currently orphaned.

  `windows-waitable-queues`' design notes reference `M31.6` in three places as the planned `loom`
  verification, and [crates/windows-waitable-queues/README.md](crates/windows-waitable-queues/README.md)
  tells adopters it is planned before 1.0. Until this item, there was no live checklist item for it
  anywhere in the repository -- the crate has only a
  [COMPLETED-CHECKLIST.md](crates/windows-waitable-queues/COMPLETED-CHECKLIST.md). That was the
  "design notes are not a work queue" failure the repository instructions name: a public commitment
  that nothing will cause anyone to pick up.

  Give it a real item in a real checklist, with its scope as D-31 describes it (both MPSC shapes or
  neither), and repoint every reference at it. **There are five, not three**: three in
  [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md), one in
  [src/doorbell.rs](crates/windows-waitable-queues/src/doorbell.rs), and one in
  [sabotage.json](crates/windows-waitable-queues/sabotage.json). Sweeping only the design notes would
  leave a stale identifier in a source file and in the sabotage manifest -- the same
  fix-the-reported-site-not-the-class failure this repository keeps paying for.

  Opening queue work also obliges the component tracker:
  [crates/windows-waitable-queues/PLANS.md](crates/windows-waitable-queues/PLANS.md) currently says
  "No checklist is open against this crate", which this item falsifies. Add the row, as other crates
  do for root-owned checklists.

- [ ] **M30.5** -- Decide what, if anything, the workspace adopts, and record the decision with its
  cost.

  The cost to name explicitly, because it is the one this workspace keeps paying: **a specification is
  another statement of the contract, and it can drift from the code with nothing to detect it.** That
  is the restatement-drift problem in
  [.github/copilot-instructions.md](.github/copilot-instructions.md)'s CONTRACT INTEGRITY section,
  applied to an artefact that is harder to keep honest than prose because it looks authoritative. A
  stale model that still passes is worse than no model.

  So the decision has to answer: what keeps the model and the code in step, who re-runs it, and what
  happens when they disagree. "Adopt nothing, and say why" is a legitimate outcome -- D-31 reached it
  once already on narrower grounds.

  **Either outcome obliges a contract sweep, and a no-adoption outcome obliges it most.** Three
  public places promise machine-checked verification before 1.0:
  [README.md](crates/windows-waitable-queues/README.md),
  [src/lib.rs](crates/windows-waitable-queues/src/lib.rs), and D-31 in
  [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31). Deciding to adopt nothing
  without reconciling those leaves the crate promising adopters something no item will deliver --
  which is the failure M30.4 exists to fix, recreated by the milestone that fixed it. Sweep all three
  as part of this item, whichever way it goes.

## M-inf -- Parked

Ungated work with no identified predecessor deliverable.

- [ ] **M-inf.1** -- Root-cause the process death when impersonating the UAC-linked token. The device-map
  probe reached a marker immediately before `ImpersonateLoggedOnUser` on a token obtained via
  `TokenLinkedToken` and never the marker immediately after, with no panic message. It was removed from the
  probe because a `LOGON32_LOGON_NEW_CREDENTIALS` token answered the question with a passing control, so
  the fallback was redundant -- not because the crash was understood. Parked rather than dropped so the
  unexplained result is not mistaken for a tested one.

- [ ] **M-inf.2** -- Archive the eight completed milestone groups in
  [CHECKLIST-thread-ambient.md](CHECKLIST-thread-ambient.md) into
  [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

  **Raised by a review that named one item, and measured to be eight groups.** The comment asked for
  M26.5's completed multi-line body to be replaced by a one-line stub, per the checklist-hygiene rule
  that an active checklist is an action queue. That rule is right and the file does violate it -- but
  M26.5 is not exceptional: its five siblings in M26 are written the same way, so stubbing only the
  reported item would have made it inconsistent with the group it belongs to rather than more
  consistent with the rule.

  Counted rather than assumed, every group in the file is complete and due for migration under the
  "move the completed group" rule: M22 (8 items), M23 (6), M24 (6), M25 (7), M26 (6), M27 (6),
  M28 (4) and M29 (5). Only `M26+` has open items, and it is what keeps the file alive.

  Not taken in PR #86 because that branch corrects `GetFullPathNameW` documentation and touched
  M26.5 only to fix one technical premise inside it. Migrating roughly 400 lines of another feature's
  bookkeeping through it would bury the change it exists to make. The migration is mechanical, is its
  own commit, and needs the group headings dated per the archive format -- date-only on the `## Moved`
  line, with any precise timestamp reserved for an anchored item heading.
