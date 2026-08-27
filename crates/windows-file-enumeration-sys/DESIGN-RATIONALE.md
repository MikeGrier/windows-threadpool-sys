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
`Win32_Storage_FileSystem` provides directory open/enumeration APIs and record
layouts. `Win32_System_Threading` provides signaling and resetting the lazily
created manual-reset CQ event. Event creation itself comes from the safe
`WaitableHandle` constructor in `windows-threadpool-sys`, so this crate does not
need direct Security bindings. The enumeration API is not overlapped, so direct
System IO bindings would claim a capability this design intentionally excludes.
