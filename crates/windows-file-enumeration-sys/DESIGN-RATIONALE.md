# Design rationale: windows-file-enumeration-sys (Tier 2)

This file records why the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were
reached. The complete originating discussion is in the workspace
[design session](../../design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md),
and cross-component rationale is in the workspace
[DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md). Tier 1 remains authoritative.

## Why flat enumeration is a separate crate

Enumeration and traversal are different dimensions. This layer owns one native
directory handle, bounded delivery for its entries, and one terminal outcome.
Traversal owns recursion, breadth/depth policy, descendant admission, and
tree-wide scheduling. Keeping traversal above the flat primitive preserves a
level platform that can serve direct one-directory consumers and multiple future
traversal policies.

## Why the crate depends on three sibling platform layers

Opening the directory under the submitter's effective security context requires
`windows-impersonation-token-sys`; inferring context later on a worker would use
the wrong identity. `windows-threadpool-sys` provides the coalesced SQ doorbell
and finite per-enumeration work callbacks. `wtf-string` preserves native-width
WTF-16 paths and names without conversion loss. Each dependency owns its
specified primitive rather than incidental behavior this crate could reproduce.

## Why the native engine is synchronous

`GetFileInformationByHandleEx` with `FileIdExtdDirectoryRestartInfo` and
`FileIdExtdDirectoryInfo` provides caller-owned staging and richer metadata than
find-first/find-next. It has no documented overlapped, APC, event, completion
routine, or IOCP form. Each refill therefore runs as potentially blocking
`ThreadpoolWork`, with at most one refill and finite parsing work per callback.
Direct `Nt*` APIs were rejected because this layer is defined on documented
Windows contracts.

## Why these direct windows-sys features are sufficient

`Win32_Foundation` provides native handles, errors, and metadata value types.
`Win32_Globalization` provides `CompareStringOrdinal` for crate-owned
non-linguistic name comparison.
`Win32_Storage_FileSystem` provides directory open/enumeration APIs and record
layouts. `Win32_System_Threading` provides signaling and resetting the lazily
created manual-reset CQ event. Event creation itself comes from the safe
`WaitableHandle` constructor in `windows-threadpool-sys`, so this crate does not
need direct Security bindings. The enumeration API is not overlapped, so direct
System IO bindings would claim a capability this design intentionally excludes.

## Why paths are snapshotted before submission

Opening on the caller thread would defeat asynchronous enumeration, but deferring
relative-path interpretation to a worker would make the target depend on the
process current directory at an unrelated later instant. Resolving ordinary path
forms while the request is built separates string resolution from the privileged
open: `GetFullPathNameW` produces the caller-time absolute snapshot, then
`CreateFileW` opens that snapshot under the captured impersonation token.

An ordinary path returned by `GetFullPathNameW` is still opened by
`CreateFileW`, whose long-path acceptance depends on the host's manifest and
system policy. Depending on that policy would surrender the crate's behavior to
its consumer. The contract therefore keeps ordinary input and resolved paths
within `MAX_PATH` and requires a fully qualified `\\?\` input for long paths.
Only `\\?\` disables Win32 parsing and remains verbatim. `\\.\` inputs retain
ordinary normalization, are included in caller-time resolution, and stay in the
device namespace if that is what `GetFullPathNameW` returns.

## Why native order is not stabilized

The documented enumeration API does not promise a filesystem-independent order,
and `FILE_ID_EXTD_DIR_INFO::FileIndex` is undefined on NTFS. Sorting would require
retaining a whole directory, destroy streaming backpressure, add a collation
choice unrelated to enumeration, and delay the first result. The crate therefore
preserves the useful fact it can guarantee -- per-request delivery in native
record order -- while specifying that order as unstable. Traversal or user
interfaces that require sorting can do so at the layer that owns that policy.

## Why failure is embedded in the terminal

Embedding an enumeration failure in its reserved terminal avoids a two-record
failure protocol that could deadlock when all unreserved CQ data slots are
occupied. An adapter that wants separate error and failed-terminal events can
expand that one terminal after consuming it. `ERROR_NO_MORE_FILES` is different:
it is the usual clean exhaustion signal and maps to `Completed`. The initial
restart query can instead report `ERROR_FILE_NOT_FOUND` when it has no first
record, so that code is also clean exhaustion only at that exact query phase.
Keeping the mapping phase-specific prevents a failed directory open from being
mistaken for an empty directory.

## Why inline metadata is always returned

`FILE_ID_EXTD_DIR_INFO` pays for the name, attributes, reparse tag, sizes, four
times, extended-attribute size, and 128-bit ID in the same record. Omitting any of
those fields would not avoid a syscall or make the native record smaller, and it
would make the platform less level. Keeping timestamps as signed Windows
100-nanosecond ticks avoids the overflow, saturation, precision loss, and sentinel
policy inherent in eager Unix conversion.

Volume qualification is different: it requires a separate `FileIdInfo` query.
The three identity modes let a caller avoid that work, preserve Globazog's
best-effort unknown-identity behavior, or demand a complete volume-plus-ID
invariant. The raw 128-bit identifier remains available in every mode, but only
the volume-qualified pair is globally meaningful.

## Why the predicate is data rather than code

An owned predicate can cross the SQ, be validated before acceptance, and execute
inside a Windows thread-pool callback without invoking arbitrary client code.
Arbitrary closures would introduce panic, latency, and reentrancy policy into the
cadence path. A flat conjunction matches Globazog's existing metadata-leaf model,
allows ranges by repeating comparison clauses, and remains bounded and
serializable. The non-exhaustive outer enum preserves room for a future
expression-tree family without changing the request container.

The crate owns name semantics rather than delegating a wildcard string to a
filesystem. Compiled single-segment tokens preserve unpaired surrogates, and
`CompareStringOrdinal` supplies the Windows non-linguistic case behavior selected
by the contract. Explicit sensitive and insensitive modes are preferable to
querying per-directory case sensitivity: predicate matching is a caller choice,
and `FileCaseSensitiveInfo` would both add a newer OS dependency and conflate
"which names may coexist" with "how this query wants to compare them."

Zero attribute masks are rejected because both "all zero bits are set" and "all
zero bits are clear" are mathematically true. Accepting them would turn a likely
caller mistake into an invisible match-all clause. Empty name-pattern sets are
rejected for the same reason: negating one would also be an invisible match-all.

## Why unsupported and oversize cases fail explicitly

Falling back from extended directory records to find-first/find-next would keep
names but lose contract fields. That is not graceful degradation; it is a
different platform. The crate instead maps the unsupported-operation error codes
seen on a well-formed query to one typed failure and retains the raw code for
diagnostics and future classification changes.

The fixed buffer is equally intentional. Silent growth would make a configured
memory bound advisory, and retrying a failed directory query would depend on
undocumented cursor behavior. `ERROR_MORE_DATA`,
`ERROR_INSUFFICIENT_BUFFER`, and `ERROR_BAD_LENGTH` therefore mean that one
record exceeded the effective capacity. A typed terminal lets the caller retry
with a larger explicitly chosen request without hiding allocation or replay.

## Why a worker reports rather than acts (D-16, D-17)

The first shape of this was the obvious one: a worker finishes its own
enumeration, removing the registry entry and delivering the terminal. It does not
survive contact with the thread pool. Removing the entry drops that
enumeration's work object, and this workspace's `ThreadpoolWork::drop` waits for
outstanding callbacks before closing -- so the worker waits for itself, forever,
and would then free the closure it is still running inside.
`CloseThreadpoolWork` is legal from within a callback, but the wait and the
synchronous context free are not, so `DisassociateCurrentThreadFromCallback`
does not rescue it either.

The fix is not a safer release path but a smaller worker. A worker's outputs are
records and one retirement report; the servicer, which already had sole authority
over the registry, does the releasing. That also removes a liveness defect that
had nothing to do with deadlock: abandonment used to drop every enumeration's
work object on the servicer, waiting for in-flight *and queued* directory
queries, so the cheap teardown path stalled the session's only drain authority
behind an unbounded network read.

Keeping thread-pool objects out of the registry entirely -- one session-owned
engine object plus a ready set -- is what makes that guarantee structural rather
than a rule someone has to remember. It costs a claim protocol, which the
single-flight rule needs anyway, and saves modifying a published sibling crate
purely to make a per-enumeration object releasable from the wrong place.

## Why retirement is reserved like cancellation (D-18)

A worker that cannot report itself finished leaks its enumeration: the registry
entry, its token, handle, and buffer stay until the session dies. Retirement is
therefore exactly as unable to fail as cancellation is, and gets the same
treatment -- a slot claimed at admission, before the enumeration is allowed to
start. The visible cost is one more reserved slot per live enumeration and a
minimum submission capacity of four rather than three, which is the honest price
of a control message that must always fit.

## Why the native buffer belongs to the enumeration, not the request (D-19)

The contract first put allocation in request construction, which reads well until
`EnumerationRequest` is examined: it is `Clone`, it is `Eq`, it may be submitted
more than once, and a refused begin hands it back for retry. A request owning a
64 KiB buffer would make cloning it an infallible large allocation, make equality
compare scratch space, and make a traversal layer pay that allocation for every
begin the rings refuse.

Admission is where the buffer belongs, beside the token capture and the two
reservations, because admission is already the boundary where everything that can
fail does so on the caller's own thread. The allocation must be fallible and
8-byte aligned, and neither comes free: the ordinary growable vector aborts the
process on allocation failure and guarantees only byte alignment.

## Why a quantum has two independent budgets, not one (D-20)

A record count alone under-bounds a quantum whenever per-record cost is not
uniform: a handful of records with very long names, or a predicate clause that
does real comparison work, can cost far more than a budget sized for the
ordinary case expects. A time budget alone under-bounds it the other way: a
directory whose predicate rejects everything is cheap per record, so a
time-only budget would let it examine an unbounded number of records before
the clock ran out, which is exactly the "reject-all predicate monopolises a
worker" failure mode the record budget exists to prevent. Neither bound
substitutes for the other, so a quantum stops the moment either is spent.

`Instant::now()` was measured against QPC-backed timers as cheap enough to
call once per record without mattering next to the record parse it accompanies,
so the time budget is checked on every iteration rather than every Nth one --
a periodic check would only complicate the loop for a cost that is not there
to save.

Backpressure from a full completion ring is deliberately not folded into
either budget. A quantum that cannot deliver parks rather than yields, because
resubmitting immediately into a ring that is still full would burn a worker on
a callback that can only fail again; parking instead waits for the one event
that can create room; a receiver taking a record. What *is* worth avoiding on
every retry is repeating the parse, the entry construction, and the predicate
evaluation for a record whose disposition never changes while it waits --
hence `EngineState::awaiting_room`, a one-bit memory of "this exact record
already matched and is only waiting on room," checked with
`CompletionRing::has_data_room` before any of that work is repeated.

## Why directory-ness is checked rather than inferred (FE-8)

Two things the filesystem taught us during FE-8, both of which the contract had
described in a way that reads correctly but implements wrongly.

The first: `FILE_LIST_DIRECTORY` is the same bit as `FILE_READ_DATA`, so opening
an ordinary file with it succeeds. Left to the first refill, "you named a file"
would arrive as one of the same codes that mean "this filesystem does not support
extended directory information" -- and the crate would report a caller's mistake
as a capability failure, which is precisely what the unsupported-class
preconditions exist to prevent. Checking `FILE_ATTRIBUTE_DIRECTORY` on the opened
handle costs one query per enumeration and makes the distinction structural.

The second: an empty subdirectory is not an empty listing. It contains `.` and
`..`, so it returns a batch and exhausts on its *second* query. The
first-query-empty rule is still needed and still correct -- an empty volume root
reaches it -- but stating it as "an empty directory completes immediately" would
have been wrong, and a test written to that phrasing fails against any real
filesystem. The rule is therefore stated in terms of which query reported the
code, which is what it always depended on.

## Why this remains a Globazog replacement

Globazog's existing Windows backend demonstrates the minimum viable native
surface: leaf-name fidelity, inline type/reparse/attribute/size/time metadata,
optional volume-qualified 128-bit identity, partial-result error reporting, and
no per-entry open. The new crate adds bounded asynchronous transport and correct
submitter impersonation around that same capability. Treating the existing
backend as an acceptance witness prevents the lower layer from becoming easier
to implement by quietly forcing a second metadata path back into traversal.

## FE-14: discharging the Globazog replacement gate

D-15's gate demands a demonstration, not a promise, so FE-14 built one: a
hand-reconstructed adapter under
[tests/integration/globazog_adapter/](../tests/integration/globazog_adapter.rs)
that reimplements Globazog's real Windows backend's public shape and
translates its predicate vocabulary, then exercises it end-to-end against the
live native engine in this crate. The adapter is deliberately a
reconstruction rather than a dependency: Globazog is meant to consume this
crate, never the reverse, so its value types (`DirEntry`, `DirScan`,
`EntryFailure`, `EnumPlan`, `FileId`, the `Leaf`/`Token`/`Segment` predicate
vocabulary) are copied field-for-field from Globazog's real source at
`MikeGrier/globazog-rs` commit `55a0b1aec7a93051a675852636ab41a6437440fb`
(`crates/globazog/src/{sys,sys/win,predicate,syntax,syntax/decode,error}.rs`),
each with a doc comment citing the exact file the shape came from so a future
maintainer knows to re-diff against the real repo rather than trust this copy
blindly.

Every property D-15 lists is exercised through the adapter against the live
engine: native name/path fidelity (including UTF-16 round-tripping through
Globazog's own `decode_utf16`, ported verbatim to preserve its unpaired-
surrogate handling), file/directory type, reparse status and tag, raw
attributes, logical size, all four timestamps converted through Globazog's
own FILETIME-to-Unix-nanos formula, and 128-bit volume-qualified identity.
The predicate translation table covers every `Leaf` variant that a
one-directory backend can answer, including the `EntryType::Other` case --
Windows has no third entry kind, so a non-negated `IsType{ty:Other}`
translates to a self-contradictory attribute-clause pair (directory bit set
and clear in the same conjunction, which can never hold) rather than being
silently dropped. `Leaf::Depth` is excluded on purpose: it is a property of
Globazog's own recursive multi-directory traversal engine, not something a
single-directory backend is ever asked to answer, so translating it is out of
scope by construction rather than an oversight.

Two properties could not be proven organically in this environment, and both
are handled by narrowing the proof rather than skipping it, matching the
precedent FE-13's `capability.rs` set for untestable capability gaps.
A genuine live late-failure -- a `TerminalOutcome::Failed` arriving after
some entries were already delivered -- needs a filesystem or redirector fault
this environment cannot manufacture on demand. The adapter's translation of
that outcome is instead extracted into a pure function, `finish_scan(entries,
outcome) -> io::Result<DirScan>`, and unit-tested directly against hand-built
`TerminalOutcome::Failed` values (both with and without prior entries),
proving the error-plus-partial-listing contract without needing a live fault.
"No path opens an individual entry" -- inherited from D-3 -- is proven
empirically instead: a directory junction whose target does not exist is
still listed successfully by the batched directory query, which would not be
true if the adapter (or the engine beneath it) resolved each entry
individually.

The demonstration is 53 integration tests plus the reused unit-level
`finish_scan` proofs, stable across ten consecutive runs with no leaked
scratch state, and it satisfies D-15 without turning Globazog into an actual
dependency of this workspace.

## Why reporting `Parked` re-checks room instead of trusting a wakeup (D-21)

CI found what local runs never did: `scale::a_completion_ring_far_smaller_than_
the_directory_still_delivers_everything` (500 entries, the smallest ring the
contract allows) hung for the full receive timeout on GitHub Actions' hosted
Windows runner, twice in a row, at essentially the same elapsed time both
times. The first hypothesis was environmental slowness -- the runner's few
vCPUs and the process-default thread pool's conservative thread-injection
heuristic under the full workspace's concurrent test load -- and the fix
tried first was simply a larger receive timeout (30s to 120s). That made no
difference: the third run still hung for the full 120s. A deterministic
timeout being hit exactly, twice, at two different ceilings, is not what
genuine contention-driven slowness looks like (which would vary run to run);
it is what a permanent stall looks like.

Re-reading `report_quantum`'s `QuantumOutcome::Parked` arm against
`Receiver::recv`'s `resume_parked` call exposed the real defect: `advance`
decides `Parked` with no lock held (by design -- D-3 -- since a quantum may
block on a directory query), and only afterward does `report_quantum` take
the registry lock to write `state.parked = true`. Nothing stops a receiver
from draining the very record the worker was blocked on, and calling
`resume_parked`, inside that gap. `resume_parked` reads the registry
correctly and finds nothing parked, because nothing was parked yet by any
definition it can observe -- and once the ring runs dry, nothing will ever
call `resume_parked` again. The worker then writes `parked = true`
unconditionally, over room that has already existed for however long the gap
lasted, and the enumeration is stranded: exactly the deterministic hang
observed, at whatever the timeout happened to be.

This is an ordinary missed-wakeup race, not specific to Windows or this
crate's shape, but two things made it fail exactly here and only on CI: the
smallest ring forces one park/resume round trip per entry rather than
amortizing many entries per round trip, so five hundred entries meant five
hundred chances to hit a gap that is normally a handful of instructions wide;
and GitHub's hosted runner's thinner, more contended scheduling (fewer vCPUs,
more preemption from the whole workspace's concurrent tests sharing the
process-default pool) widens that gap enough to make hitting it at least once
in five hundred tries reliable rather than vanishingly rare. A dev machine
with more headroom can run the same scenario thousands of times without ever
landing in the gap, which is exactly why FE-12's local stability loop (30
consecutive runs) never caught it.

The fix -- re-checking `has_data_room` inside `report_quantum`, under the
same registry lock `resume_parked` reads under, and resuming immediately
instead of parking when room is already there -- was chosen over two
alternatives. Making `resume_parked` retry (poll again after a short delay,
in case the parked flag was about to be set) would still leave a race, just a
narrower one, and would trade a correctness bug for a timing-dependent
band-aid of exactly the kind this investigation started by mistakenly
reaching for. Moving the room check inside the registry lock everywhere
(checking room every time any registry field is touched) would couple the
completion ring's locking to the registry's for no benefit beyond this one
call site. Re-checking only at the one point that writes `parked = true` is
the minimal, provably sufficient fix: whichever of the two racing sides --
the worker's write, or the receiver's drain-and-read -- runs second under
that single lock is now the side that observes the other's update, so no
interleaving of the two events can lose the wakeup.

The race was reproduced deterministically, not just inferred, using the
existing state-machine model (`model.rs`): script filling the ring to
capacity, claiming the enumeration, draining everything queued with
`Op::DrainReceiver` -- which is what calls `resume_parked` while the
worker's claim is still outstanding -- and only then reporting `Parked`.
Temporarily reverting the fix and running that scenario reproduces the
stranding exactly (`model.ready()` stays `0` forever); with the fix, the
enumeration resumes (`model.ready()` becomes `1`) as soon as `Parked` is
reported, because room was already there. The receive-timeout increase tried
first was reverted once the actual defect was fixed: the original 30s ceiling
was never the problem, and inflating it for every integration test would only
have made a future genuine hang in any other test take four times as long to
be reported as a failure.
