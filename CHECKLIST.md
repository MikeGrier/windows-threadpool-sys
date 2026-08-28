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

## M22 -- `windows-thread-ambient-sys`: decisions and per-aspect primitives

The captured-context composite is extracted into its own crate and lands **before** M19-M21, despite the
higher milestone number -- the numbering records authoring order, not execution order. The trigger is the
one the imported decision named: an independent consumer exists that needs to carry a caller's ambient
state onto another thread without any of the namespace facility around it. The crate is a *level*
platform, so it offers each aspect for capture **and** for explicit declaration, and does not bake in the
namespace facility's dialog-suppression policy; that policy is composed by the facility from primitives
this crate provides.

Scope boundary, stated so the crate cannot swell: it carries thread-scoped ambient state that changes what
a Win32 call does. It does not carry request parameters, does not open files, and does not know what a
namespace operation is.

- [ ] **M22.1** -- Record the extraction decision and the WOW64 correction in
  [DESIGN-NOTES.md](DESIGN-NOTES.md), sweeping every statement of each rather than the one site a reader
  happens to notice. Two changes. First, the composite is extracted **now**, into
  `windows-thread-ambient-sys`: the imported text says it "lives in the facility's crate" and is "not
  extracted preemptively", which was written when the facility was its only consumer, and an independent
  consumer is exactly the trigger that decision named. Second, WOW64 filesystem redirection moves from
  **transplanted** to **declared**, because `Wow64DisableWow64FsRedirection` has no getter -- there is no
  value to transplant, so the transplanted classification was not implementable. That dissolves the WOW64
  half of the session's open question rather than leaving it standing, and the open question must be struck
  in the same commit.

- [ ] **M22.2** -- Measure which `SEM_` bits `SetThreadErrorMode` actually accepts, because it decides
  which bits this crate can offer as declarable. The documented set is three bits and excludes
  `SEM_NOALIGNMENTFAULTEXCEPT`, which is process-scoped and sticky once set. If measurement confirms that,
  M21.2's second sub-question dissolves rather than needing an ARM64/x64 pair, and M21.2 is updated to say
  so. Reason it from measurement, not from the documentation.

- [ ] **M22.3** -- Create the crate: `Cargo.toml`, workspace membership, `README.md`, a `CHANGELOG.md`
  baseline, a row in [PLANS.md](PLANS.md), and a crate `DESIGN-NOTES.md` recording the shape decisions
  before any of them are implemented -- the two-set decomposition (a capture set over capturable aspects,
  and declared fields that have nothing to collect and default to leaving the worker's value alone); the
  three-state per-aspect value that keeps *not captured* distinguishable from *captured and absent*, since
  both end with the worker on its own value and only one is deliberate; the default capture set as a
  **named constant** rather than a `Default` impl, because growing an implicit default silently changes
  behaviour for callers who never named it; the guard composition order; and the per-aspect restore policy.

- [ ] **M22.4** -- Implement the thread error mode aspect: capture via `GetThreadErrorMode`, declaration of
  an explicit value, and scoped application restoring the worker's entry value on every path including
  unwind. This is the aspect that appears in **both** categories, and that is deliberate -- the facility
  captures the caller's value for diagnostics while declaring the forced dialog-suppressing bits, and
  keeping both available here is what stops this crate encoding one consumer's policy. Depends on M22.2 for
  the accepted bit set.

- [ ] **M22.5** -- Implement the impersonation aspect by consuming
  [windows-impersonation-token-sys](crates/windows-impersonation-token-sys/DESIGN-NOTES.md) rather than
  reimplementing capture, transport, or restoration. Its restore failure is fail-fast and that semantics is
  inherited unchanged; note in the crate notes that its capture never yields an absent token, because it
  snapshots the process identity when the thread has none, so this aspect's *absent* state is unreachable
  by construction while the three-state shape is retained for uniformity.

- [ ] **M22.6** -- Implement the TxF transaction aspect: capture the calling thread's current transaction,
  carry an owned duplicate so the value does not depend on the caller's handle outliving it, and apply it
  around the callback. Bind `ktmw32` lazily rather than linking it, so a consumer that never captures a
  transaction does not acquire a dependency nothing else in the workspace has. State the hazard the aspect
  cannot remove: the caller may commit or roll the transaction back while the worker is still inside it.

- [ ] **M22.7** -- Implement the declared aspects -- WOW64 filesystem redirection, memory priority, and I/O
  priority. Each is unspecified by default, meaning the worker's own value is left untouched. Record why
  each is declared rather than captured, per aspect rather than as one blanket statement: redirection has
  no getter at all, memory priority is readable but is a policy choice rather than something a caller
  implicitly consents to remoting, and I/O priority has no documented getter and moves only in lockstep
  with CPU priority through background mode. Depends on M22.1 for the reclassification.

## M23 -- `windows-thread-ambient-sys`: the composite

- [ ] **M23.1** -- Implement the capture set and its named default, covering only the capturable aspects.
  The default set is a named constant whose growth is a breaking change, so a caller who wants stability
  can name aspects explicitly and a caller who takes the default can see what it contains.

- [ ] **M23.2** -- Implement composite capture, failing synchronously on the calling thread. A capture that
  cannot be performed is an admission failure, not a deferred one, and the error names which aspect failed.

- [ ] **M23.3** -- Implement application as a composition of per-aspect guards, applied outermost-first and
  released in exact reverse, with the impersonation guard innermost because its window is narrowest and its
  restoration is the one that must not be delayed. Applying a subset must stay expressible, which is what
  the differing application windows require. Restore failure is fail-fast for impersonation, inherited
  rather than chosen; for the other aspects it is reported rather than fatal, and the report must reach the
  caller instead of being dropped on the floor.

- [ ] **M23.4** -- Prove the crate across a real thread boundary rather than only in-process: capture on
  one thread, apply on a thread-pool worker, and assert each aspect took effect there and was restored
  afterwards. Include the negative that motivates the whole crate -- an uncaptured aspect does **not**
  arrive on the worker -- since a test suite that only ever sees capture succeed cannot tell the two apart.
  Complete the API documentation, the README examples, and the changelog baseline.

## M24 -- `windows-namespace-request-sys`: marshalable namespace call parameter sets

A sibling crate, not a layer above M22-M23: a request carries no ambient context, and a context is useful
to work that never opens a file. The submission site pairs them, which is what keeps both independently
reusable. This crate is the catalogue-plus-faithful-execution layer -- synchronous, testable with no ring,
pool, or async anywhere near it. `CreateFileW` is its first entry and the family grows by one entry per
Win32 call.

- [ ] **M24.1** -- Create the crate, with a `DESIGN-NOTES.md` recording the three boundary decisions before
  implementation: a request excludes ambient context; a request captures parameters and performs the call
  faithfully, but does not choose a delivery model, so the handle-destination fork stays out and an opened
  handle comes back plain and unassociated; and the family grows one entry per Win32 call.

- [ ] **M24.2** -- Capture the security attributes. This is the substance of the crate: a caller's
  descriptor may be **absolute**, holding raw pointers to owner SID, group SID, DACL and SACL that are
  quite possibly on the caller's stack, so capture normalises to **self-relative** and owns the resulting
  contiguous blob. Two traps must be handled rather than discovered: a self-relative descriptor requires
  DWORD alignment, which a plain boxed byte slice does not guarantee; and *no descriptor*, *a descriptor
  with a NULL DACL*, and *a descriptor with an empty DACL* are three different security outcomes that the
  type must keep distinct. Validate on capture, so an invalid descriptor fails at the caller rather than on
  the worker.

- [ ] **M24.3** -- Own the template file handle by duplicating it at capture, so the request does not
  depend on the caller keeping its handle open, and the duplicate is closed with the request.

- [ ] **M24.4** -- Assemble the `CreateFileW` request over the owned parameter set, resolving the path on
  the calling thread at construction because the process current directory is mutable by any thread. Bind
  to the shipped precedent in
  [crates/windows-file-enumeration-sys/src/path.rs](crates/windows-file-enumeration-sys/src/path.rs)
  rather than writing a second path preparation. The request inherits M20.1: until the
  session-independent path form is decided, a session-relative drive letter is a documented hazard on this
  type, and the documentation must say so rather than imply the resolution is complete.

- [ ] **M24.5** -- Execute the request faithfully and synchronously, returning an owned handle or the raw
  Win32 code **unaltered**. Preserving the code is a constraint from a real consumer, not a stylistic
  choice: `ERROR_FILE_NOT_FOUND` means different things across an open and its follow-up queries, and only
  the consumer can disambiguate, so the crate must not normalise or reclassify. Capture `GetLastError`
  before any restoration runs, so nothing in between overwrites it.

- [ ] **M24.6** -- Test the security-descriptor path against descriptors that are absolute, self-relative,
  null, empty-DACL, and invalid, and prove a captured request survives the caller dropping every input it
  was built from. Complete the API documentation, README examples, and changelog baseline.

## M24+ -- Gated on the namespace-facility design branch landing

- [ ] **M24+.1** -- Reconcile the duplicated design background. This branch imported
  [DESIGN-NOTES.md](DESIGN-NOTES.md)'s namespace-plane section and its design session byte-identical from
  `mikegrier/pseudo-async-file-ops` so the merge would resolve automatically, then corrected part of it
  under M22.1. Once both branches are on `main`, verify that exactly one statement of each fact survived
  the merge and that the corrected statements won, since an automatic resolution of near-identical text is
  precisely how a superseded statement survives unnoticed.

## M-inf -- Parked

Ungated work with no identified predecessor deliverable.

- [ ] **M-inf.1** -- Root-cause the process death when impersonating the UAC-linked token. The device-map
  probe reached a marker immediately before `ImpersonateLoggedOnUser` on a token obtained via
  `TokenLinkedToken` and never the marker immediately after, with no panic message. It was removed from the
  probe because a `LOGON32_LOGON_NEW_CREDENTIALS` token answered the question with a passing control, so
  the fallback was redundant -- not because the crash was understood. Parked rather than dropped so the
  unexplained result is not mistaken for a tested one.
