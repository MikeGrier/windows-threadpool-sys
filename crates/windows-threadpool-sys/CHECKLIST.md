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

- [ ] **M-T5.2** -- **Establish what prompts a factory to create a worker after work is queued.**
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

- [ ] **M-T5.8** -- **DECISION TO RAISE: the removal is in the close, not only in `cancel_pending`.**

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

  Still coupled to **M-T4.4** and **M-T4.8**, and still the engineer's call rather than an
  assistant's.

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

- [ ] **M-T5.10** -- **Does the pool break again once it returns to zero workers?** The one
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

- [ ] **M-T5.11** -- **Is the fault reachable on a pool that already has a worker?** Raised by the
  engineer's question about whether the stuck pool has idle threads. It does not -- 12 stalled
  captures all read total 0, waiting 0, pending 0, against 20 healthy runs reading total 1 -- and
  that is the *only* difference between the two arms at the same instant. See
  [the-blast-radius-is-one-pool](../windows-ioring-sys/measurements/2026-09-30-the-blast-radius-is-one-pool/README.md).

  **Every stall this investigation has ever captured is of a cold pool**, because the reproducer's
  trigger runs at 0.00004s, before the pool has dispatched anything. Whether that is incidental or
  a precondition is untested, and the two answers have very different consequences:

  - **If cold is required**, this is a startup hazard. A process that gets through its first
    dispatch is safe while it stays busy, and the exposure is a narrow window near process start
    plus whatever windows follow each idle-timeout expiry.
  - **If a warm pool is also reachable**, it is a steady-state hazard and the exposure is
    continuous.

  A plausible mechanism favours the first -- the severed notification is the one asking the factory
  to *create* a worker, and a pool with a thread parked for work does not need that question asked.
  **Do not bind to that reading**; it is a mechanism sketch, and three hypotheses of exactly that
  shape have already been refuted in this investigation.

  Measurement: warm the default pool first (submit a work item, wait for the callback, confirm
  `total_worker_count` is 1), *then* run the trigger. A rate indistinguishable from the cold arm
  says warm pools are reachable; a zero over enough runs to be meaningful against the ~4 per
  thousand the `hand-spin-3us` arm gives says cold is required. Cheap -- it reuses the existing
  trigger and capture path, and needs only a warm-up before it.

  Note the coupling to **M-T5.10**: if cold is required, then "does a recovered pool break again
  once its workers retire" has an expected answer rather than being open, because retirement is
  exactly what makes a warm pool cold again. Running this one first may make that one cheaper to
  interpret.
