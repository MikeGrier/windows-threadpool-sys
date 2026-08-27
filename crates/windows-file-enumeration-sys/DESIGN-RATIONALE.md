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

## Why this remains a Globazog replacement

Globazog's existing Windows backend demonstrates the minimum viable native
surface: leaf-name fidelity, inline type/reparse/attribute/size/time metadata,
optional volume-qualified 128-bit identity, partial-result error reporting, and
no per-entry open. The new crate adds bounded asynchronous transport and correct
submitter impersonation around that same capability. Treating the existing
backend as an acceptance witness prevents the lower layer from becoming easier
to implement by quietly forcing a second metadata path back into traversal.
