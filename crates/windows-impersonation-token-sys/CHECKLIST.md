# Checklist: windows-impersonation-token-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). Completed work is in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## IT-M-inf -- The horizon (ungated)

- [ ] **IT-inf.1** -- **`ImpersonationToken` holds its captured token as a
  `SharedHandle` instead of an `Arc<OwnedHandle>`.** Raised 2026-10-07 in the
  durable-ioring design session, where
  [win-shared-os-owned-handle](../win-shared-os-owned-handle/README.md)'s
  `SharedHandle` became the workspace's one shared owner of a handle (its
  `SH-1+.1`). Deferred by the engineer; nothing gates it. The change is to the private field only:
  - **The handle stays unexposed.** [D-6](DESIGN-NOTES.md#d-6) and the public
    docs promise that no safe API exposes the captured handle, so no
    `AsHandle`, `AsRawHandle`, `From` or accessor reaches the surface.
  - **No equality appears.** [D-8](DESIGN-NOTES.md#d-8) refuses equality, so
    `SharedHandle::same_handle` stays internal, as `Arc::ptr_eq` is today.
  - **What it removes:** the hand-written `Clone`, which a derive replaces.
  - Not a breaking change; the public surface is unchanged.
