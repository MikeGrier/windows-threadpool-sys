# Design rationale: windows-impersonation-token-sys (Tier 2)

This file records why the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were
reached. The complete originating discussion is in the workspace
[design session](../../design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md),
and the cross-component rationale is in the workspace
[DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md). Tier 1 remains authoritative.

## Why this is a separate crate

Legacy `QueueUserWorkItem` can flow the current token through
`WT_TRANSFER_IMPERSONATION`; the modern object-based Windows thread pool offers no
equivalent. Microsoft WIL provides C++ building blocks for opening the current
token and exactly restoring a prior token, while the Microsoft Rust crates expose
the native APIs rather than a complete transportable abstraction.

File enumeration needs to open a directory under the submitter's context. Future
recursive traversal must capture once at traversal submission and reuse that same
context for descendant opens. Keeping the primitive in either consumer would
misplace an independently useful security invariant; placing it in
`windows-threadpool-sys` would add security policy to the level thread-pool layer.

## Why the scope is narrow

The earlier plural `helpers` name was rejected because it invited unrelated
security utilities to accumulate here. The crate is named after its owned
capability. Token inspection, privilege adjustment, authentication, authorization,
and general account management do not belong merely because they use nearby Win32
APIs.

## Why exact restoration is mandatory

`RevertToSelf` restores process identity, not an arbitrary prior nested
impersonation token. A shared thread-pool worker may have a prior state that must
survive the bounded operation. If restoration fails, reporting an ordinary error
does not repair the worker; later unrelated callbacks could execute under the
wrong identity. The guard therefore panics from `Drop`.

## Why capture duplicates and narrows the token

The source thread handle is opened with `OpenAsSelf` so an identification-level
caller can still capture its own token while the access check uses the process
context. Query access is needed only to read that token's impersonation level,
and duplicate access is needed only to create the independent snapshot. The
source handle is then closed.

A thread with no token is running as the process. Its primary process token has
no impersonation level to preserve, so capture duplicates that security context
as `SecurityImpersonation`: sufficient for local worker operations without
claiming delegation semantics the source did not have. An existing thread
token's identification, impersonation, or delegation level is passed through to
`DuplicateTokenEx` unchanged.

The captured handle requests only `TOKEN_IMPERSONATE`, which is all scoped
application needs. Null security attributes keep it non-inheritable. Sharing that
immutable owned handle through clones avoids both borrowed-handle lifetime
hazards and acquisition of duplicate or adjustment rights after capture.

## Why application is closure-only

A public RAII guard could be passed to `mem::forget`, which is safe Rust and would
leave a shared worker under the captured identity indefinitely. The public
`with_impersonation` operation keeps its guard private, so every ordinary return
restores before control reaches the caller and every unwind runs the guard's
destructor.

Before applying the captured token, the guard opens the current thread token with
only `TOKEN_IMPERSONATE`; `ERROR_NO_TOKEN` is recorded as explicit process
context rather than as an error. `OpenThreadToken` opens another handle to the
same token object; it does not duplicate or normalize the token. Restoration
passes that saved handle directly back to `SetThreadToken`, preserving
identification and delegation levels. Explicit process context is restored by
passing a null token.

The guard contains an `Rc` marker solely to make it `!Send` and `!Sync`, so the
saved state can only be restored on the thread where it was acquired. If that
restoration call fails, `Drop` panics rather than dropping the saved handle and
returning a worker with an unknown effective identity. If this happens while
another panic is already unwinding through the guard, Rust aborts the process.
