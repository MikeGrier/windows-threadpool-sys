# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M-T4 -- Teardown drains rather than cancels

Implements [Teardown drains rather than cancels](../../DESIGN-NOTES.md#teardown-drains), decided
2026-09-28. Forced by a measurement in the ring crate:
[closing-too-soon-after-the-disarm](../windows-ioring-sys/measurements/2026-09-28-closing-too-soon-after-the-disarm/README.md)
shows the cancelling form leaves the default pool unable to make its first worker, 10 failures in
20000 against 0 for the draining form.

- [ ] **M-T4.1** -- **DECISION TO RAISE, not to take: does `stop_and_drain` change, or gain a
  sibling?** It calls `cancel_pending` on `ThreadpoolWait`, `ThreadpoolTimer` and `PeriodicTimer`,
  so the method whose name says "drain" is the one that cancels. Changing it matches the name and
  fixes every caller at once; it is also a silent behavioural change to a published crate, turning
  a call that discarded queued callbacks into one that runs them -- a caller relying on teardown
  being prompt would start blocking. Adding `stop_and_run_out` beside it keeps the old behaviour
  reachable at the cost of two methods a reader must tell apart. **Decide before M-T4.2**, because
  `PeriodicTimer::drop` calls `stop_and_drain` and the answer changes what that drop does.

- [ ] **M-T4.2** -- **Drain instead of cancel in the three teardowns that do not.** `Drop` for
  `ThreadpoolWait` and `ThreadpoolTimer` calls `cancel_pending`; `PeriodicTimer::drop` reaches it
  through `stop_and_drain`. `ThreadpoolWork` and `ThreadpoolIo` already drain, so this removes an
  inconsistency rather than introducing a policy. The suppression is already raised before the
  drain in every one of these paths, which is what makes draining safe: a callback that runs asks
  to re-arm and the ask is refused. Guard it by asserting the callback **ran** -- the existing
  tests assert quiescence, which both forms satisfy.

- [ ] **M-T4.3** -- **Report an obligation discharged in `Drop`.** Record on the trace that the
  drain happened in `Drop` rather than having been done earlier, so a capture distinguishes
  "teardown was paid for deliberately" from "teardown happened wherever the value went out of
  scope". Needs a flag the early-discharge method sets and arming clears.

- [ ] **M-T4.4** -- **DECISION TO RAISE: the opt-in fail-fast, and what it does while panicking.**
  Off by default. How it is selected -- environment variable, constructor option, process-wide
  setter -- is open, and so is its behaviour when the object is dropped on an already-unwinding
  path, where a panic aborts. The root
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md#a-panicking-callback-aborts-rather-than-being-contained)
  already chose abort for a panicking callback, which is the nearest precedent.

- [ ] **M-T4.5** -- **Decide whether `CleanupGroup` follows.** Six sites carry both forms, it has
  its own ownership model, and it was not measured. Gated on M-T4.2 landing.

- [ ] **M-T4.6** -- **Re-measure the ring reproducer against the drained build.** The 20000-run
  arms were a hand-rolled model of the teardown, not this crate's code. Confirm the real
  `EventDelivery` path reaches 0 where it currently reaches ~10, with a live control in the same
  session, before `M26.9` is called closed in
  [windows-ioring-sys](../windows-ioring-sys/UNRESOLVED-TEST-FAILURES.md).