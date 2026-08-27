# Design notes: windows-impersonation-token-sys (Tier 1)

This file is authoritative for the crate. The cross-component decision is also
recorded in the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md), its historical
reasoning is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), and implementation is
scheduled by M4 in the workspace [CHECKLIST.md](../../CHECKLIST.md).

## Intent

Provide one memory-safe Rust abstraction for capturing the calling thread's
effective Windows impersonation state, carrying it to another thread, applying it
for a bounded operation, and restoring the exact prior state.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **The crate is an independently publishable platform layer.** File enumeration and later traversal both need the same security-context transport, while the primitive is independent of either consumer and of the Windows thread pool itself. |
| <a id="d-2"></a>D-2 | **The public abstraction is the opaque owned `ImpersonationToken`.** A raw `HANDLE` cannot express ownership, capture timing, required rights, immutability, cross-thread transport, or scoped restoration. The implementation still uses the corresponding Microsoft `windows-sys` APIs and types. |
| <a id="d-3"></a>D-3 | **Capture is explicit and synchronous on the submitting thread.** No worker may infer or recapture the submitter's context. A consumer may capture once and reuse the resulting token for later operations. |
| <a id="d-4"></a>D-4 | **Application restores the exact prior thread-token state.** This includes nested impersonation and unwind paths. Restoration failure fails fast because returning a shared worker under an unknown identity is a process security failure. |
| <a id="d-5"></a>D-5 | **This is not a general security-helper collection.** New surface belongs only when it is required to preserve the capture, transport, apply, or restore lifecycle of `ImpersonationToken`. |
| <a id="d-6"></a>D-6 | **Capture duplicates the effective token into an immutable `TOKEN_IMPERSONATE`-only handle.** The thread token is opened with `OpenAsSelf` and only query/duplicate rights; its identification, impersonation, or delegation level is preserved. No-thread-token context falls back to a process-token snapshot at `SecurityImpersonation`. Anonymous context is rejected synchronously. Clones share ownership of the captured handle rather than acquiring broader rights. |

## Publication boundary

The crate is Windows-only, independently versioned, and published to crates.io.
Its manifest carries the workspace's normal metadata, Windows docs.rs target, and
only the `windows-sys` features needed by the token lifecycle. Release automation
is registered at the workspace level.
