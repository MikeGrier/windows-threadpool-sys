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
wrong identity. The only safe response is fail-fast.
