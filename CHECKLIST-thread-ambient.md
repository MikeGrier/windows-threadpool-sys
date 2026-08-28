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

- [x] **M22.4** -- Implement the thread error mode aspect: capture via `GetThreadErrorMode`, declaration of
  an explicit value, and scoped application restoring the worker's entry value on every path including
  unwind. This is the aspect that appears in **both** categories, and that is deliberate -- the facility
  captures the caller's value for diagnostics while declaring the forced dialog-suppressing bits, and
  keeping both available here is what stops this crate encoding one consumer's policy. Depends on M22.2 for
  the accepted bit set.

- [x] **M22.5** -- Implement the impersonation aspect by consuming
  [windows-impersonation-token-sys](crates/windows-impersonation-token-sys/DESIGN-NOTES.md) rather than
  reimplementing capture, transport, or restoration. Its restore failure is fail-fast and that semantics is
  inherited unchanged; note in the crate notes that its capture never yields an absent token, because it
  snapshots the process identity when the thread has none, so this aspect's *absent* state is unreachable
  by construction while the three-state shape is retained for uniformity.

- [x] **M22.6** -- Implement the TxF transaction aspect: capture the calling thread's current transaction,
  carry an owned duplicate so the value does not depend on the caller's handle outliving it, and apply it
  around the callback. Bind `ktmw32` lazily rather than linking it, so a consumer that never captures a
  transaction does not acquire a dependency nothing else in the workspace has. State the hazard the aspect
  cannot remove: the caller may commit or roll the transaction back while the worker is still inside it.

- [x] **M22.7** -- Implement the declared aspects -- WOW64 filesystem redirection, memory priority, and I/O
  priority. Each is unspecified by default, meaning the worker's own value is left untouched. Record why
  each is declared rather than captured, per aspect rather than as one blanket statement: redirection has
  no getter at all, memory priority is readable but is a policy choice rather than something a caller
  implicitly consents to remoting, and I/O priority has no documented getter and moves only in lockstep
  with CPU priority through background mode. Depends on M22.1 for the reclassification.

- [x] **M22.8** -- Give the aspect surface runnable examples, and compile the README as doctests. Added
  after M22.7 landed with **zero** doctests, which execution revealed to be a planning error rather than a
  deferral: M23.4 had scheduled all documentation at the end of the composite, so the aspects would have
  shipped a whole milestone with examples nothing compiled. Per this repository's rule that prose
  containing code must compile, the README carries
  `#[cfg(doctest)] #[doc = include_str!("../README.md")]`, so a contract change breaks the build instead of
  leaving the README teaching the old answer. Verify by sabotage that the README examples are genuinely
  executed rather than merely parsed. M23.4 retains the *composite's* documentation.
## M23 -- `windows-thread-ambient-sys`: the composite

- [x] **M23.1** -- Implement the capture set and its named default, covering only the capturable aspects.
  The default set is a named constant whose growth is a breaking change, so a caller who wants stability
  can name aspects explicitly and a caller who takes the default can see what it contains.

- [x] **M23.2** -- Implement composite capture, failing synchronously on the calling thread. A capture that
  cannot be performed is an admission failure, not a deferred one, and the error names which aspect failed.

- [x] **M23.3** -- Implement application as a composition of per-aspect guards, applied outermost-first and
  released in exact reverse, with the impersonation guard innermost because its window is narrowest and its
  restoration is the one that must not be delayed. Applying a subset must stay expressible, which is what
  the differing application windows require. Restore failure is fail-fast for impersonation, inherited
  rather than chosen; for the other aspects it is reported rather than fatal, and the report must reach the
  caller instead of being dropped on the floor.

- [x] **M23.4** -- Prove the *composite* across a real thread boundary rather than only in-process (the
  per-aspect cross-thread cases already landed with M22.4-M22.7, and the aspect documentation with M22.8):
  capture on
  one thread, apply on a thread-pool worker, and assert each aspect took effect there and was restored
  afterwards. Include the negative that motivates the whole crate -- an uncaptured aspect does **not**
  arrive on the worker -- since a test suite that only ever sees capture succeed cannot tell the two apart.
  Complete the API documentation, the README examples, and the changelog baseline.

- [x] **M23.5** -- Prove the composite against a **many-worker consumer's shape**, which is the audit's
  second purpose and was not discharged when M23 was closed. The in-repository consumers each apply a
  captured state on one worker at a time; Globazog takes one capture at `submit()` and shares it across up
  to 64 concurrent workers for the length of a traversal, and nothing currently tests that. Assert
  `AmbientState: Sync` -- it holds, but only `Send` was asserted, and `Send` alone would let this design
  pass its own suite and then fail to compile in the consumer that motivated it. Share one `Arc<AmbientState>`
  across concurrent pool callbacks, applying and restoring independently on each, and assert every worker
  saw the captured context and was left clean. Then document the two things a consumer of that shape must
  know and cannot currently learn from the crate: that applying once around a batch and applying per
  operation are both expressible and differ by a `SetThreadToken` per operation, so the granularity choice
  is theirs to make deliberately; and that an impersonation restore failure is fail-fast, which on a shared
  pool means a process abort rather than one failed operation.

## M24 -- `windows-namespace-request-sys`: foundations

A sibling crate, not a layer above M22-M23: a request carries no ambient context, and a context is useful
to work that never opens a file. The submission site pairs them, which is what keeps both independently
reusable. This crate is the catalogue-plus-faithful-execution layer -- synchronous, testable with no ring,
pool, or async anywhere near it. The family grows by one entry per Win32 call.

**The round-one entry list is audited, not guessed.** It is the union of what three real consumers call:
[windows-file-watcher](crates/windows-file-watcher/src/directory.rs) and
[windows-file-enumeration-sys](crates/windows-file-enumeration-sys/src/native.rs) in this repository, and
`MikeGrier/Globazog-rs` at commit `55a0b1ae`.

| # | Entry | Needed by | Shape observed |
|---|---|---|---|
| 1 | `CreateFileW` | all three | `FILE_LIST_DIRECTORY`, share `R\|W\|D`, `OPEN_EXISTING`, `FILE_FLAG_BACKUP_SEMANTICS`; the watcher adds `FILE_FLAG_OVERLAPPED` (port branch), the other two omit it (unassociated branch) |
| 2 | `OpenFileById` | watcher | volume-hint handle + `FILE_ID_DESCRIPTOR`; no creation disposition |
| 3 | `FindFirstChangeNotificationW` | watcher | path, subtree flag, `FILE_NOTIFY_CHANGE_*` mask; handle-producing |
| 4 | `CloseHandle` and variant close routines | all three | `FindCloseChangeNotification` is **not** `CloseHandle` |
| 5 | `GetFileInformationByHandleEx` | all three | five classes: `FileBasicInfo`, `FileIdInfo`, `FileCaseSensitiveInfo`, `FileIdExtdDirectoryInfo`, `FileIdExtdDirectoryRestartInfo` |
| 6 | `GetFileInformationByHandle` (non-Ex) | watcher | `BY_HANDLE_FILE_INFORMATION`; a distinct call, not a class of entry 5 |
| 7 | `GetFinalPathNameByHandleW` | watcher directly, Globazog via `std::fs::canonicalize` | `VOLUME_NAME_DOS \| FILE_NAME_NORMALIZED` |
| 8 | `GetVolumeInformationByHandleW` | watcher | handle-based, not the path-based `GetVolumeInformationW` |
| 9 | `GetFullPathNameW` | enumeration | lexical only |

Four audit findings that shape the milestones below, recorded because each contradicts an assumption the
first draft of this plan was written on.

**Five of the nine entries take a handle, not a path.** The first draft assumed a request owns everything
it names. Decided: a request **owns a duplicate**, taken with `DuplicateHandle` at capture, so it is
self-contained and cannot be left referencing a handle its originator has closed. That makes handle
ownership a shared primitive rather than an `hTemplateFile` detail.

**No consumer passes a security descriptor or a template file, and none creates a file.** Every audited
open is `OPEN_EXISTING` against a directory with a null `lpSecurityAttributes` and a null `hTemplateFile`.
Those parts of the `CreateFileW` entry are kept anyway: an entry that cannot express two of its own
parameters is a *narrowed* `CreateFileW`, and narrowing a platform entry to fit currently visible consumers
is the anti-pattern this repository's platform-integrity rule names. This is recorded so a later reader does
not mistake the absence of a consumer for an oversight.

**The strongest offload evidence is not an open.** Globazog's `QueryBuilder::submit()` calls
`std::fs::canonicalize` on the **caller's** thread, once per root -- a full `CreateFileW` plus
`GetFinalPathNameByHandleW` plus `CloseHandle` with unbounded latency on a network path -- and
`escapes_confinement()` repeats it per reparse-point candidate on a worker. Entry 7 is therefore
first-class, not second-tier.

**Globazog is a prospective consumer of the ambient crate, not evidence against it.** An earlier draft of
this section recorded that Globazog "uses no ambient thread state at all" and drew a structural conclusion
from it -- that the two crates are siblings rather than a stack. The observation is accurate about the code
as it stands and the inference from it was wrong: a consumer that is still synchronous-on-worker-threads
has not *needed* ambient state yet, which says nothing about whether it will. Globazog's own notes schedule
the async follow-up (`NtQueryDirectoryFile` plus IOCP), and that is exactly the point at which its work
moves onto pool workers and the caller's identity has to be marshaled to reach it. Every aspect this
workspace carries is plausibly live for it: impersonation for identity, the error mode because a traversal
is precisely what meets a dead network path or an empty removable drive on a shared pool thread, WOW64
redirection for a 32-bit host, and priority for a background scan. The sibling claim still stands, but on
its own footing -- a request needs no context and a context needs no request -- and not on this evidence.

**The audit had two purposes and only one was discharged.** Establishing the operation set is the first;
establishing that the *scenario* is adequately served is the second, and it was not answered. Globazog's
shape makes the scenario concrete and demanding in a way the in-repository consumers do not: one capture
taken at `submit()`, shared by up to 64 concurrent workers, applied repeatedly over a traversal that may
run for minutes. That imposes requirements no existing test covers, which are queued as M23.5 rather than
assumed:

- **One state, many workers, concurrently.** This needs `AmbientState` to be `Sync` and shareable through
  an `Arc`, not merely `Send`. It *is* `Sync`, verified, but only `Send` was ever asserted -- and `Send`
  alone would let a design pass its tests and then fail to compile in the consumer that motivated it.
- **Granularity is the consumer's choice and has a cost.** Applying the composite once around a batch of
  directories and applying it per open are both expressible, and they differ by a `SetThreadToken` per
  operation. Globazog's worker loop processes many directories per invocation, so the choice is real and
  the crate should say what it costs rather than leave it to be discovered.
- **Fail-fast has a blast radius on a shared pool.** An impersonation restore failure panics, and a
  panicking pool callback aborts the process. That is inherited and correct, but a consumer running 64
  concurrent impersonated workers should learn it from the documentation rather than from an incident.
- **Path resolution under a captured identity is still open.** Globazog resolves its roots on the
  *submitting* thread and opens them on workers. Under a token from another logon session, M20.1's
  session-relative drive letter hazard makes that a genuine divergence rather than a theoretical one, and
  the namespace-request crate inherits it.
- [x] **M24.1** -- Create the crate, with a `DESIGN-NOTES.md` recording the boundary decisions before
  implementation: a request excludes ambient context; a request captures parameters and performs the call
  faithfully but does not choose a delivery model, so the handle-destination fork stays out and an opened
  handle comes back plain and unassociated; the family grows one entry per Win32 call; and a request owns
  duplicates of any handle it names. Record the audited entry list above as the round-one scope, with its
  provenance, so a later reader can tell a deliberate omission from an unexamined one.

- [x] **M24.2** -- Implement owned handle references: duplicate at capture with `DuplicateHandle`, own the
  duplicate for the request's life, and close it with the request. This is the shared primitive behind both
  `hTemplateFile` and the five handle-taking entries, so it lands before any of them. Cover the case the
  audit makes unavoidable -- a source handle that is already closed, or is a pseudo-handle -- and decide
  whether duplication failure is a construction error (it is: capture fails on the caller's thread, where
  the caller can still do something about it).

  State plainly, in the type's own documentation, what a duplicate is and is not, because the distinction
  is the one a caller reasoning in terms of value semantics will get wrong: **a path is a value and is
  copied; a handle is a reference to a kernel object, and duplicating it shares that object rather than
  cloning it.** A request is therefore self-contained with respect to *lifetime* -- it cannot be left
  pointing at a closed handle -- and **not** isolated with respect to *state*. M26.1 measures where that
  distinction has teeth. One property this design depends on is measured there and must be asserted here
  too: closing the duplicate does **not** disturb the source, so a request owning a duplicate and dropping
  it cannot damage the handle its caller kept.

- [x] **M24.3** -- Capture the security attributes. A caller's descriptor may be **absolute**, holding raw
  pointers to owner SID, group SID, DACL and SACL that are quite possibly on the caller's stack, so capture  normalises to **self-relative** and owns the resulting contiguous blob. Two traps must be handled rather
  than discovered: a self-relative descriptor requires DWORD alignment, which a plain boxed byte slice does
  not guarantee; and *no descriptor*, *a descriptor with a NULL DACL*, and *a descriptor with an empty
  DACL* are three different security outcomes the type must keep distinct. Validate on capture, so an
  invalid descriptor fails at the caller rather than on the worker. The alignment requirement is not
  peculiar to descriptors -- M26.1 needs an 8-byte-aligned buffer for the same underlying reason -- so build
  it once as an owned aligned buffer primitive rather than twice.

- [x] **M24.4** -- Implement path preparation: resolve on the calling thread at construction, because the
  process current directory is mutable by any thread. Bind to the shipped precedent in
  [crates/windows-file-enumeration-sys/src/path.rs](crates/windows-file-enumeration-sys/src/path.rs)
  rather than writing a second path preparation. **That precedent's `prepare` is `pub(crate)`**, noticed
  while writing M24.1's design notes, so "bind to it" is not yet possible as written: it must be published
  from that crate or extracted to a shared one first. Duplicating it is the option this repository's
  mono-repo policy rejects -- fix the layer rather than work around it -- so decide which before
  implementing, and treat the decision as part of this item. The result inherits M20.1: until the session-independent
  path form is decided, a session-relative drive letter is a documented hazard on these types, and the
  documentation must say so rather than imply the resolution is complete.

  **Decided: copy it, temporarily and on the record.** Neither published option was taken. The enumeration
  crate is released and this one is not, so making it depend here would make it unpublishable, and this
  branch exists to reach publication with minimal impact on what already ships; extracting a third shared
  crate buys a new published member before any consumer justifies it. The copy is the duplicate-then-decide
  procedure working as intended -- the released path stays untouched while this one is proven -- and it is
  not permitted to become permanent by default: `path.rs` carries a provenance comment naming its source
  and commit, D-9 records the reasoning, and the merge-or-delete decision is scheduled as **M26+.3**, gated
  on this crate's first release.
- [x] **M24.5** -- Establish the faithful-execution contract that every entry then follows: an entry
  returns its result or the raw Win32 code **unaltered**, and `GetLastError` is captured before any
  restoration runs so nothing in between overwrites it. Preserving the code is a constraint from a real
  consumer rather than a stylistic choice -- `ERROR_FILE_NOT_FOUND` means a missing directory from an open,
  an empty directory from a first query, and a genuine failure from a later one, and only the consumer can
  disambiguate.

- [x] **M24.6** -- Test the foundations: security descriptors that are absolute, self-relative, null,
  empty-DACL, and invalid; handle duplication against a live handle, a closed handle, and a pseudo-handle;
  and the property that binds the whole crate together -- a captured request survives the caller dropping
  every input it was built from, including the source handle. Complete the API documentation and the
  changelog baseline.

  **Re-planned during execution.** The enumerated per-case tests were not deferred to this item: each
  landed with the item that introduced the behaviour, which is the sequencing the one-item-then-commit
  loop produces and is better than holding tests back to a trailing test item. What was genuinely left,
  and is what this item delivered, is the **composite** the per-module tests cannot show -- one value
  holding a prepared path, two captured handles, and captured security attributes, outliving every input
  at once and still working on a thread that saw none of them -- plus the crate example, the README
  example compiled as a doctest, and confirmation that the changelog baseline matches its siblings.
## M25 -- `windows-namespace-request-sys`: the handle-producing entries

Entries 1-4 of the audited list. Each depends on M24's foundations and on nothing else.

- [x] **M25.1** -- The `CreateFileW` entry, over the complete parameter set: path, desired access, share
  mode, security attributes, creation disposition, flags and attributes, and template file. It must express
  all three audited flag shapes, including the `FILE_FLAG_OVERLAPPED` split -- the watcher's open is
  destined for a completion port and the other two are not, and that difference is a request field rather
  than something the crate decides.

- [x] **M25.2** -- The `OpenFileById` entry. It is a second open primitive, not a `CreateFileW` variant: it
  takes a volume-hint handle and a `FILE_ID_DESCRIPTOR` and has no creation disposition. One entry per
  Win32 call means it is its own entry, and it is the first consumer of M24.2's owned handle on the input
  side.

- [x] **M25.3** -- The `FindFirstChangeNotificationW` entry. Path, subtree flag, and notification filter,
  producing a handle that is **not** closed with `CloseHandle`.

- [x] **M25.4** -- The close entries. `CloseHandle` belongs in the catalogue because it blocks on
  outstanding I/O and can block hard on a dead network path, which is the whole reason this facility
  exists. The audit shows a close entry cannot assume its routine: `FindCloseChangeNotification` closes
  M25.3's handle and `CloseHandle` is wrong for it. A handle therefore carries its close routine rather
  than the entry assuming one -- the same shape
  [windows-threadpool-sys](crates/windows-threadpool-sys/README.md) already needed for wait targets.

- [x] **M25.5** -- Prove the handle-producing entries against real directories, including the three flag
  shapes the audit found, the non-`CloseHandle` close routine, and a reopen-by-id that survives its source
  handle being closed first. Landed as an integration test (`tests/handle_entries/`) rather than more unit
  tests, because these cross a real filesystem boundary and chain entries together: the per-entry unit tests
  prove each entry against Windows in isolation, and only a composed test reaches the combination the audit
  called out -- a handle opened by one request becoming the *input* to a later one. Also covers the whole
  chain performed on a worker that saw none of its inputs, and many requests across concurrent workers,
  which is Globazog's shape.

- [x] **M25.6** -- Give the catalogue a **test seam**, so a consumer can exercise its own code against these
  entries without a filesystem. Every entry is a value whose `perform` is the single point where Win32 is
  touched, which is already the right shape -- what is missing is a trait over it, so a consumer's code can
  be generic over "a request that produces `T`" and take a fake in its tests. Two traits, not one, because
  the distinction is real rather than cosmetic: an open is a parameter set that may be performed repeatedly
  and takes `&self`, while a close is one-shot and consumes itself. Collapsing them would either make a
  close look repeatable or make every open look single-use. Prove the seam by writing a fake in a doctest --
  a seam nobody has substituted is a seam nobody knows works.

- [x] **M25.7** -- Give the public surface **runnable examples**. The crate currently has 6 doctests against
  roughly 128 public items, which is thin enough that a contract change could silently invalidate the
  documentation without breaking the build. Every public type gets a worked example, and every method whose
  correct use is not obvious from its signature gets one -- with priority on the ones a caller gets wrong:
  the three-way security and DACL distinctions, the two handle-failure conventions, what a duplicated handle
  does and does not share, and rearming a notification. These are compiled, so they cannot rot. This sets
  the standard M26's entries are then held to rather than being a one-off cleanup.

## M26 -- `windows-namespace-request-sys`: the query entries

Entries 5-9 of the audited list. All but the last take a handle, so all but the last depend on M24.2.

- [x] **M26.1** -- The `GetFileInformationByHandleEx` entry: one entry with the info class as a request
  field, per the one-entry-per-Win32-call rule. As a *marshaling* problem this is the easiest entry in the
  catalogue and should be built as such -- its inputs are a handle, a scalar class, and a buffer size, with
  no pointer into caller memory anywhere, so nothing needs normalising. An earlier draft of this item
  claimed the design problem was that the five audited classes have two result shapes (fixed-size
  out-params versus variable-length batches); that was wrong. This crate returns bytes and the unaltered
  outcome and does not parse, so both shapes collapse to one owned aligned buffer, and per-class parsing
  stays with the consumer that already owns it.

  The real difficulty is elsewhere, and it falls directly out of M24.2. **Measured**, not reasoned: an
  earlier draft asserted the following from the object-manager model, which is precisely the kind of claim
  this repository has been burned by. Measured on Windows 11 Enterprise 10.0.28000,
  `aarch64-pc-windows-msvc`, against a real directory with a deliberately small buffer so the cursor
  questions actually arise.

  | Question | Measured |
  |---|---|
  | Does a duplicated handle share the enumeration cursor? | **Yes** -- the source read `.`, `..`, `f00`; the duplicate returned `f01, f02, f03`, a clean continuation |
  | Control: do two separate opens share it? | **No** -- the second open restarted from `.`, so the probe can tell the two apart |
  | Does closing the duplicate disturb the source? | **No** -- the source continued correctly afterwards |
  | Does an interleaved `FileBasicInfo` disturb the cursor? | **No** |
  | Does an interleaved `FileIdInfo` disturb it? | **No** |
  | Does an interleaved non-Ex `GetFileInformationByHandle` disturb it? | **No** |
  | Does `FileBasicInfo` *on the duplicate* disturb the source's enumeration? | **No** |

  So the contract is **narrower** than the earlier draft claimed, and the difference matters. It is not
  that handle-taking entries are hazardous in general: **only the two directory-enumeration classes mutate
  the shared cursor**, and every other query is a pure read that composes freely with an enumeration in
  progress, on the same handle or on a duplicate. What the entry must state is therefore specific: a
  duplicate is not an independent enumeration, and an independent traversal needs a fresh open. This is
  also the one place the unresolved ordering question binds, since two *enumeration* requests against one
  handle are order-dependent in a way that no other pair of entries is.

  Two constraints that are not negotiable and are already solved in this repository, so bind to the
  precedent rather than re-deriving it. The buffer must be **8-byte aligned**: a `Vec<u8>` fails the very
  first query with `ERROR_NOACCESS`, which is why
  [crates/windows-file-enumeration-sys/src/buffer.rs](crates/windows-file-enumeration-sys/src/buffer.rs)
  backs its storage with `Vec<u64>`. And the call **reports no written length** -- a batch is walked by its
  own next-entry offsets -- so the completion returns the whole buffer and the consumer bounds its own
  reads, rather than the entry inventing a byte count it cannot know.

  Record that this entry needs **no ambient context**: access was checked at the open, which is exactly why
  the enumeration crate applies impersonation only around `CreateFileW`. It is the clearest case that a
  request and a context are paired at submission rather than fused.

  Finally, state the relationship to
  [windows-file-enumeration-sys](crates/windows-file-enumeration-sys/DESIGN-NOTES.md), because an entry
  covering the two directory classes otherwise looks like a second implementation of a shipped streaming
  engine. It is not: this entry is **single-shot** -- one call, one batch, and the *client* sequences the
  next, which is the one-entry-per-Win32-call rule applied literally -- while that crate is a streaming
  specialisation over the same shape, owning the cursor, the refill loop, the quanta, and backpressure. All
  five audited classes stay reachable here, because restricting them would narrow the entry for a
  no-consumer reason, which is the same move refused for `lpSecurityAttributes` in M24.3. The documentation
  must nonetheless point a consumer wanting *streaming* enumeration at that crate rather than leaving it to
  rebuild the loop from single-shot calls.

  Recorded because it was challenged directly and the challenge was reasonable: this entry needs almost no
  marshaling work, which invites the conclusion that it does not belong in the catalogue at all. Membership
  is decided by whether a blocking namespace call needs performing off the caller's thread, not by whether
  it is awkward to marshal -- the latter test would select for our implementation convenience rather than
  for consumer need. On the former test this is the most-called namespace operation across all three
  audited consumers, and the call whose lack of an overlapped form is why an unassociated handle is a
  first-class destination at all.

- [x] **M26.2** -- The `GetFileInformationByHandle` entry, returning `BY_HANDLE_FILE_INFORMATION`. It is a  distinct Win32 call rather than a class of M26.1, and the watcher uses it where the Ex form would not do.

- [x] **M26.3** -- The `GetFinalPathNameByHandleW` entry, including the flags the watcher relies on
  (`VOLUME_NAME_DOS | FILE_NAME_NORMALIZED`) and the grow-the-buffer retry the call requires. This is the
  entry the audit identified as having the strongest offload evidence, since Globazog performs it on its
  submitting thread today.

- [x] **M26.4** -- The `GetVolumeInformationByHandleW` entry, returning volume label, serial, and
  filesystem name. Handle-based; the path-based `GetVolumeInformationW` is deliberately not in round one
  because no audited consumer calls it.

- [x] **M26.5** -- The `GetFullPathNameW` entry. Lexical only: it resolves relative components and `.`/`..`
  and never expands a drive letter, so it does **not** close the session-relative hazard from M20.1, and
  its documentation must say which problem it solves and which it leaves standing.

- [x] **M26.6** -- Acceptance, in **two** parts, because the audit had two purposes and checking only the
  first is how the coverage question got missed once already.

  *Operation coverage:* re-express each audited call site from the three consumers against the catalogue
  and confirm every parameter shape they use is reachable. This is the test that the entry list was derived
  from real consumers rather than from taste, and it must be run against all three -- the two
  in-repository crates and Globazog -- rather than the most convenient one.

  *Scenario coverage:* confirm the catalogue serves each consumer's actual **shape**, not just its call
  list. For Globazog specifically that means a request built on one thread and executed on another under a
  captured context, many such requests in flight across concurrent workers from one shared capture, and a
  handle opened by one request being carried into a later one -- which is where M24.2's owned duplicate and
  M26.1's shared enumeration cursor meet, and the one combination no single-entry test exercises. Record
  any gap as a defect rather than adjusting the scenario to fit what was built.

  Complete the API documentation and README examples.

## M27 -- `windows-platform-probes`: keep the measurements executable

Several decisions in this workspace rest on measurements of undocumented Windows behaviour. Recorded only
in prose, a measurement decays silently -- the claim stays in the design note while the platform, or our
reading of it, moves. This milestone gives them a durable home that an ordinary build keeps alive.

- [x] **M27.1** -- Create `windows-platform-probes` as an unpublished workspace member, with each probe's
  logic in a library function that **returns** its observation, so the binaries print it and the tests
  assert it from one implementation. Writing the check twice -- once to print, once to assert -- would make
  the test a check of the copy rather than of the platform, which is the restatement failure this
  repository has already paid for.

- [x] **M27.2** -- Adopt three tiers, because "run all the probes" is not a safe instruction: **asserted**
  (a real test), **ignored** (assertable but slow, heavy, or environment-dependent), and **binary only**
  (cannot be a test -- it hangs by design, mutates the process irreversibly, or needs privileges a test run
  must not assume). Every tier is compiled by an ordinary build, which is the floor. Record the tier of
  each probe and why, so a later contributor does not promote a hostile probe into the test path.

- [x] **M27.3** -- Migrate this session's measurements into the crate as asserted tests: the settable
  `SEM_` bit set, the whole-call failure an invalid bit causes, the independence of the thread error mode
  from the process error mode, and the four handle/cursor findings. Include the controls as their own
  assertions rather than as prose, and make a fixture that cannot exhibit the behaviour a **failure**
  rather than a silent pass. Verify the binding by sabotage -- change a fact and confirm a test actually
  fails -- since a guard only ever seen to pass is untested.

- [x] **M27.4** -- Migrate the nine earlier measurements' probes, which currently exist only in the
  git-ignored `.scratch/` directory and a previous session's private state, and are therefore one machine
  failure away from being lost. They are the evidence for the `IoRing` registration, thread-agnosticism,
  completion-port fork, token inheritance, `CancelSynchronousIo`, thread-pool growth, and device-map
  findings recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). Most belong in the ignored or binary-only tiers:
  one never returns by design, one moves 512 MiB, one spawns 512 threads, and one needs `subst` drives and
  a second logon session. Deliberately **not** done alongside M27.1-M27.3, which established the scheme on
  two cheap probes first.

  Landed as `worker_context` (asserted), `pool_growth`, `device_map` and `ioring` (ignored), and
  `cancel_io` (binary only). Two corrections were made in the move, recorded in
  [crates/windows-platform-probes/DESIGN-NOTES.md](crates/windows-platform-probes/DESIGN-NOTES.md): the
  device-map **control could never have passed** (it read a thread token the non-impersonating side does
  not have, so it always reported "same session"), and the `IoRing` registration probe must **not** use
  `windows-ioring-sys`, whose guard exists because of the very assumption being measured -- probing through
  it would confirm our own belief by consulting it. Calling Win32 directly also closes a standing gap: that
  crate recorded its replace-not-append assumption as explicitly *unverified*, and it is now measured and
  holds.

  **The completion-port fork is not migrated, and is not deferred for lack of need.** The original Probe D
  was superseded by its own corrected rewrite after the first version checked the wrong field and declared
  coexistence while its result code was `ERROR_INVALID_PARAMETER`. Re-establishing that measurement means
  re-deriving which of the two readings is right, which is measurement work rather than migration work.
  Queued as **M27.6** rather than folded in here, so it is scheduled instead of quietly dropped.

- [ ] **M27.5** -- Re-run the probes on an **x64** host and record which findings are architecture-
  dependent. Every measurement in this workspace so far was taken on ARM64. This subsumes M19.5's narrower
  request for the thread-pool numbers, and the binaries exist precisely so this needs no re-derivation.

  **Blocked on hardware, not on work.** There is nothing to build first; it needs a machine this branch has
  not been run on. Everything needed is committed, so it is a run-and-record task:

  ```text
  cargo test -p windows-platform-probes -- --include-ignored
  cargo run  -p windows-platform-probes --bin probe-error-mode
  cargo run  -p windows-platform-probes --bin probe-handle-state
  cargo run  -p windows-platform-probes --bin probe-worker-context
  cargo run  -p windows-platform-probes --bin probe-pool-growth
  cargo run  -p windows-platform-probes --bin probe-device-map
  cargo run  -p windows-platform-probes --bin probe-ioring
  cargo run  -p windows-platform-probes --bin probe-cancel-io
  ```

  A **failing ignored test is the interesting result**, not a problem to fix: those tests assert the shape
  of a finding, so a failure means the finding is architecture-dependent and the design note resting on it
  needs revisiting. Record which, and where.

  The ARM64 baseline to diff against, measured on Windows 11 Enterprise 10.0.28000,
  `aarch64-pc-windows-msvc`, so the comparison has something concrete rather than a memory:

  | Probe | ARM64 result |
  |---|---|
  | settable `SEM_` bits | all but `SEM_NOALIGNMENTFAULTEXCEPT`, which is rejected rather than silently dropped |
  | worker ambient state | no thread token (`ERROR_NO_TOKEN` = 1008), error mode `0x0000` |
  | pool growth, max 4 | 4 threads, slowest arrival ~214us |
  | pool growth, max 8 | 4 arrive in <500us, then ~one per 165ms (651ms slowest) |
  | raise 2 -> 6 while saturated | extra work started ~1.8ms after the raise |
  | device map | `subst` letter resolves in our session (LUID `fd80c`), not in anonymous (`3e6`) |
  | `IoRing` registration | **replaces**: index 0 usable, index 1 not, after re-registering one handle |
  | `IoRing` thread agnosticism | operation completed with result `0x00000000` after its submitter exited |
  | `CancelSynchronousIo`, idle thread | returned `ERROR_NOT_FOUND` (1168) -- point-in-time |
  | `CancelSynchronousIo`, busy thread | 4 attempts all returned; **no wedge on this run**, though the original spike wedged for 12s |

  The last row is the one to treat carefully: it is **nondeterministic**, which is exactly why that probe is
  binary-only. A clean x64 run does not clear the design, and the probe's own output says so.

- [ ] **M27.6** -- Migrate the completion-port fork measurement (Probe D): does associating a handle with
  an IOCP foreclose `IoRing` use of it? This is the evidence for `windows-namespace-request-sys` returning
  an opened handle **plain and unassociated**, so it is load-bearing for a shipped decision rather than a
  curiosity. It was split out of M27.4 because the original probe exists in two versions that disagree --
  the first declared coexistence while checking the wrong field, with a result code of
  `ERROR_INVALID_PARAMETER` and a zero byte count; the second checks the result and adds the negative
  control (the identical read on a non-associated handle) so a failure can be attributed to the
  association rather than to the probe. Migrating it therefore requires deciding which reading is correct,
  which is a fresh measurement rather than a port. Belongs in the ignored tier alongside the other
  `IoRing` probes, and must carry the negative control.

## M26+ -- Gated on the namespace-facility design branch landing

- [ ] **M26+.1** -- Reconcile the duplicated design background. This branch imported
  [DESIGN-NOTES.md](DESIGN-NOTES.md)'s namespace-plane section and its design session byte-identical from
  `mikegrier/pseudo-async-file-ops` so the merge would resolve automatically, then corrected part of it
  under M22.1. Once both branches are on `main`, verify that exactly one statement of each fact survived
  the merge and that the corrected statements won, since an automatic resolution of near-identical text is
  precisely how a superseded statement survives unnoticed.

- [ ] **M26+.2** -- Apply the M22.2 narrowing to M21.2 in [CHECKLIST.md](CHECKLIST.md). That item still
  says the error-mode sub-question "needs measurement on both ARM64 and x64 rather than reasoning", which
  M22.2 has since performed: `SEM_NOALIGNMENTFAULTEXCEPT` is rejected by `SetThreadErrorMode` outright, so
  it drops out of the question entirely and needs no architecture pair, and only `SEM_NOGPFAULTERRORBOX`
  remains a genuine policy question. The narrowing was deliberately **not** written into `CHECKLIST.md`
  here, to keep that file byte-identical to the branch it was imported from; the cost of that choice is
  this item, without which `main` would carry a request to measure something already measured. Record the
  measured trap in the same edit: an invalid bit fails the whole `SetThreadErrorMode` call, so a
  forced-plus-transplanted combination installs nothing if the transplanted part is invalid.

- [ ] **M26+.3** -- Make the merge-or-delete decision on the duplicated path preparation. M24.4 copied
  `windows-file-enumeration-sys`'s `path.rs` into `windows-namespace-request-sys` rather than depending on
  it, because that crate is released and this one was not, and this branch exists to reach publication with
  minimal impact on what already ships. **The de-duplication happens after this branch merges with `main`**,
  which is what gates this item -- not a release of the new crate. Decide then: either make the enumeration
  crate consume it and delete the older copy, or keep both and record what makes them genuinely separate. Do **not** let the duplication become permanent by nobody circling back -- that is
  the failure mode the duplicate-then-decide procedure exists to prevent, and it is why this item is here
  rather than only in a design note. Until it is settled, a fix to either copy must be applied to both.
  See [crates/windows-namespace-request-sys/DESIGN-NOTES.md](crates/windows-namespace-request-sys/DESIGN-NOTES.md)
  -> `D-9`.
