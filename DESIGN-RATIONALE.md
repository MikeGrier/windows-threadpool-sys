# Design rationale: captured impersonation and asynchronous file enumeration (Tier 2)

This file records why the cross-component decisions in
[DESIGN-NOTES.md](DESIGN-NOTES.md) were reached. The raw discussion is in
[design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md](design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md).
Tier 1 remains authoritative.

## Why captured impersonation is its own crate

The enumeration open must occur asynchronously, but directory access is determined
by the submitter's effective security context. A thread-pool worker cannot recover
that context later: modern `SubmitThreadpoolWork` dispatch does not flow the
submitter's thread token.

Windows demonstrates that the capability is legitimate rather than application
policy. Legacy `QueueUserWorkItem` has `WT_TRANSFER_IMPERSONATION`, which causes the
callback to use the submitting thread's current process or impersonation token. The
modern object-based thread-pool API has no corresponding callback-environment flag,
so users of the modern API must build the transfer themselves.

Microsoft WIL supplies useful C++ precedent. Its
`open_current_access_token_nothrow` opens the thread token or falls back to the
process token, and `impersonate_token_nothrow` saves the current thread token,
temporarily applies another, and restores the exact saved state. Its restoration
path fails fast. The Microsoft Rust crates expose the Win32 calls and types but do
not compose them into a transportable safe abstraction.

The third-party `windows-token` crate was considered and rejected. At the time of
this design it was new and pre-alpha in its own documentation, did not implement
`OpenThreadToken`, restored with `RevertToSelf` rather than restoring a prior nested
impersonation token, and swallowed restoration failure in release builds. Those are
the exact guarantees this workspace needs.

Putting the helper directly in `windows-file-enumeration-sys` would make traversal
either depend on an enumeration implementation for an orthogonal security
primitive or reproduce the same sensitive code. Putting it in
`windows-threadpool-sys` would add security-token dependencies and policy to the
level thread-pool layer even though token transport is useful across dispatch
mechanisms. A separate `windows-impersonation-token-sys` crate preserves
both boundaries.

The future traversal layer creates a second concrete need. It must capture once
when traversal is submitted and reuse that context when it schedules descendant
directory enumerations later. Capturing separately inside each enumeration helper
would capture arbitrary workers and reintroduce the Globazog bug.

## Why the public token type is crate-owned

The workspace normally reuses Microsoft native types, but a `HANDLE` is only a raw
identifier. It does not express ownership, token rights, mutability restrictions,
cross-thread transport, capture timing, scoped application, exact restoration, or
what happens when restoration fails. `ImpersonationToken` exists to own those
additional invariants; internally it remains an implementation over
`windows-sys`.

The use-site API should make the safe path easy. An enumeration SQ offers an
ordinary begin helper that captures the current context and publishes the
token-bearing begin message as one operation. It also offers an explicit-token form
for traversal and other orchestrators that need to reuse a previously captured
context.

## Why restoration failure panics

Failure to restore an arbitrary application thread would already be serious.
Failure on a shared Windows thread-pool worker is worse: later unrelated callbacks
could execute under the wrong identity. Returning an error to the enumeration
consumer does not repair that worker, and swallowing the failure turns a known
security breach into process-global nondeterminism. The guard therefore panics
from `Drop` when `SetThreadToken` fails. Restoration still runs during Rust unwind;
if restoration itself then panics, Rust's double-panic behavior aborts the process.

## Why enumeration uses a bounded SQ and CQ

The session needs asynchronous communication in both directions without calling
client code from the cadence path. The SQ gives every begin, cancellation, and
abandonment operation one ordered ingress path. The CQ gives entries and terminal
outcomes one ordered egress path. `ThreadpoolWork` acts only as a coalesced SQ
doorbell and drain authority; it is not an enumeration worker.

Bounded rings make resource use explicit, but cancellation cannot be allowed to
fail when ordinary traffic fills the SQ. Every accepted enumeration therefore owns
a reserved future cancellation slot, and the session owns one abandonment
reservation. The CQ similarly reserves each accepted enumeration's terminal slot,
while retaining at least one unreserved data slot so reservations cannot deadlock
all useful progress.

## Why `GetFileInformationByHandleEx` replaces find-first/find-next

Globazog already uses `GetFileInformationByHandleEx` with
`FileIdExtdDirectoryRestartInfo` and `FileIdExtdDirectoryInfo`. The caller-owned
buffer exposes the refill boundary, retains wider metadata than
`WIN32_FIND_DATAW`, and can be held across callbacks while a bounded CQ is full.
That directly solves the hidden-buffer problem in `FindNextFileW`.

The API is synchronous and has no documented overlapped, APC, event, completion
routine, or IOCP form. A potentially-long-running `ThreadpoolWork` callback is
therefore the documented Windows-native bridge. Limiting each callback to one
refill and finite parsing work prevents an enumeration from monopolizing workers
without reaching into unsupported `Nt*` APIs.

## Why the enumeration surface resolves paths and preserves native order

Resolving ordinary paths before SQ acceptance snapshots relative-path meaning
without opening under the submitter thread. Opening the resulting ordinary path
can depend on executable and system long-path policy, so the crate deliberately
caps ordinary forms at `MAX_PATH`; callers use a fully qualified, verbatim
`\\?\` path for long input. `\\.\` retains ordinary Win32 normalization and is
snapshotted like the other non-verbatim forms.

The native API supplies no stable ordering contract, and sorting would require a
whole-directory staging layer that defeats streaming bounded delivery. The crate
therefore preserves order within each native record stream but labels it
unspecified. A higher traversal or presentation layer owns any sorting policy.

## Why metadata selection is limited to volume identity

The extended record supplies attributes, reparse tag, logical and allocation
sizes, extended-attribute size, four timestamps, and a 128-bit file ID together.
Dropping any of those fields saves no native work and would narrow the level
platform. The values remain native; in particular, signed 100-nanosecond Windows
timestamps avoid lossy epoch conversion.

Only volume qualification needs another query. Omitted, best-effort, and required
modes expose that real cost and capability boundary without per-entry opens.

## Why predicates and failures are crate-owned data

A validated data-only predicate can cross the SQ and run in a callback without
calling user code. A flat query-by-example conjunction maps the predicate leaves
already needed by Globazog, including native-name patterns, attribute masks, and
six-way size/time comparisons. Windows ordinal comparison defines case behavior
without handing wildcard semantics to a filesystem.

The same reasoning applies inside a session, to which of its own components may
act. A worker that finished its own enumeration would drop that enumeration's
thread-pool work object from inside that object's callback, waiting for itself
and then freeing the closure still running; and abandonment that released those
objects on the servicer would stall the session's only drain authority behind an
unbounded directory query. Making the worker a reporter, and keeping thread-pool
objects out of the registry entirely, removes both structurally rather than by
rule. Full details are in the enumeration crate's
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).

An accepted failure is embedded in the enumeration's one reserved terminal.
This retains exact native detail without requiring another CQ data slot. Extended
directory information that is unavailable fails explicitly instead of falling
back to a metadata-poorer API. Likewise, a record that cannot fit the caller's
fixed capacity produces a typed oversize failure; hidden growth would violate the
memory bound and retry would rely on undocumented cursor behavior. Normal
exhaustion includes `ERROR_NO_MORE_FILES` on any refill and
`ERROR_FILE_NOT_FOUND` only on the initial restart refill; phase specificity
keeps a failed open or late read from looking like success. Full details and
alternatives are in the enumeration crate's
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).

## References

- [`QueueUserWorkItem` and `WT_TRANSFER_IMPERSONATION`](https://learn.microsoft.com/windows/win32/api/threadpoollegacyapiset/nf-threadpoollegacyapiset-queueuserworkitem)
- [`SubmitThreadpoolWork`](https://learn.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-submitthreadpoolwork)
- [WIL token helpers](https://github.com/microsoft/wil/blob/master/include/wil/token_helpers.h)
- [`GetFileInformationByHandleEx`](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandleex)
