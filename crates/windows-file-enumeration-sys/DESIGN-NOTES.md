# Design notes: windows-file-enumeration-sys (Tier 1)

This file is authoritative for the crate. Cross-component decisions are also
recorded in the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md), historical
reasoning is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), and implementation
is scheduled by M5 and M6 in the workspace [CHECKLIST.md](../../CHECKLIST.md).

## Intent

Provide a memory-safe, asynchronous, flat one-directory enumeration layer over
documented Windows APIs. Preserve native path/name and metadata fidelity while
making submission, delivery, cancellation, and resource bounds explicit.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **The crate is an independently publishable flat-enumeration platform layer.** One request enumerates one directory; recursive traversal composes requests in a separate layer. |
| <a id="d-2"></a>D-2 | **The session uses a bounded multi-producer SQ and bounded single-receiver CQ.** Begin, cancellation, abandonment, and future controls enter through the SQ. Entries and exactly one terminal outcome leave through the CQ with an `EnumerationId`; reserved cancellation, abandonment, and terminal capacity make accepted control and terminal delivery lossless. |
| <a id="d-3"></a>D-3 | **The native engine uses `GetFileInformationByHandleEx` with caller-owned storage.** It uses `FileIdExtdDirectoryRestartInfo` followed by `FileIdExtdDirectoryInfo`, retains partially consumed buffers under CQ backpressure, and performs at most one synchronous refill per worker callback. Find-first/find-next, direct `Nt*` APIs, and IOCP integration are outside this design. |
| <a id="d-4"></a>D-4 | **Opening a directory uses the submitter's explicitly captured `ImpersonationToken`.** Ordinary begin captures before publishing its SQ message; an explicit-token form lets traversal reuse one captured context. Later refills use the already-open handle and do not impersonate. |
| <a id="d-5"></a>D-5 | **Native values remain native where they express the contract.** Paths and names use `wtf-string` for native-width WTF-16 storage, and Microsoft `windows-sys` value types remain public where no additional crate-owned invariant is required. |
| <a id="d-6"></a>D-6 | **The remaining v1 API decisions are deliberately unsettled until FE-2.** Path inputs, ordering, result/error/terminal taxonomy, metadata selection, predicate operators, timestamps, unsupported filesystems, and oversize records are queued in the workspace [CHECKLIST.md](../../CHECKLIST.md), so this scaffold does not pre-empt them. |

## Publication boundary

The crate is Windows-only, independently versioned, and published to crates.io.
It depends on `windows-impersonation-token-sys`, `windows-threadpool-sys`, and
`wtf-string` by both path and version. Its direct `windows-sys` surface enables
only Foundation and Storage FileSystem for enumeration plus System Threading for
the completion-ring event doorbell. Release automation is registered at the
workspace level.
