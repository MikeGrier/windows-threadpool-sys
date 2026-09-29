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

- [x] **M-T4.1** -- **DECIDED 2026-09-28: `stop_and_drain` changes rather than gaining a sibling.**
  It always should have drained; the name was right and the body was wrong. This is a breaking
  behavioural change to a published crate -- a call that discarded queued callbacks will now run
  them and block until they finish -- and ships as one. No second method.

- [ ] **M-T4.2** -- **Drain instead of cancel in the three teardowns that do not.** `Drop` for
  `ThreadpoolWait` and `ThreadpoolTimer` calls `cancel_pending`; `PeriodicTimer::drop` reaches it
  through `stop_and_drain`, which M-T4.1 changes. `ThreadpoolWork` and `ThreadpoolIo` already
  drain, so this removes an inconsistency rather than introducing a policy. The suppression is
  already raised before the drain in every one of these paths, which is what makes draining safe:
  a callback that runs asks to re-arm and the ask is refused.

  **Guard it by asserting the callback RAN.** The existing tests assert quiescence, which both
  forms satisfy -- that is precisely why the wrong one survived this long. A guard that cannot
  tell a drained teardown from a cancelling one is not a guard for this change. Sabotage-verify by
  reverting each site to `TRUE` and confirming the new assertion fails.

  Breaking: `feat!` on this crate, with the changed `stop_and_drain` semantics named in the commit.

- [ ] **M-T4.3** -- **Report an obligation discharged in `Drop`.** Record on the trace that the
  drain happened in `Drop` rather than having been done earlier, so a capture distinguishes
  "teardown was paid for deliberately" from "teardown happened wherever the value went out of
  scope". Needs a flag the early-discharge method sets and arming clears.

- [ ] **M-T4.4** -- **DECISION TO RAISE, deferred by the engineer 2026-09-28: the opt-in
  fail-fast, and whether the wait is bounded at all.** Off by default. Selection -- environment
  variable, constructor option, process-wide setter -- is open, as is behaviour when the object is
  dropped on an already-unwinding path, where a panic aborts.

  **One bound is already fixed and constrains every answer: forward progress is not the
  alternative.** A teardown that cannot drain may abort, or fail fast by some other route, but it
  may not return to its caller having abandoned the callback. Bounding the wait is a question
  about which failure to take, never about whether to continue.

- [x] **M-T4.5** -- **`CleanupGroup` already complies; no change needed.** Queued on the strength
  of a grep showing both drain forms at eight sites; reading it, those are the *member* accessors
  (`WaitMember::wait` against `WaitMember::cancel_pending`, and the same pair for work and the two
  timers) -- caller-facing choices, not teardown. The group's own teardown is
  `release_members(false)` in `Drop`, which drains, and `close_members(cancel_pending: bool)` is
  already the explicit early-release method. A member never closes itself either, so nothing in
  that path issues a close behind a disarm.
- [ ] **M-T4.7** -- **Is a cleanup-group consumer already immune?** `M-T4.5` found the group's
  teardown drains and that a member never closes itself, so nothing on that path issues a close
  behind a disarm. Whether that makes it immune to the measured stall is **untested**. Run the ring
  reproducer with the trigger's wait owned by a `CleanupGroup` instead of standing alone, against a
  live control in the same session. A clean result would be independent evidence for the mechanism;
  a failing one would say the group close has the same hazard inside a single kernel call, which
  would be worth knowing before `M-T4.2` is trusted as the fix.
- [ ] **M-T4.6** -- **Re-measure the ring reproducer against the drained build.** The 20000-run
  arms were a hand-rolled model of the teardown, not this crate's code. Confirm the real
  `EventDelivery` path reaches 0 where it currently reaches ~10, with a live control in the same
  session, before `M26.9` is called closed in
  [windows-ioring-sys](../windows-ioring-sys/UNRESOLVED-TEST-FAILURES.md).