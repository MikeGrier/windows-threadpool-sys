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

- [x] **M-T4.2** -- **Done 2026-09-28, and the item was wrong about two of its three sites.** All
  five teardown call sites now drain (`ThreadpoolWait::drop` and `stop_and_drain`,
  `ThreadpoolTimer::drop` and `stop_and_drain`, `PeriodicTimer::stop_and_drain`, which its `Drop`
  reaches). Only the wait's two are **verifiable**, and finding that out was most of the work.

  **The wait is a real defect and is guarded.** Two new tests use a private pool capped at one
  occupied thread, which is what makes "queued but not started" deterministic instead of a race,
  and assert the callback **ran**. Both sabotage-caught by reverting to `cancel_pending`. All 234
  pre-existing tests passed *before* the change, because they assert quiescence and both forms
  satisfy it -- which is exactly why the wrong form survived.

  **The timers' change is unobservable, measured rather than assumed.** A probe found that
  `SetThreadpoolTimer(NULL)` discards an already-queued tick where `SetThreadpoolWait(NULL)` does
  not: without a disarm the queued tick ran, with one it did not. Both timer teardowns disarm
  before draining, so no queued callback survives for the drain to run and the two forms are
  identical. The change was **kept** -- it is the form the rest of the crate uses and stays correct
  if that asymmetry ever changes -- and is documented as unobservable at both call sites rather
  than left looking verified. No guard was written that could not discriminate; instead the
  asymmetry itself is pinned by
  `disarming_cancels_a_queued_tick_which_a_waits_disarm_does_not`, and sabotaged by inverting it.
- [ ] **M-T4.3** -- **The discharge flag, and what it is allowed to decide.** Investigated
  2026-09-28; findings below are measured from the source, not proposed.

  **`ThreadpoolIo` already implements this whole pattern** and is the precedent rather than a gap.
  Its `Drop` reads `outstanding()`, and when that is non-zero it reports on a diagnostic channel,
  names the method the caller should have used, then makes the block terminate and blocks. So the
  work here is generalising one type's existing behaviour to the rest, not inventing it.

  **`io` needs no flag because it has a derived signal.** `outstanding()` is a real observable of
  whether rundown happened. The other four have no equivalent, which is what the flag is for.

  **The flag goes on the struct, not the context.** `Drop` holds `&mut self`; callbacks never read
  it; and the clearing methods take `&self` on `Sync` types, so it is an `AtomicBool` on the
  struct. Set by the synchronous close, cleared by anything that re-arms:

  | type | synchronous close sets it | these clear it |
  |---|---|---|
  | `ThreadpoolWait` | `stop_and_drain` | `arm` |
  | `ThreadpoolTimer` | `stop_and_drain` | `set_after`, `set_at`, `set_after_with_window` |
  | `PeriodicTimer` | `stop_and_drain` | `start`, `start_after` |
  | `ThreadpoolWork` | `wait` -- see M-T4.8, it has no named close | `submit` |
  | `ThreadpoolIo` | `run_down` | -- derived from `outstanding()`, no flag |

  **The flag must gate the REPORT, never the WORK.** This is the load-bearing finding. If `Drop`
  skips the drain because the flag is set, then a stale flag silently skips finalisation -- and it
  can be stale, because the root
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md#the-suppression-covers-the-callbacks-re-arm-not-an-external-one)
  already records that a concurrent external `arm` is not excluded from `stop_and_drain`. That
  would reintroduce exactly the hazard
  [the decision](../../DESIGN-NOTES.md#teardown-drains) forbids, in exchange for saving a drain on
  an already-quiescent object, which is nearly free. So: always drain; consult the flag only to
  decide whether to say anything.

- [ ] **M-T4.8** -- **DECISION TO RAISE: the synchronous close is not uniform, in name or in
  existence.** Found while investigating M-T4.3. Four shapes across five types:
  `stop_and_drain` on `ThreadpoolWait`, `ThreadpoolTimer` and `PeriodicTimer`; `run_down` on
  `ThreadpoolIo`; `close_members(cancel_pending: bool)` on `CleanupGroup`; and **nothing named as
  such on `ThreadpoolWork`**, whose `wait()` happens to be the drain.

  This blocks any uniform flag or fail-fast, because there is no uniform method to attach the
  obligation to. `run_down` and `close_members` have good reasons to differ -- one waits on an
  operation registry, the other releases a whole group -- so the question is whether they are
  renamed, given a common alias, or left alone with the obligation defined per type.

- [ ] **M-T4.9** -- **DECISION TO RAISE: which diagnostic channel carries an obligation report.**
  The crate has two and no stated rule. `ThreadpoolIo::drop` uses `eprintln!`; everything else uses
  `trace_record!`, and [trace.rs](src/trace.rs)'s own module docs open with "Why this is not
  `eprintln!`".

  **That argument does not settle this case**, which is why it is a decision rather than a lookup:
  it is about not perturbing a timing-sensitive race during observation, and an obligation report
  at `Drop` is neither timing-sensitive nor addressed to an investigator. It is addressed to a
  developer who will not have the `trace` feature on -- and the trace compiles to nothing without
  it, so a trace-only report would be invisible to exactly the audience it is for.
- [ ] **M-T4.4** -- **DECISION TO RAISE, reserved by the engineer 2026-09-28 as CRATE-WIDE: a
  fail-fast that forces the caller to have closed, making these types linear rather than affine.**

  **Not to be made piecemeal, and if made, made uniformly.** That is a constraint on the work, not
  a note about it: implementing it for `ThreadpoolWait` alone -- the type the M26.13 measurement
  happens to implicate -- would leave the crate with one linear type and four affine ones, which
  is a worse surface than either choice made consistently. Gated on **M-T4.8**, because there is
  no uniform method to be linear *about* until the synchronous close is uniform.

  **It is not greenfield.** `ThreadpoolIo` already ships a soft version: its `Drop` reports a
  skipped rundown and then continues. A hard fail-fast changes that type's existing behaviour too,
  so the decision is "does the crate become linear", not "do we add something new".

  Still open within it: whether the fail-fast is off by default (assumed), how it is selected --
  environment variable, constructor option, process-wide setter -- and what it does when the
  object is dropped on an already-unwinding path, where a panic aborts.

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