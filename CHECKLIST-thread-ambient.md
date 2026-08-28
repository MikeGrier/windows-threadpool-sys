# Checklist: windows-thread-ambient-sys and windows-namespace-request-sys

Feature-scoped checklist for the `mikegrier/thread-ambient` branch. It covers two new crates and the
workspace-level changes that introduce them, so it lives at the workspace root -- their lowest common
source-component -- rather than inside either crate. Per the naming convention for feature files, it is
deleted outright once every item is complete, with the content moved to
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

**This file is the whole of this branch's work.** [CHECKLIST.md](CHECKLIST.md) holds the deferred
namespace-facility work (M19-M21, M-inf), which this branch imported but does **not** execute; that import
exists so the design decisions landing here do not reference queued work that is absent from `main`.

Authoritative decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) and, for the first crate, in
[crates/windows-thread-ambient-sys/DESIGN-NOTES.md](crates/windows-thread-ambient-sys/DESIGN-NOTES.md).

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

- [x] **M22.1** -- Record the extraction decision and the WOW64 correction in
  [DESIGN-NOTES.md](DESIGN-NOTES.md), sweeping every statement of each rather than the one site a reader
  happens to notice. Two changes. First, the composite is extracted **now**, into
  `windows-thread-ambient-sys`: the imported text says it "lives in the facility's crate" and is "not
  extracted preemptively", which was written when the facility was its only consumer, and an independent
  consumer is exactly the trigger that decision named. Second, WOW64 filesystem redirection moves from
  **transplanted** to **declared**, because `Wow64DisableWow64FsRedirection` has no getter -- there is no
  value to transplant, so the transplanted classification was not implementable. That dissolves the WOW64
  half of the session's open question rather than leaving it standing, and the open question must be struck
  in the same commit.
  **Landed together with M22.3, and the coupling is a defect in this plan rather than a convenience:** the
  correction's authoritative statement links to the new crate's `DESIGN-NOTES.md`, so writing it before the
  crate existed would have created a broken cross-reference. Sequencing M22.3 first would have been the
  correct plan.
- [x] **M22.2** -- Measure which `SEM_` bits `SetThreadErrorMode` actually accepts, because it decides
  which bits this crate can offer as declarable. The documented set is three bits and excludes
  `SEM_NOALIGNMENTFAULTEXCEPT`, which is process-scoped and sticky once set. If measurement confirms that,
  M21.2's second sub-question dissolves rather than needing an ARM64/x64 pair, and M21.2 is updated to say
  so. Reason it from measurement, not from the documentation.
  **Measured.** Settable: `SEM_FAILCRITICALERRORS`, `SEM_NOGPFAULTERRORBOX`, `SEM_NOOPENFILEERRORBOX`.
  `SEM_NOALIGNMENTFAULTEXCEPT` is **rejected** with `ERROR_INVALID_PARAMETER` -- loudly, not silently
  dropped, which is what the probe read every value back to distinguish. Two findings beyond the documented
  list: an invalid bit fails the **whole** call, installing none of the valid bits alongside it, so the
  declarable type must be unable to represent it rather than validating it at runtime; and M21.2 is
  narrowed rather than closed, since `SEM_NOGPFAULTERRORBOX` is settable and remains a real policy
  question. Recorded in
  [crates/windows-thread-ambient-sys/DESIGN-NOTES.md](crates/windows-thread-ambient-sys/DESIGN-NOTES.md).

- [x] **M22.3** -- Create the crate: `Cargo.toml`, workspace membership, `README.md`, a `CHANGELOG.md`
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

- [ ] **M24+.2** -- Apply the M22.2 narrowing to M21.2 in [CHECKLIST.md](CHECKLIST.md). That item still
  says the error-mode sub-question "needs measurement on both ARM64 and x64 rather than reasoning", which
  M22.2 has since performed: `SEM_NOALIGNMENTFAULTEXCEPT` is rejected by `SetThreadErrorMode` outright, so
  it drops out of the question entirely and needs no architecture pair, and only `SEM_NOGPFAULTERRORBOX`
  remains a genuine policy question. The narrowing was deliberately **not** written into `CHECKLIST.md`
  here, to keep that file byte-identical to the branch it was imported from; the cost of that choice is
  this item, without which `main` would carry a request to measure something already measured. Record the
  measured trap in the same edit: an invalid bit fails the whole `SetThreadErrorMode` call, so a
  forced-plus-transplanted combination installs nothing if the transplanted part is invalid.
