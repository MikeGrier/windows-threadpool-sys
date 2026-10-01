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

- [x] **M-T4.3** -- Report at `Drop` when the caller did not close synchronously. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t43)

- [x] **M-T4.10** -- Extract the re-arm suppression that `ThreadpoolWait` and `ThreadpoolTimer`
  each implemented separately. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m-t410)

- [x] **M-T4.9** -- Decided: a developer-facing report is a trace event, and the crate writes
  nothing to stderr. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m-t49)

- [x] **M-T4.5** -- **`CleanupGroup` already complies; no change needed.** Queued on the strength
  of a grep showing both drain forms at eight sites; reading it, those are the *member* accessors
  (`WaitMember::wait` against `WaitMember::cancel_pending`, and the same pair for work and the two
  timers) -- caller-facing choices, not teardown. The group's own teardown is
  `release_members(false)` in `Drop`, which drains, and `close_members(cancel_pending: bool)` is
  already the explicit early-release method. A member never closes itself either, so nothing in
  that path issues a close behind a disarm.
- [x] **M-T4.7** -- **Answered 2026-09-30, analytically, and the answer is NO.** The item expected a
  20000-run sweep; the question turned out to be decidable from the code. The group dispatches
  member teardown through a vtable whose wait entry includes `TppStopWaitCallbackGeneration`, which
  reaches `NtCancelWaitCompletionPacket` and **threads the caller's cancel-pending argument
  straight through to `RemoveSignaledPacket`**. So a group has exactly the same drain-versus-cancel
  structure as a standalone wait: releasing with FALSE is safe, with TRUE is not. This crate's
  `Drop` already passes FALSE, so a consumer who only drops is safe -- not because the group
  protects them, but because the default was already the safe one. Artifact:
  [which-teardowns-can-still-yank](../windows-ioring-sys/measurements/2026-09-30-which-teardowns-can-still-yank/README.md).

  **The first answer was the opposite and was wrong**, which is recorded in the artifact because
  the failure mode generalises: a reachability walk over direct calls can prove reachability but
  **cannot prove unreachability** where dispatch is indirect, and the group's release dispatches
  through CFG-guarded indirect calls. Reading the vtable reversed the verdict. Any future use of
  that technique must check the closure for indirect calls before relying on a negative.

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

**CLOSED 2026-10-01, by decision rather than by arrival at an answer.** Three hypotheses were
refuted in the end, the third by two flags the capture had been decoding and discarding for days.
What the milestone established is the fault's *shape* -- the port is healthy, the factory is
healthy, and the notification between them is lost -- plus its preconditions, its blast radius, and
what recovers it. What it did not establish is **why**, which is inside the kernel's
queue-to-factory notification and beyond any instrument available here.

The engineer's judgement was that continuing would require fixing the kernel seam, and that the
pattern is now understood well enough to avoid. So the remaining questions -- `M-T5.2`, `M-T5.3`,
`M-T5.4`, `M-T5.10` -- are closed as not-pursued, each with its reason recorded rather than left
looking like an unfinished measurement. **None of them gates the remedy**: the drain is structural
and does not depend on the mechanism, and `M-T6`'s self-heal repairs by a route measured to work
whatever the mechanism turns out to be.

Standing lesson, earned three times: **emit more of what is already in hand before reasoning about
what is not.**

- [x] **M-T5.1** -- **Done 2026-09-30: the work is queued and the pool is idle beside it.** Depth
  **2** on the pool under test, in 15 captures of 15 -- exactly the two victims -- while that pool
  reports 0 workers, `may_create` 1 and `create_in_progress` 0. So the fault is **not** in delivery:
  the packet reaches the port, the factory was entitled to make a thread, and it did not. Also kills
  the benign reading of those counters, which had been consistent with a factory correctly seeing no
  work. Artifact:
  [the-work-is-queued-and-the-pool-is-idle](../windows-ioring-sys/measurements/2026-09-30-the-work-is-queued-and-the-pool-is-idle/README.md).
  The instrument (`trace::completion_port_depths`) has a sabotage-verified positive control, which
  mattered here because a silently broken probe reports "depth 0" -- the finding that would have
  sent the investigation the other way.

- [x] **M-T5.2** -- **CLOSED 2026-10-01: answered as far as measurement reaches.** `M-T5.6`
  established the shape -- the port is healthy, the factory is healthy, and the notification
  between them is lost -- and that is the end of what any instrument available here can see. The
  remaining "why" is inside the kernel's queue-to-factory notification, and the engineer's decision
  was to stop there and address the fault by repair (`M-T6`) rather than by prevention.

  **Closing this does not weaken the fix.** The drain is structural and does not depend on knowing
  the mechanism; self-heal repairs by a route measured to work regardless of it. What is given up
  is the explanation, not the remedy.

  Original text follows; its property 5 is refuted and the refutation is part of the record.

- [x] ~~**M-T5.2 (original)** -- Establish what prompts a factory to create a worker after work is queued.~~
  **Ungated 2026-09-30: `M-T5.1` returned depth 2, so this is now the live question** -- the create
  test would approve, because the port is non-empty, so the stall is not a decision to decline. It
  is the absence of the question.

  **Narrowed 2026-09-30, and property 5 below is REFUTED.** Emitting two flags the capture had
  always decoded and thrown away -- queued-for-deferred-create, and deferred-timer-armed -- shows
  both **0** in 14 stalls of 14. So nothing is pending and nothing is scheduled to ask again. The
  stalled factory reads as *perfectly idle in every field*; the only thing distinguishing it from a
  factory with nothing to do is the two packets on its port. That is a simpler and stronger
  statement than the wedge it replaces: not a creation that got lost, but a prompt that never
  happened. Artifact:
  [nothing-ever-asks-the-factory](../windows-ioring-sys/measurements/2026-09-30-nothing-ever-asks-the-factory/README.md).
  Next measurement is `M-T5.6`.

  Our own measurements already bound the answer: a
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
  4. **A transient empty queue can erase the justification for a create that is already pending.**
     This is the interaction that makes the fault plausible at all, and it is a two-step:
     - A deferred request treats "every `policy_retry_level` is clear" as meaning the work was
       already picked up by some existing worker, and returns **without creating**.
     - The basic test declining **clears every `policy_retry_level`** on its way out.

     Those retry levels are the only record that a thread is still wanted. So if the basic test
     runs while the queue happens to be empty, it wipes the justification a pending deferred
     request was going to rely on, and that request then cancels itself.

     The design reads "queue empty" as "the work was consumed". **Removing a delivered packet makes
     consumed and destroyed indistinguishable** -- the same class of aliasing the platform already
     documents elsewhere on this path, where a cancel-then-reassociate to the same port cannot be
     told from the original association. Our teardown manufactures exactly that ambiguity, a few
     microseconds after the work is queued.

  5. **REFUTED 2026-09-30. `queued_for_deferred_create` was the prime suspect and it reads 0.** The
     hypothesis was that a factory stays flagged as queued for a creation nobody services, wedging
     it permanently. Both that flag and the deferred-timer flag read **0** in 14 stalls of 14, so
     no creation is pending and none is scheduled. Kept here, refuted rather than deleted, because
     it was the third hypothesis this investigation has lost and the pattern is worth preserving:
     each was killed by data that either already existed or cost one record to emit. Property 4's
     two-step remains *unrefuted but unsupported* -- nothing measured bears on it either way, and
     it should not be leaned on.

     **The standing lesson: emit more of what is already in hand before reasoning about what is
     not.** These two flags had been decoded into the capture struct and discarded on every run for
     three days, while the hypothesis they refute was being built.

  Verify this structure against the shipped binary before building on it, rather than carrying it
  forward as an assumption: it was read once, and `M-T5.5` may invalidate it.

- [x] **M-T5.3** -- **NOT PURSUED, by decision 2026-10-01.** The question only mattered for
  locating the mechanism, and the engineer's decision was to stop at the kernel seam and address
  the fault by repair instead (`M-T6`). Anchoring the window more precisely would not change the
  repair, the drain, or anything a consumer does. Recorded rather than deleted because the arms
  were built and verified, so anyone who later wants the answer starts from a known position
  rather than from scratch. Original text follows.

- [x] ~~**M-T5.3 (original)** -- Is the hazard window anchored to the queueing or to the disarm?~~ A run was
  built and started for this and stopped at 45% to free the machine; redo it when a quiet machine
  is available. Two arm families place the packet removal the same distance after `SetEvent` while
  putting the delay on opposite sides of the disarm: `SetEvent -> disarm -> spin N -> close`
  against `SetEvent -> spin N -> disarm -> close`. Coinciding curves say the window is anchored to
  the queueing; a flat second family says it is anchored to the disarm. Both families were verified
  to place the removal at matching times (4-5us, 12us, 32us) before the run started, so the arms
  are ready to rebuild.

- [x] **M-T5.4** -- **CLOSED 2026-10-01: blocked, and no longer needed.** It was queued to settle
  `M-T5.2` by reading the factory's state directly. `M-T5.2` is now closed as far as measurement
  reaches, and the investigation is not continuing past the kernel seam, so the blocker no longer
  gates anything. The firmware finding below stands and is worth keeping -- it is the reason no
  kernel-level answer was available to this investigation at all, and anyone who revisits the
  question will hit the same wall. Original text follows.

- [x] ~~**M-T5.4 (original)** -- local kernel debugging is unavailable on this machine.~~ Reading the factory's own state directly would settle `M-T5.2` outright, and
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

- [x] **M-T5.5** -- **Done 2026-09-30: the update moved the kernel, and nothing else.** ntoskrnl
  went 10.0.26100.9444 -> **10.0.26100.9457**; ntdll is **unchanged** at 10.0.26100.9278, so the
  committed disassembly still describes the shipped binary. Every worker-factory routine the
  analysis rests on is still present in 9457, and the reproducer's rate is unchanged (15 stalls in
  8400 on `hand-spin-3us`, against the 4.15 per thousand measured before the update). Later work is
  measured against 9457.


- [x] **M-T5.6** -- **Done 2026-09-30: arrivals no longer reach the factory.** The posted packet
  lands -- depth 2 -> 4, so the port accepts it -- and **no worker is created**, in 13 captures of
  13. The same poke on a healthy factory in the same starting position takes it from 0 workers to 1
  and consumes the packet, so the stimulus is valid and the null result is a property of the
  stalled process. Artifact:
  [arrivals-no-longer-reach-the-factory](../windows-ioring-sys/measurements/2026-09-30-arrivals-no-longer-reach-the-factory/README.md).

  **This answers `M-T5.2` and dissolves the standing asymmetry.** Port healthy, factory healthy,
  and the notification between them persistently gone -- a packet posted by hand five seconds into
  the stall is ignored exactly as the originals were, so nothing was special about the victims'
  inserts. `NtReleaseWorkerFactoryWorker` recovers the stall every time because it reaches the
  create decision by the other route, which does not depend on the severed link. The fault's shape
  is now: a per-port, persistent loss of the arrival-to-factory notification, caused by removing a
  delivered packet a few microseconds after it was queued, on a port whose factory has no threads
  yet. Why that link breaks is inside the kernel and out of reach here.

- [x] **M-T5.7** -- **Done 2026-09-30.** The retry timeout, infinite-wait goal, start routine and
  parameter, process id, and stack reserve/commit are now emitted alongside the rest. None proved
  decisive this time -- `M-T5.6` answered the question first -- but they are in every future
  capture at the cost of a handful of records, which is the point: the guessing is over.

- [x] **M-T5.8** -- **DECIDED 2026-10-01; superseded by `M-T6`.** The answer is not removal:
  cancellation stays, renamed `try_cancel_pending` to connote the best-effort attempt the platform
  has always actually performed, and backed by a repair that submits a work item to the affected
  pool. Safe method gated on the `self-heal` feature, `unsafe`
  `try_cancel_pending_no_heal_tracking` always present so that disabling the feature breaks call
  sites loudly rather than silently removing a guarantee. Decision:
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md#cancellation-self-heals). Original text follows, and its
  analysis stands -- in particular that removing `cancel_pending` would not have removed the
  hazard, since the close makes the same call.

- [x] ~~**M-T5.8 (original)** -- the removal is in the close, not only in `cancel_pending`.~~

  > **CORRECTED 2026-09-30, same day it was written.** The first version of this item claimed
  > `cancel_pending` is uniquely dangerous and asked whether to remove it. That premise is **false**
  > and this workspace's own committed data said so before the item was written:
  > [cancel-and-gap-are-both-required.csv](../windows-ioring-sys/measurements/2026-09-29-what-the-gap-is-made-of/cancel-and-gap-are-both-required.csv)
  > records `hand-nocancel` -- an arm that **makes no cancel call at all** -- failing 22 times in
  > 20004, against `hand-control`'s 16 with the cancel. Dropping the cancel changes nothing
  > measurable, because `CloseThreadpoolWait` performs the same removal, through the same kernel
  > routine (`IopCancelWaitCompletionPacket`) with the same `RemoveSignaledPacket` flag, whenever it
  > finds a packet still outstanding.
  >
  > **So removing `cancel_pending` would not remove the hazard**, and the four options the item
  > originally offered were all answers to the wrong question.

  What the measurements actually support: the removal happens at whichever call first finds a
  delivered packet. `cancel_pending` does it if called; otherwise the close does it, and **every**
  wait teardown ends in a close. The hazard is the removal landing a few microseconds after the
  packet was queued, on a port whose factory has no threads yet.

  That makes the shipped fix the *only* shape of fix available, rather than one option among
  several: a drain lets the queued callback run, which clears the association, after which the
  close has nothing to take. It does not avoid the dangerous call -- it empties it.

  The decision that remains is narrower and is about surface rather than safety:

  1. `cancel_pending` still exists on waits and wait members, and its honest description is now
     "performs the teardown's removal earlier, removing the chance for the callback to run first."
     That is a much less attractive proposition than its name suggests, and arguably has no
     remaining use case -- but it is not the hazard's cause and removing it buys no safety.
  2. The cost is documented on both methods as of this commit. Prose is not a rung on the detection
     ladder, so if the surface is kept, consider whether anything stronger is wanted.

  Still coupled to **M-T6.8** and **M-T6.7** (raised in `M-T4` as `M-T4.4` and `M-T4.8`, renumbered
  2026-10-01), and still the engineer's call rather than an assistant's.

  The audit's exposure table remains correct as written -- every default path is safe, because
  `Drop` and `stop_and_drain` drain and the group releases with false -- but note *why*: not
  because those paths avoid the removal, but because they leave nothing for it to remove. The audit
  in
  [which-teardowns-can-still-yank](../windows-ioring-sys/measurements/2026-09-30-which-teardowns-can-still-yank/README.md)
  maps every remaining path that can remove a delivered packet, and its table stands.

- [x] **M-T5.9** -- **Done 2026-09-30: severity characterised, and the "end of execution" reading is
  wrong.** The fault did not present at the end of execution -- the trigger finished, and the
  damage blocked work that arrived afterwards. Continued use does **not** hide it: a fresh wait, a
  fresh timer and a completed overlapped read each leave the pool stalled for a full two-second
  window. Only a work submit recovers it, and recovery is complete -- a brand-new wait armed after
  it dispatches in microseconds, in 63 captures across six experiments. Artifact:
  [what-a-process-does-after-the-stall](../windows-ioring-sys/measurements/2026-09-30-what-a-process-does-after-the-stall/README.md).

  **The consequence worth carrying forward is that the fault is camouflaged, not benign.** A
  program that submits work items near its waits sees a latency spike bounded by the interval to
  the next submit -- easy to mistake for scheduler jitter. A program using only waits, timers and
  I/O has no stimulus that will ever recover it, and hangs. This workspace's reproducer is the
  second kind, which is the only reason the fault was ever seen rather than shrugged off.

- [x] **M-T5.10** -- **CLOSED 2026-10-01: unanswerable from here, and accepted.** The engineer's
  judgement was that this is unanswerable without the kernel seam, and the investigation stopped
  there. It is closed as a **decision**, not because an answer arrived: on everything known, a pool
  whose last worker retires becomes vulnerable again, and `pool::prewarm_default_pool` therefore
  narrows a window rather than removing a cause.

  **`M-T6` is what makes that acceptable.** Self-heal bounds the damage without needing to know
  whether the notification loss is permanent or edge-consumed -- it repairs by submitting work,
  which recovers the pool either way. The design deliberately does not assume an answer to this
  question, and the accepted residual is that the self-heal pool could in principle share the
  fault.

  Original text follows, including an instrument that must not be retried as written.

- [x] ~~**M-T5.10 (original)** -- Does the pool break again once it returns to zero workers?~~ The one
  severity question `M-T5.9` could not close. Every capture observes a pool that still holds the
  worker its recovery created, because the idle timeout is 67s and no capture runs that long. If
  the notification is permanently lost rather than edge-consumed, a long-lived process is in a
  permanent stop-start state -- working while a worker happens to be alive, stopping each time the
  pool drains -- rather than having had one bad moment.

  **The obvious experiment hangs and must not be retried as written.** Draining the stalled pool's
  completion port with `GetQueuedCompletionStatus` at a zero timeout, to make a genuine
  empty-to-non-empty transition, does not return: six workers sat fifteen minutes with no progress
  and captured nothing. The instrument was removed rather than kept behind a warning. A plain
  `GetQueuedCompletionStatus` is not a safe way to inspect a thread pool's own port whatever the
  timeout says; `NtQueryIoCompletion` reads the depth without disturbing it, which is what the
  surviving instruments use.

  Viable alternative: recover a stalled process with a work submit, wait past the 67s idle timeout
  for the worker to retire, then arm a fresh wait and time it. Slow -- a handful of captures at a
  minute-plus each -- but it needs no new primitive and answers the question directly.

- [x] **M-T5.11** -- **Done 2026-09-30: a warm pool does not stall. This is a cold-start hazard.**
  Warming the pool first -- one work item, its callback confirmed to have run, so a worker provably
  exists -- gives **0 failures in 24000** against a cold control's **66**. A delay-matched cold arm
  that pays 400us without warming (more than warming's measured 223-304us) still fails at the cold
  rate, so it is the worker and not the elapsed time. Every warm run is individually confirmed, and
  the arm aborts rather than proceed if its warm-up fails. Artifact:
  [a-warm-pool-does-not-stall](../windows-ioring-sys/measurements/2026-09-30-a-warm-pool-does-not-stall/README.md).

  **Exposure is therefore bounded**: near process start, before the pool's first dispatch, and
  after each idle-timeout expiry when the last worker retires (67s for the default pool). A process
  keeping its pool busy is not exposed between those points. This fits the mechanism -- the severed
  notification is the one asking the factory to *create* a worker, and a pool with a thread already
  parked does not need that question asked -- but the fit is corroboration, not proof.

  Does **not** establish that a warm pool is unreachable by any trigger, only by this one at this
  rate over 24000 runs. Changes nothing about the fix: draining remains correct regardless, and
  this bounds when a *non*-draining teardown is dangerous.


## M-T6 -- Cancellation self-heals

Implements [Cancellation repairs the pool it may have wedged](../../DESIGN-NOTES.md#cancellation-self-heals),
decided 2026-10-01. Consumer-facing account in
[README-FEATURE-self-heal.md](README-FEATURE-self-heal.md); how the design was
reached, including the branches abandoned, in
[DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md#how-cancellation-self-heal-was-reached).

Supersedes **M-T5.8**, which asked whether `cancel_pending` should be removed.
The answer is no: it is renamed, made best-effort in name as it always was in
behaviour, and backed by a repair.

- [ ] **M-T6.1** -- **Add the `self-heal` feature, default on, and the pool registry.** A registry
  of the pools this crate is interacting with -- entries created when an object is created against
  a pool and released when the last object on it goes away, so the cost is proportional to use and
  an idle process pays nothing. Each entry holds the last-dispatch stamp, the
  cancellation-owed flag and its stamp, and a pre-created repair work object. **The repair object
  is created at registration, never on the healing path**: creating a work object is measured not
  to release a stall, only submitting one is, so allocation must not happen while a pool is
  wedged.

  **The default pool needs a different retention rule, and this is the decision to take within the
  item.** For a private pool, "release the entry when the last object on it goes away" is right --
  the pool itself is going away too. The default pool is not ours and does not go away: our last
  object dropping says nothing about whether the process is still using it, and a cancellation we
  performed may have left it owing a repair that outlives the object that caused it. Releasing its
  entry on the same rule would drop a pending repair on the floor at exactly the wrong moment.
  Suggested rule, to be confirmed when implementing: the default pool's entry is retained while a
  repair is owed, independent of object count, and the self-heal timer is what releases it once the
  repair is discharged.

- [ ] **M-T6.2** -- **Stamp the last dispatch in every trampoline.** Work, wait, timer and I/O all
  dispatch through a trampoline of this crate's before reaching the caller's closure; each stamps
  its pool's slot before the call. Use the **interrupt-time counter**, not
  `QueryPerformanceCounter`: only ordering against the cancellation is needed, and this path runs
  for every callback, so a memory read is wanted rather than a syscall. One relaxed store.

- [ ] **M-T6.3** -- **Rename to `try_cancel_pending`, and add the ungated `unsafe` sibling.**
  `try_cancel_pending` is gated on `self-heal` and marks its pool as owing a repair;
  `try_cancel_pending_no_heal_tracking` is `unsafe`, always present, and transfers the repair
  obligation to the caller. Same treatment for `WaitMember`, and for `CleanupGroup::close_members`,
  which passes the cancel through to each member. **The contract is identical in both feature
  states** -- best-effort cancellation, the pool may stall briefly -- and only the repair latency
  differs; do not document it as a behavioural difference.

- [ ] **M-T6.4** -- **The self-heal timer.** A periodic timer on a private pool created **lazily on
  the first cancellation**, so a consumer who never cancels never pays for a pool. Each tick, for
  every entry owing a repair: skip when a dispatch has been stamped *after* the cancellation --
  which is direct evidence the pool is live -- and otherwise submit the pre-created repair item.
  The private pool is this crate's own and so is torn down with the drain discipline; a self-heal
  pool that wedged the way it exists to repair would be the worst possible defect.

- [ ] **M-T6.5** -- **Guard it, with the sabotage that matters.** The load-bearing claims are that a
  cancellation arms a repair, that a dispatch after the cancellation suppresses it, and that the
  repair is a submit on a pre-made object rather than a fresh one. Sabotage each: a cancel that
  does not mark, a stamp that never updates, a heal that creates instead of submitting. **A guard
  that passes with the mechanism disabled is worse than none** -- this was already learned once on
  `prewarm`, where a unit test passed the full suite with the function sabotaged because a sibling
  test had warmed the pool. Prefer an integration test where process-wide state would otherwise
  make the assertion vacuous.

- [ ] **M-T6.6** -- **Verify the `self-heal`-off build.** `cargo check --no-default-features` plus
  whatever feature set CI uses, confirming that `try_cancel_pending` is absent, that the `unsafe`
  sibling is present, and that no trampoline stamps. The compile error a consumer gets is the
  designed behaviour, so it is worth asserting the shape of the off build rather than assuming it.

- [ ] **M-T6.7** -- **DECISION TO RAISE: the synchronous close is not uniform, in name or in
  existence.**

  > **Must follow `M-T6.3`**, which is what makes the surface final. That item adds
  > `try_cancel_pending` and `try_cancel_pending_no_heal_tracking` to `ThreadpoolWait`,
  > `WaitMember` and -- through `close_members` -- `CleanupGroup`, so taking this inventory before
  > it would be taking it against a list that is about to change. `M-T6.3` is also the first time
  > this crate has deliberately paired a safe and an `unsafe` form of the same operation, which is
  > a precedent worth weighing here rather than discovering later.

  Raised in `M-T4` while investigating `M-T4.3`, and moved here 2026-10-01 because `M-T6.3` is
  what unblocks it. Four shapes across five types: `stop_and_drain` on `ThreadpoolWait`,
  `ThreadpoolTimer` and `PeriodicTimer`; `run_down` on `ThreadpoolIo`;
  `close_members(cancel_pending: bool)` on `CleanupGroup`; and **nothing named as such on
  `ThreadpoolWork`**, whose `wait()` happens to be the drain.

  **The obligation half is already settled and is not part of this.** `M-T4.3` defined it per
  type and shipped, so what remains here is naming alone. `run_down` and `close_members` have good
  reasons to differ -- one waits on an operation registry, the other releases a whole group -- so
  the question is whether they are renamed, given a common alias, or left as they are.

  Note that the cancel surface and the drain surface are different questions. `M-T6.3` renames the
  former; this asks about the latter.

- [ ] **M-T6.8** -- **DECISION TO RAISE, reserved by the engineer 2026-09-28 as CRATE-WIDE: a
  fail-fast that forces the caller to have closed, making these types linear rather than affine.**

  > **Gated on `M-T6.7`**, because there is no uniform method to be linear *about* until the
  > synchronous close is uniform. Raised in `M-T4` as `M-T4.4` and moved here 2026-10-01 with the
  > item it depends on.

  > **`M-T5.8` set a relevant precedent 2026-10-01**, without settling this. Faced with an
  > operation that could not be made safe, the crate did not reach for linearity -- it paired a
  > safe form with an `unsafe` sibling carrying a statable obligation, and let a feature decide
  > which exists. That is a different answer to "how do we make the caller take responsibility"
  > than a linear type is, and it is now shipping. Weigh it as an alternative here rather than
  > treating linearity as the only way to bind a caller to a protocol.

  **Not to be made piecemeal, and if made, made uniformly.** That is a constraint on the work, not
  a note about it: implementing it for `ThreadpoolWait` alone -- the type the M26.13 measurement
  happens to implicate -- would leave the crate with one linear type and four affine ones, which
  is a worse surface than either choice made consistently.

  **It is not greenfield.** `ThreadpoolIo` already ships a soft version: its `Drop` reports a
  skipped rundown and then continues. A hard fail-fast changes that type's existing behaviour too,
  so the decision is "does the crate become linear", not "do we add something new".

  `M-T4.3` has since given every type a flag recording whether a drain is owed, deliberately not
  behind the `trace` feature, so a fail-fast has the fact it needs without new bookkeeping.

  Still open within it: whether the fail-fast is off by default (assumed), how it is selected --
  environment variable, constructor option, process-wide setter -- and what it does when the
  object is dropped on an already-unwinding path, where a panic aborts.

  **One bound is already fixed and constrains every answer: forward progress is not the
  alternative.** A teardown that cannot drain may abort, or fail fast by some other route, but it
  may not return to its caller having abandoned the callback. Bounding the wait is a question
  about which failure to take, never about whether to continue.
