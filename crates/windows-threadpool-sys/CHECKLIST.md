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
- [x] **M-T4.6** -- **Done 2026-09-29: the fix holds on the real path.** The 20000-run arms were a
  hand-rolled model of the teardown, not this crate's code, so the committed change had to be
  measured against `EventDelivery` itself. Two builds differing only in `ThreadpoolWait`'s teardown,
  same reproducer, same session: the reverted (cancel) build reproduces, the committed (drain) build
  does not over seven times the runs. Counts and provenance in
  [measurements/2026-09-29-the-fix-on-the-real-path/](../windows-ioring-sys/measurements/2026-09-29-the-fix-on-the-real-path/README.md).
  Note what it does *not* establish: it excludes "the rate is unchanged", not "the rate is zero",
  and it is not a root cause. `M26.9` may be called closed on this; the open question is why the
  close-behind-disarm stalls the pool at all.
## M-T5 -- Why the pool stops making workers

Opened 2026-09-30. `M-T4` shipped a fix whose correctness is structural rather than statistical --
the drain cannot reach the primitive that does the damage, so it holds however the timing falls --
but the *cause* is still open. What is established is in
[what-the-disassembly-says](../windows-ioring-sys/measurements/2026-09-30-what-the-disassembly-says/README.md)
and [STALL-TIMELINE.md](../windows-ioring-sys/STALL-TIMELINE.md): all four teardown paths converge
on `NtCancelWaitCompletionPacket`, differing only in whether they ask it to remove an
already-delivered packet, and the drain never calls it at all.

Two hypotheses have already been killed by evidence that existed before they were proposed -- "the
queued callback must have run" (refuted by a graded gap sweep) and "a creation-in-progress gate is
stuck" (refuted by a 2026-09-27 capture recording that counter as 0). Treat any third with the
same suspicion, and look for a disconfirming measurement before building on it.

- [ ] **M-T5.1** -- **Measure the completion port's queue depth during a stall.** The decisive next
  measurement, and it needs no elevation, no reboot and no kernel debugger. A worker factory makes
  a thread when its completion port has work outstanding, so the port's queue depth is the quantity
  that decides whether the stalled pool *should* have created one. Read it from user mode with
  `NtQueryIoCompletion(handle, IoCompletionBasicInformation, ...)`, which reports the depth without
  dequeuing, so the observation does not disturb what it measures. Getting the handle needs no new
  discovery work: the completion port is an argument to `NtCreateWorkerFactory`, and this crate
  already hooks that surface. Run it on the `hand-spin-3us` arm, whose rate is about 4.15 per
  thousand against a baseline of 0.87, so a few thousand runs suffice rather than tens of
  thousands.

  **The two outcomes send the investigation in opposite directions**, which is what makes this
  worth doing first. A depth above zero means the victims' work is queued and undispatched while
  the factory reports itself willing and idle -- so the factory was never asked, and the question
  becomes what should have asked it. A depth of zero means the work is not there at all, the
  factory is behaving correctly on the information it has, and the fault is upstream in delivery.

- [ ] **M-T5.2** -- **Establish what prompts a factory to create a worker after work is queued.**
  Gated on `M-T5.1` returning a non-zero depth. Our own measurements already bound the answer: a
  healthy run creates a worker 0.24-0.31ms after the delivery is armed, so something on the
  queueing path does prompt it; a stalled run never does, and the only call ever observed to
  release the stall is `NtReleaseWorkerFactoryWorker` from the work-submit path, which reaches the
  factory by a different route than queued work does. That asymmetry is the thing to explain.

  **What the create decision looks like, and why it narrows the search.** The relevant routines are
  named in the public symbols -- `ExpWorkerFactoryCheckCreate`, `ExpWorkerFactoryWantsToCreate`,
  `ExpWorkerFactoryCreateThread`, `ExpSetWorkerFactoryDeferredCreateTimer`,
  `ExpWorkerFactoryManagerThread` -- alongside globals for a creation state, a deferred-creation
  list, and short, medium and long deferral timeouts. Their structure is readable by disassembly
  (the public PDB carries these names but no struct layouts, so field *names* are not available and
  nothing below depends on one).

  **Nomenclature.** The factory's per-instance fields are reachable only as offsets, so the names
  below are **ours, assigned for this investigation**, not the platform's. They are written in
  `snake_case` to keep that visible. The sole exception is `create_in_progress`, which is a real
  field of the public `WORKER_FACTORY_BASIC_INFORMATION` and is already what
  [hook.rs](src/trace/hook.rs) records.

  Four properties matter for this investigation:

  1. **The create test has two independent triggers.** One is work outstanding on the completion
     port; the other is a count of user-mode release requests. `NtReleaseWorkerFactoryWorker`
     arrives on the second. Removing a delivered packet zeroes the first and leaves the second
     untouched -- which is precisely the asymmetry measured between queued work (never recovers)
     and the submit (always recovers).
  2. **A one-at-a-time gate is tested before either trigger**, so a creation believed to be in
     flight suppresses all others. That was the obvious wedge and it is **already refuted**: the
     2026-09-27 captures record `create_in_progress` as 0 in stalled processes.
  3. **The deferral path is built to self-heal.** Three policies can decline a create; each keeps
     its own `policy_retry_level`, which escalates across two deferrals and then causes that policy
     to be skipped outright, forcing the create. So a factory cannot be wedged by a policy that
     keeps saying no -- which is what makes "the factory is never asked again" the remaining shape,
     and why `M-T5.1`'s queue depth is the measurement that matters.
  4. **`queued_for_deferred_create` is the current prime suspect.** A factory is pushed onto the
     global deferred-creation list only when this per-factory flag is clear, and setting it is what
     makes the push happen. Both of the early exits from the create routine -- the one taken when a
     deferred request finds its retry levels already cleared, and the one taken when the basic test
     declines -- return without clearing it. If a stall leaves it set while the factory is on no
     list, nothing would ever queue it again, and with no workers alive nothing else would ask.
     That is consistent with every counter we captured, including `create_in_progress` being 0.
     **Do not treat this as established**: it is the third hypothesis in this investigation, and
     the first two were killed by data that already existed. Look for the disconfirming measurement
     first.

  Verify this structure against the shipped binary before building on it, rather than carrying it
  forward as an assumption: it was read once, and `M-T5.5` may invalidate it.

- [ ] **M-T5.3** -- **Is the hazard window anchored to the queueing or to the disarm?** A run was
  built and started for this and stopped at 45% to free the machine; redo it when a quiet machine
  is available. Two arm families place the packet removal the same distance after `SetEvent` while
  putting the delay on opposite sides of the disarm: `SetEvent -> disarm -> spin N -> close`
  against `SetEvent -> spin N -> disarm -> close`. Coinciding curves say the window is anchored to
  the queueing; a flat second family says it is anchored to the disarm. Both families were verified
  to place the removal at matching times (4-5us, 12us, 32us) before the run started, so the arms
  are ready to rebuild.

- [ ] **M-T5.4** -- **RECORDED AS BLOCKED, NOT DEFERRED: local kernel debugging is unavailable on
  this machine.** Reading the factory's own state directly would settle `M-T5.2` outright, and
  `kd -kl` is the tool for it. It is blocked by a **firmware** condition rather than a missing
  step: `bcdedit -debug on` fails with "The value is protected by Secure Boot policy", and the
  machine reports Secure Boot enabled with VBS running and Credential Guard active. Enabling it
  needs Secure Boot disabled in UEFI, which is a real security downgrade and may be policy
  forbidden. Two further cautions if it is ever revisited. Local kernel debugging is **read-only**,
  so it cannot set the kernel's thread-pool debug-print mask (the symbol exists, and the component
  id is 84) -- capturing that narration would additionally need a registry filter and a
  kernel-print capture. And **boot-debug mode perturbs what is being measured**: the signal is a
  2-6us race at about one run in a thousand, so a configuration change that alters kernel timing
  could mask it while appearing to test it. A clean result under debug boot is not evidence.

- [ ] **M-T5.5** -- **Re-establish the baseline after the pending Windows update.** The analysis so
  far is of specific builds -- ntdll 10.0.26100.9278 and ntoskrnl 10.0.26100.9444 -- and an update
  was being installed when this milestone was opened. Internal layouts are unstable across builds
  and the thread-pool code may differ. Before comparing any earlier rate against a new one,
  re-measure the control arm and record the new build numbers. The committed disassembly listings
  name the build they came from precisely so that this comparison is possible.
