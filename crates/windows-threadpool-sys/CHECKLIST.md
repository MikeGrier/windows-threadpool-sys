# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M17 — Custom-close owner for non-`CloseHandle` wait targets

- [ ] **M17.1** — Let `ThreadpoolWait` own a wait target whose close routine is **not** `CloseHandle`. Today
  `WaitableHandle` wraps a std `OwnedHandle`, so `ThreadpoolWait` always closes its handle with `CloseHandle`
  on teardown (see [src/wait.rs](src/wait.rs) `Drop`). Add a seam — e.g. `WaitableHandle::assume_waitable_with(raw,
  closer)` or a small `WaitClose` owner — so the caller supplies the close function (for a
  `FindFirstChangeNotification` handle, `FindCloseChangeNotification`), and `ThreadpoolWait` drains the wait
  **before** invoking it exactly once. Keep the existing `OwnedHandle` path as the default. Unit-test that the
  custom closer runs exactly once and only after the wait is drained (direct `ThreadpoolWait::drop`).

- [ ] **M17.2** — Propagate the custom closer through the `CleanupGroup` path. `CleanupGroup::create_wait`
  moves the owner out via `ThreadpoolWait::into_parts` and adopts it as a boxed `OwnedHandle` freed with
  `CloseHandle` (see [src/cleanup_group.rs](src/cleanup_group.rs)), so a coarse handle in a group would be
  closed with the wrong routine. Carry the closer through `into_parts` / `WaitMember` / the adopted resource
  so the group release invokes it (after the group drains the wait) rather than `CloseHandle`, preserving the
  existing `OwnedHandle` default. Unit-test the group-release teardown path.

- [ ] **M17.3** — Integration: exercise **both** teardown paths — direct `ThreadpoolWait::drop` and
  `CleanupGroup` release (with and without `cancel_pending`) — and assert the custom closer runs exactly once,
  and only after the wait is drained, for each.

  > **➡ CROSS-COMPONENT HANDOFF:** completing M17 unblocks component `crates/windows-file-watcher` → M6 → M6.1
  > (the coarse `FindFirstChangeNotification` watcher). See [../windows-file-watcher/CHECKLIST.md](../windows-file-watcher/CHECKLIST.md).

When M17 completes, its items move to [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) and this file returns
to its closed state — no pending work, reopening only when new work (a new object type, a new capability, or
hardening) is planned.
