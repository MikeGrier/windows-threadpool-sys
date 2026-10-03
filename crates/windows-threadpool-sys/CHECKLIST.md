# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

M-T6 -- cancellation self-heals -- completed on 2026-10-01 and is archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M-T7 -- The pool stall: why a worker factory loses its worker

**Transferred from `windows-ioring-sys` on 2026-10-01, because the fault was never that crate's.**
`M26.13.4` established it by measurement: during the stall the process thread pool dispatches **no
callback of any kind** -- a wait, a timer and an I/O object each created and armed *during* the
stall, with no connection to any ring, all sit undispatched until a work item is submitted. The ring
is still needed to *reach* the state, which is why the reproducer stays in that crate, but nothing
about the ring is what is stuck.

What moved with it: [STALL-TIMELINE.md](STALL-TIMELINE.md), which walks one captured occurrence
record by record, and the 33 measurement captures under [measurements/](measurements) taken during
the investigation. What stayed: the reproducer itself, which is `windows-ioring-sys`' own test
binary, and that crate's resolved-failure entry, because the tests that failed are its.

**The remedy already shipped and is measured.** `M-T4` made teardown drain rather than cancel, which
is what closes the failure; `M26.15` re-measured it against the current build at 0 in 12000 with a
control reproducing at 12 in 4000. So everything below is **diagnostic, not remedial** -- it buys an
explanation, not working software.

**The hazard is still reachable, which is the argument for keeping any of it.** `try_cancel_pending`
remains public: the drain changed the default, not the available operations. The self-heal is the
mitigation for a caller who cancels, and it works by submitting a work item to a pool owing a
repair -- which `M26.13.3` found is exactly what ends the stall. That makes the self-heal a rescue
for this specific kernel behaviour, built out of this investigation, and never measured against it.

**Item IDs keep their original `M26.*` numbers in parentheses**, because roughly thirty measurement
READMEs cite them and renumbering would orphan every citation.

- [ ] **M-T7.1** (was `M26.13`) -- **Find why the delivery stall's dispatch is delayed.**
  Re-opened and substantially narrowed on 2026-09-26; the measurements are in
  [UNRESOLVED-TEST-FAILURES.md](../windows-ioring-sys/RESOLVED-TEST-FAILURES.md) and are not repeated here, and one
  captured occurrence is walked through record by record in
  [STALL-TIMELINE.md](../windows-threadpool-sys/STALL-TIMELINE.md).

  **What changed.** The stall was believed fixed by [D-68](../windows-ioring-sys/DESIGN-NOTES.md#d-68). It is not: it
  still reproduces against the current build, with D-68's arm-before-signal ordering visible in the
  trace of every capture. It is a **permanent hang** -- `M26.13.9` removed the post-mortem probe and
  waited sixty seconds, and the delivery never arrived at all. An earlier revision of this item said
  D-68 had converted the permanent loss into "a delayed dispatch that eventually delivers
  everything"; that was an artifact of the probe, which releases the pool before the measurement is
  taken.

  **Re-planned 2026-09-26 after `M26.13.3`.** The question this item was written around --
  what releases the stall -- is answered, and the answer moves the search. The stall ends when a
  work item is **queued** to the pool and at no other time: it does not end on a timer, does not end
  on its own through a two-second quiet period, and does not end when the work object is merely
  created. Nine captures in three configurations, in
  [measurements/2026-09-26-what-releases-the-stall/](../windows-threadpool-sys/measurements/2026-09-26-what-releases-the-stall/README.md).
  So the remaining question is a **pool** question -- why a queued wait callback waits for an
  unrelated `SubmitThreadpoolWork` -- where this item had been framed as a **ring** question.

  **Re-planned again 2026-09-27 after `M26.13.4`, and the item stopped being about the ring at all --
  which is what eventually moved it to this crate.**
  During the stall the pool dispatches **no callback of any kind**: a wait, a timer, and an I/O
  object each created and armed *during* the stall, with no connection to any ring, all sit
  undispatched for two seconds and then run only once a work item is submitted. Fifteen captures in
  [measurements/2026-09-27-which-poke-releases-the-stall/](../windows-threadpool-sys/measurements/2026-09-27-which-poke-releases-the-stall/README.md).
  So the earlier framing -- "a queued *wait* callback waits for an unrelated `SubmitThreadpoolWork`"
  -- was still too narrow. Nothing dispatches, and a work submit restarts everything.

  **The four ring-ingredient experiments are withdrawn as the next step**, not because they are
  answered but because they were aimed at the wrong half: they were designed to reproduce the
  *entry* into the stall in isolation, and the isolation already fails to reproduce it across six
  thousand trials. They stay available if the experiments below dead-end. For the record, they were:
  an event the kernel also signals via `SetIoRingCompletionEvent`; a callback that re-arms itself
  from the pool thread; a callback that drains under a mutex the test thread also takes; and
  `CloseIoRing` releasing the kernel's reference to a still-armed event.

  **Re-planned 2026-10-01: a remedy shipped, so this item is no longer the thing standing between
  the suite and a clean run.** `M26.14.1` and `M26.14.2` found the entry -- the trigger's teardown,
  closing a wait too soon after disarming it with a *cancel* -- and `M26.14.3`'s decision was taken:
  `windows-threadpool-sys` now drains instead, which `M26.14.2` measured at 0 in 20000 against a
  control's 10. So the experiments below are now **diagnostic rather than remedial**. They would say
  *why* the kernel loses the worker; they are no longer the route to making the tests pass, and
  `M26.15` is what establishes whether they still describe a live failure at all.

  **Experiment 3 is withdrawn as a remedy.** "Adopt a private pool, or keep looking?" was explicitly
  a workaround ahead of a diagnosis; the diagnosis arrived and produced a fix that costs no design
  change, so the workaround's questions -- who owns the pool, one per delivery or shared, what it
  means for a caller's own environment -- no longer need answering to get past this. It is kept
  below only as the record of what was considered.

  **DECISION TO RAISE, not to take: park experiments 0, 1 and 2, or keep them live?** They ask a
  real question nobody has answered -- what leaves the worker factory with no worker -- and this
  item has already established that ETW cannot separate the two candidate explanations, that the
  `DISPATCHER` flag needs elevation, and that no public provider describes a completion packet
  reaching a port. Against that: the fix has shipped, so the work buys understanding rather than
  working software. Parking them in `M-inf` beside `M26.14.4` would be consistent with how that
  item was treated; keeping them here says the diagnosis is still owed. Not taken here, because
  "stop investigating a kernel behaviour we cannot explain" is the engineer's call, not this
  item's.

  **The remaining experiments**, in order:
  0. **Arm a wait on an already-signalled event at process start, and see whether it is ever
     delivered.** The question is unchanged -- separate `the pool would never have dispatched in
     this process` from `it was put into this state during setup` -- but `M26.13.14` ruled out the
     method this item previously proposed. A heartbeat timer cannot get below **15.625 ms**, the
     default Windows tick, without `timeBeginPeriod` changing the machine's timer behaviour under
     the measurement, and 15.7 ms is still after the deliveries are armed at about 2.5 ms. An
     already-signalled wait is due **immediately**, needs no timer resolution at all, and is in the
     kernel-delivered class that fails. Firing in the first fraction of a millisecond means the pool
     dispatched before the trigger ran and the fault was induced afterwards; not firing means the
     pool never dispatched in this process at all. Guard it with the healthy case: in a passing run
     it must fire, or the probe proves nothing.
  1. **Add `DISPATCHER` to the kernel trace and name who readies the worker.** `M26.13.19` ran the
     `PROC_THREAD` half of this and it landed: across 900 traced runs exactly one process has a gap
     over a second, the failing one, whose first pool worker arrives 5008.7 ms after the last test
     thread against a healthy 0.232 to 17.928 ms. That confirms the missing worker from an
     instrument the `ntdll` hooks do not touch, which was the point of running it.

     What is left of this item is the second flag. `DISPATCHER` emits `ReadyThread`, naming which
     thread readied which, so a healthy run would say what *causes* the worker to appear 0.376 ms
     after the last test thread -- and a failing run would say whether anything in the process is
     readied at all during the five seconds. Run it the same way, and pair every capture with a
     healthy control from the same session.

     **It needs elevation, and that is the only thing stopping it.** The NT Kernel Logger refuses a
     non-elevated session (`xperf -on ...` answers `Access is denied. (0x5)`); `sudo` in Inline mode
     works and was used for `M26.13.19`. `DISPATCHER` is far higher volume than `PROC_THREAD`, so
     size the run deliberately: 900 runs took 190 s and produced a 31 MB trace with `PROC_THREAD`
     alone, and one failure. Consider a ring buffer flushed on failure rather than a continuous
     file.

     **What no amount of ETW will answer**, checked rather than assumed on 2026-09-27: there is no
     public event for a wait-completion packet reaching an I/O completion port, and none for
     worker-factory activation. All 1198 registered providers carry no thread-pool provider by name;
     `xperf -providers KF` has no thread-pool kernel flag; and a census of all 40
     `Microsoft-Windows-Kernel-*` manifests (`wevtutil gp /ge /gm`) finds no event declaring a
     worker factory or a completion packet -- the only `Worker` hits are the cache, power and
     prefetch providers' own unrelated workers. `ntdll`'s `TppETW*` routines emit through
     `NtTraceEvent` directly and carry the same facts the hooks already record. So "the factory was
     given the packet and did not act" and "the packet never arrived" cannot be separated this way,
     and this item must not be written up as though it settles them.
  2. **What about the trigger leaves the pool with no worker?** `M26.9` narrowed entry to a
     co-running test that creates an `EventDelivery` over a ring with nothing outstanding and drops
     it promptly. Re-ask it as a thread-supply question rather than a ring question: does that
     teardown retire the pool's last worker, and is the ring incidental to it?

  3. **DECISION TO RAISE, not to take: adopt a private pool, or keep looking?** `M26.13.5` measured
     a clean 12000 runs across three private-pool arms against 13 failures in 4000 on the default
     pool. `EventDelivery::new` already takes an environment, so this is reachable today -- but it
     is a **workaround ahead of a diagnosis**, and it carries design questions this item must not
     answer alone: who owns the pool, whether there is one per delivery or one shared, what it means
     for a caller who passes an environment of their own, and whether a crate should quietly move a
     caller's callbacks off the pool they expected. Raise it before building it.

     > **Handoff discharged 2026-10-01.** This callout, written from `windows-ioring-sys`, said a
     > remedy would most likely land in this crate rather than in the ring. It did: `M-T4`'s
     > draining teardown. The item itself has since followed it here.

  **Ruled out, so they are not retried:** that the stall is a timer, that the test thread waking
  ends it, that creating a work object rather than queuing one ends it, that any non-work poke ends
  it, that it is specific to waits or to the ring's wait, that a thread minimum prevents it, that
  this workspace's own callbacks occupy the pool's threads, and that announcing those callbacks as
  long-running prevents it.

  **Done, and archived rather than repeated here:**
  - `M26.13.3` -- the stall ends on a work submit and at no other time.
  - `M26.13.4` -- and nothing else dispatches either, so the fault is pool-wide.
  - `M26.13.5` -- it has only ever been seen on the default pool, and the thread minimum is not why.
  - `M26.13.6` -- the pool has no worker while stalled and makes two or three when work is
    submitted; runs-long does not change that, and none of our callbacks is holding a thread.

  **Do not treat the report's "it arrived, N past the bound" as delivery latency.** That interval is
  measured from the start of the post-mortem, which is after the probe has run -- and the probe is
  now known to be what ends the stall, so the figure describes the probe, not the delivery. Fixing
  that wording is part of this item.

- [x] **M26.13.1** -- Paired entry/exit records added to all five pool trampolines, the re-arm, and the test's post-mortem path; a sabotage sweep confirms each is load-bearing. -> [completed 2026-09-26](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26131)

- [x] **M26.13.2** -- The buffer is per-process, so the population was never "the full suite"; the capturing binary emits 89 records, the threadpool crate's own 14061, and an overflow now announces itself. -> [completed 2026-09-26](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26132)

- [x] **M26.13.3** -- The stall ends when a work item is queued to the pool, at no other time, and the five-second coincidence is the probe's timing rather than a timer. -> [completed 2026-09-26](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26133)

- [x] **M26.13.4** -- Experiment 3: nothing else releases it, and nothing else dispatches either -- a wait, a timer and an I/O object armed during the stall all sit undispatched, so the fault is pool-wide and not this crate's. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26134)

- [x] **M26.13.5** -- Experiment 2: a private pool does not stall in 12000 runs, but the thread minimum is not why -- the no-minimum arm is already clean, so the supply reading it was written to test is unsupported. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26135)

- [x] **M26.13.6** -- The pool has 6 threads while stalled and 8 or 9 right after the work submit, so it has no worker and makes one; runs-long does not change that, and no callback of ours is holding a thread. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26136)

- [x] **M26.13.7** -- With call-boundary and exception tracing on, no Win32 call blocks during the stall: 707 bracketed calls across 24 captures all returned, slowest 220us. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26137)

- [x] **M26.13.8** -- The reproducer's own 5000ms submit timeout is not the five seconds: changed to 4000ms, dispatch still resumed at five in 14 of 14. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26138)

- [x] **M26.13.9** -- It never self-releases. With the probe removed the delivery never arrives in 65s, so this is a permanent hang and the `delayed dispatch` claim was an artifact of the instrument. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26139)

- [x] **M26.13.10** -- Not a missed wake: re-signalling the event the wait is armed on releases nothing in 5 of 5, with every SetEvent's success recorded. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261310)

- [x] **M26.13.11** -- A dump taken while stalled shows three pool workers parked idle in `ZwWaitForWorkViaWorkerFactory`, so the pool is not starved of threads and `M26.13.6`'s reading was wrong. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261311)

- [x] **M26.13.12** -- A self-rearming timer armed while the pool was healthy, due inside the stall window, fires only at the release in 10 of 10 -- so the fault is dispatch, not registration. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261312)

- [x] **M26.13.13** -- At a 100ms period the heartbeat becomes a clock: its first expiry is already missed and no pool callback of any kind is dispatched before the release, in 18 of 18. The pool never starts.  -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261313)

- [x] **M26.13.14** -- At the 15.625ms system tick the heartbeat misses 320 consecutive expiries and the trace window is empty end to end, in 13 of 13; the onset is bracketed to the first 15.7ms, and the sub-millisecond period item 0 asked for is not reachable this way. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261314)

- [x] **M26.13.15** -- Delaying only the `SubmitThreadpoolWork` call by up to 2000ms moves the delivery with it in 99 of 99, the released worker serves the queued wait ahead of the work that woke it, and an `ntdll` census corrects "user-mode queue push" to "the only path that calls `NtReleaseWorkerFactoryWorker`". -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261315)

- [x] **M26.13.16** -- No thread alive at the stall ever runs a callback, in 12 of 12, while a passing run serves the delivery on a pre-existing thread in 30 of 30: the parked workers are present and unused, so `M26.13.11`'s "not starved" survives but the inference drawn from it does not. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261316)

- [x] **M26.13.17** -- Inline ntdll hooks and a worker-factory scan: the process holds two factories, and the default pool reports zero workers while stalled against one while healthy, so `M26.13.11`'s three parked workers were the *other* factory's all along. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261317)

- [x] **M26.13.18** -- Hooks installed before `main` show the pool's first worker is never created: a healthy run makes one 0.24-0.31ms after the delivery is armed, a stalled run makes none before the release in 8 of 8, and the `AlreadySignaled` race is refuted. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261318)

- [x] **M26.13.19** -- An ETW kernel trace confirms the missing worker independently of the ntdll hooks: across 900 traced runs exactly one process has a gap over a second, the failing one, at 5008.7ms against a healthy 0.232-17.928ms. -> [completed 2026-09-27](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261319)

- [x] **M26.13.20** -- Re-verified the reproducer's premises from a clean build: the 3-test reduction matches the full binary (16 vs 18 in 4000), the trigger is necessary (0 in 4000 without it), one victim suffices at about half the rate, and "fail together, never singly" was wrong. -> [completed 2026-09-28](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m261320)

- [x] **M26.14.1** -- The trigger's *teardown* is what poisons: built and never dropped it gives 0 in 4000 against a live control's 11, and a ring dropped without a delivery also gives 0. -> [completed 2026-09-28](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26141)

- [x] **M26.14.2** -- Closing the wait too soon after disarming it is the poison: a 1ms gap between the disarm and the close, or a true drain in place of a cancel, each give 0 in 20000 against a control's 10. -> [completed 2026-09-28](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26142)

- [x] **M26.14.3** -- Taken, and taken the way this item proposed: `windows-threadpool-sys` adopted
  the draining teardown. -> [completed 2026-10-01](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m26143)

- [x] **M26.15** -- Re-measured: control 12 in 4000, current 0 in 12000, and the stall's entry moved
  to [RESOLVED-TEST-FAILURES.md](../windows-ioring-sys/RESOLVED-TEST-FAILURES.md). The item's own premise was wrong and is
  corrected in the archive. -> [completed 2026-10-01](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m2615)

## M-T8 -- Pool placement

Opened 2026-10-01. It was found by a census in `windows-ioring-sys` that has since been retired
along with the question it served -- see
[that crate's archive](../windows-ioring-sys/COMPLETED-CHECKLIST.md#moved-2026-10-01-m27-retired).

**The finding outlived its origin, and this item does not depend on it.** The gap is stated below
from the measured surfaces rather than by citing the retired document, so nothing here rests on
work that was withdrawn. A pool whose threads can be placed is useful to anyone; that was true
before a realizer was imagined and remains true now that the realizer is understood to be a
component nobody has written.

- [ ] **M-T8.1** -- **Decide whether this crate expresses thread placement, and if so where.**

  **The gap, stated as measured.** An `IoRing` has no thread of its own; completion delivery runs on
  a pool reached through `CallbackEnviron`. A caller can already give a domain its own pool with a
  bounded thread count -- `ThreadpoolPool` offers `new`, `set_min_threads`, `set_max_threads` -- and
  `CallbackEnviron` offers `set_pool`, `clear_pool`, `set_priority`, `set_runs_long`. None of those
  says which processor a pool's threads run on, so "pin this domain to processor P" has no
  expression anywhere in this workspace.

  **Why this is a decision and not a fix.** Win32 offers no pool-affinity call. The reachable
  alternatives are a per-callback `SetThreadAffinityMask`, which places the *callback* rather than
  the *pool* and must be undone before the thread returns to the pool; or a dedicated thread outside
  the pool, which is a different execution model and not this crate's current one. Choosing between
  them decides what this crate is, so it is raised rather than taken.

  **Do not answer it by growing a thread.** `windows-ioring-sys` deliberately has none, and the
  census records that the gap should close here rather than be worked around by that crate acquiring
  one.

  **`M-T7.1` is unrelated and does not gate this.** That item is the pool *stall*; this is pool
  *placement*. They share a crate and nothing else.

## M-T9 -- Make the repair mark lossless, and guard the teardown that frees it

Opened 2026-10-01 by a code review of this branch, which found two defects in `heal.rs` that the
whole test suite and a full sabotage sweep had both missed. The first is fixed; `M-T9.2` is the
second, and `M-T9.1` is the guard both of them want.

- [x] **M-T9.2** -- **Replace the repair mark with three derived timestamps, and point the
      trampoline at the entry.** Implemented 2026-10-02 to the shape recorded below.

  **What landed.** `last_cancelled`, `last_started` and `last_submitted`, each written by an
  unconditional store. `unhealed()` is `last_cancelled != 0 && last_cancelled >= last_started`;
  `repair_in_flight()` is `last_submitted > last_started`; `tick` submits when unhealed and nothing
  is in flight, and **clears nothing**. The trampoline takes a `*const PoolEntry`, so all four
  values live on the entry, and `repair` is the entry's first field.

  **Correction, and then a fix, both from `M-T9.1`:** this entry first said that placement made the
  drain run "before anything it writes to is freed", and that is false. Every other field owns no
  heap, so dropping one frees nothing; the single allocation belongs to the `Arc` and is released
  only after all drop glue. The ordering was load-bearing for the `Box<AtomicU64>` design it
  replaced, and the justification was restated for a design where it no longer applied. The drain
  has since moved onto `PoolEntry::drop`, so declaration order carries nothing and the question
  cannot be got wrong a third time.

  **Verified by sabotage, not by reading.** Restoring the keep-earliest `compare_exchange` fails
  `a_cancellation_during_a_tick_is_not_lost` on the assertion that a later cancellation moves the
  stamp forward. Both `heal.rs` entries in `sabotage.json` were re-anchored and re-run through the
  harness; both are caught.

  **Equality reads as unhealed**, per the first detail the item asked to decide deliberately. A
  cancellation and a dispatch inside one `QueryInterruptTime` tick are indistinguishable, so this
  errs toward a redundant repair; `a_dispatch_in_the_same_tick_as_the_cancellation_does_not_heal_it`
  pins it.

  **The submit guard is `!repair_in_flight()`, not `last_cancelled > last_submitted`.** The item
  proposed the latter. The former is the same rule stated against the pair that already answers
  "has the pool given it back", so there is one comparison rather than two that must agree -- and it
  behaves better in the case the item did not reach: a cancellation arriving while a repair is in
  flight is answered by that repair if it dispatches afterwards, and by the next tick if it does
  not.

  **The user-dispatch signal is gone, with the tests that proved its wiring.** `stamp_dispatch`,
  `last_dispatch` and `dispatched_since` are removed, and with them the five per-kind tests that
  asserted each trampoline stamped its entry. They tested a mechanism this item replaces: health is
  now evidenced by *our own* repair dispatching, which a quiet pool cannot fake. Removing it also
  takes one atomic store off every callback the crate delivers. `Registration` is still held by the
  work, timer, periodic-timer and I/O contexts, now purely for its `Drop`, and each field says so.

  > **DECISION TO RAISE (the item's second deferred detail): a wedged pool still never retries.**
  > One repair is submitted per cancellation; if the pool never dispatches it, `repair_in_flight`
  > stays true and nothing tries again. That is unchanged from the previous scheme, so it is not a
  > regression -- but whether a repair unstarted after N ticks should be re-submitted is open, and
  > the stamps now make it cheap to answer. Not decided here.

- [x] **M-T9.1** -- **Write the test that would have caught the repair-object use-after-free.**
      Done 2026-10-02 as `retiring_an_entry_waits_for_a_repair_callback_already_running`.

  **The window, now that `M-T9.2` has landed.** `repair_in_flight` keeps an entry alive while a
  repair is queued, so the original shape -- submitted, not yet dispatched, entry freed -- is no
  longer reachable. What remains is narrower, and is what the drain actually covers: the callback
  stamps `last_started` on entry, which is precisely what makes the entry retirable, and then writes
  to the entry again. A retirement landing between those two writes drops the last `Arc`, and
  `CloseThreadpoolWork` does not wait.

  **Asserted on the drop blocking, not on a fault** -- a deliberate departure from this item's
  suggestion of `windows-guard-alloc`, for the reason the item itself gives. A crash-caught result
  is treated as uncovered here, and a guard page reports a use-after-free by crashing. Measuring
  that the retirement waited states the property directly, needs neither a second process nor a
  global allocator, and fails by name. Under sabotage: **3.7 us against a 400 ms hold**, scored by
  the harness as `exit 101` -- an ordinary test failure rather than a crash code.

  **The window is reached by holding the callback inside it**, under a `#[cfg(test)]` hook, because
  it is a few instructions wide and nothing lands there by timing. Added to `sabotage.json`, so the
  guard is re-checked rather than verified once and discarded.

  **DECIDED 2026-10-02 and done: the invariant is unrepresentable rather than guarded.** `PoolEntry`
  now drains in its own `Drop`, which runs to completion before any field is dropped, so the
  teardown no longer depends on declaration order at all. Checked by moving `repair` to the **last**
  field and re-running the guard: it still passes, where the arrangement it replaced would have had
  exactly the hazard its own comment warned about. The drain stays guarded by the test above, which
  still fails under sabotage at `exit 101`.

Opened 2026-10-01 by a code review of this branch, which found a use-after-free in `RepairWork::drop`
that the whole test suite and a full sabotage sweep had both missed.

## M-T10 -- The second review round: ordering, serialization, and a test that never ran

Opened 2026-10-02 by a review of everything on this branch. Four findings, all in
`windows-threadpool-sys`. `M-T10.1` is fixed; the rest are open. The round's lesson is recorded
with `M-T10.1`: the reviewer named two sites for a rule that lived at eight, and the sweep the
house rules require is what found the third.

- [x] **M-T10.1** -- **Record the drain obligation before the arming is published, at one site
      rather than eight.** The re-arm paths in `wait` and `timer`, and `work`'s submit, set the
      obligation flag *after* the native call that publishes the arming. A dispatch entering in
      that window settles the obligation and is then overwritten, so a correctly-drained object
      claims it owes a drain -- under `fail-fast` a panic, and an abort if `Drop` was already
      unwinding. Fixed by moving the pair into `CloseObligation::record_live_before` and making
      the bare store private, so the inverted order is a compile error. Recorded as
      [The drain obligation is recorded before the arming is
      published](../../DESIGN-NOTES.md#obligation-recorded-before-arming).

- [x] **M-T10.2** -- **Make `tests/obligation_report.rs` run in CI, then make it pass there.**

  **The defect, and which half matters.** The test deliberately drops objects that owe a drain, so
  under `--all-features` -- which arms `fail-fast` -- it exits 101. That is the reported symptom.
  The *defect* is why nobody noticed: the test early-returns when no trace filter is set, so on
  every CI run it does nothing and reports success. It is inert, and an inert test that reads as
  green is worse than an absent one.

  **Target.** Fix the inertness first and confirm the test then fails, which is what shows the
  early return was hiding it. Then make it pass under `fail-fast` -- either by catching the
  expected panic, or by gating the owing cases off the feature -- without reintroducing a path
  where the whole body is skipped silently.

  **Sabotage it**: break the behaviour the test asserts and confirm it goes red *in the
  configuration CI actually runs*. A guard that only fires under a locally-set environment
  variable has not been shown to guard anything.

- [x] **M-T10.3** -- **Serialize every entry point that suspends other threads.** Done by a
      `Mutex` inside `with_others_suspended` itself rather than a guard at each caller, so the
      rule holds for every path into it including ones not yet written. `install_requested`'s
      `AtomicBool` stops it running twice, which is idempotence and not mutual exclusion;
      `install_by_label` had nothing.

  **The reported hang did not reproduce, and the report should not be relied on.** The review
  said two named tests hang at `--test-threads 2` and pass 20 of 20 at `--test-threads 1`. Neither
  half holds here: 0 hangs in 40 runs of that pair at `--test-threads 2`, 0 hangs in 25 runs of
  the whole `trace::` module at `--test-threads 2`, and the pair *fails* rather than passes at
  `--test-threads 1` -- for the unrelated reason that became `M-T10.6`.

  **The fix is justified analytically, not by that measurement.** Two threads inside
  `with_others_suspended` can each enumerate the other and then suspend it, leaving neither
  running to resume the other; `SuspendThread` is documented as not guaranteeing the suspension
  is complete when it returns, which is the window. One `Mutex` removes the state entirely and
  costs nothing on a facility that installs hooks once per process. An unreproduced hazard that a
  cheap change makes unrepresentable is worth closing; what is not acceptable is recording it as
  measured when it was not.

- [x] **M-T10.4** -- **DECIDED 2026-10-02: hot-patching is exclusive, and the inline hooks spend
      that exclusivity knowingly.** Documented as a hazard rather than engineered around, and
      recorded as [Hot-patching is
      exclusive](../../DESIGN-NOTES.md#hot-patching-is-exclusive).

  **The decision reframed the finding, and the reframing is the useful part.** The review raised
  a patch window: suspension does not place a thread's instruction pointer *outside* the fourteen
  bytes being overwritten. True, and the narrower half. The larger half is that inline hooking is
  **process-exclusive** -- one slot per stub, no way to share or negotiate, and a second patcher
  captures the first one's jump as though it were the original. Setting
  `WINDOWS_THREADPOOL_TRACE_HOOKS` therefore gives up hot-patching those stubs for the life of
  the process, and the crate never undoes it.

  **Why documenting beats fixing.** Relocating trapped instruction pointers would close the
  window and would not make two patchers able to share a stub, because the exclusivity belongs to
  the technique and not to this implementation. The expensive fix buys the smaller half. The cost
  is better spent making the choice visible where it is made, which is what this item did: the
  module docs own the statement, the variable's own documentation and the README point at it.

  **The window itself is narrower than reported -- but not for the reason this item first gave.**
  Installation runs from the `.CRT$XCU` initialiser before `main`, so no other thread exists to be
  inside the range. This item originally asserted that as already true; a later review found it
  was not. Installation sat at the end of the public `observe_exceptions`, which `enabled` calls
  only when the trace is armed, so a process with `WINDOWS_THREADPOOL_TRACE_HOOKS` set and
  `WINDOWS_THREADPOOL_TRACE` unset could reach the installer after its threads existed. Closed by
  `M-T10.8`, which moved installation into the initialiser and sealed the window. What remains is
  `install_by_label`, which is `#[cfg(test)]`.

- [x] **M-T10.6** -- **Stop the recogniser's live canary reading a stub a sibling test has
      patched.** Found while trying to reproduce `M-T10.3`'s reported hang, which is the only
      reason it was found at all.

  **The defect.** `the_stub_recogniser_accepts_the_shape_and_rejects_everything_else` ended by
  resolving the `selftest` hook target and asserting the bytes there are a syscall stub -- the
  canary that says the recognised shape still describes this machine's Windows.
  `a_hooked_stub_records_both_ends_and_still_performs_its_syscall` patches that same stub and, by
  design, never removes it. Whichever ran first decided the answer, so the canary was asserting
  the planted jump rather than the shape Windows shipped.

  **Measured.** 25 of 25 runs of the `trace::` module failed at `--test-threads 2` before the
  fix and 0 of 25 after. It is an order dependency and not a thread-count race: running just the
  two tests single-threaded, with the hooking one first, failed too. The full suite passes only
  because the default thread count happens to order them the other way -- luck that changes with
  the test count, the machine, or a rename.

  **Fixed** by giving the canary `unhookable_stub_entry()`, an `ntdll` export deliberately absent
  from `HOOKS` and asserted to be absent, so this module cannot patch it however many hooks a run
  installs.

- [x] **M-T10.8** -- **Enforce the one-thread hook-installation window at the installer.** Found
      by the third review round, which falsified a claim `M-T10.4` had written into three
      documents one commit earlier.

  **The defect.** Hook installation sat at the end of `observe_exceptions`, which `enabled` calls
  only when the trace is armed. A process with `WINDOWS_THREADPOOL_TRACE_HOOKS` set and
  `WINDOWS_THREADPOOL_TRACE` unset therefore left the installer's once-flag unconsumed, and
  `observe_exceptions` is public -- so a later call from a running process would patch `ntdll`
  with the pool's threads alive, the one case the recogniser cannot make safe.

  **The shape is the lesson.** The guarantee was carried by *which function happened to call the
  installer*: correct while nobody moved the call, silently false afterwards, and checked by
  nothing. Fixed by moving installation into the pre-`main` initialiser, unconditionally and
  independent of the trace, then sealing the window; `install_requested` now refuses and records
  `refused-after-seal`. Guarded by `the_hook_installation_window_is_shut_before_any_test_runs`,
  verified by sabotage.

- [x] **M-T10.9** -- **Take no process lock while the other threads are suspended.** Found by the
      third review round.

  **The defect.** The suspended window took three process-wide locks: the allocator (through
  `ntdll_proc`'s owned name, and through the suspend loop consuming its enumeration vector by
  value so the `Vec` was freed with threads already stopped), the loader (`GetModuleHandleA`,
  `GetProcAddress`), and the memory manager (`VirtualAlloc`, `VirtualProtect`). A thread stopped
  holding any of them can never give it back.

  **The shape is the lesson, again.** The window's own comment said "Nothing in here may
  allocate", and the enumeration vectors were reserved ahead of time for exactly that reason, with
  a control in `sabotage.json` recording it -- while the install the window wrapped called the
  loader three lines later. The rule was stated at the allocation somebody noticed rather than at
  the window it belongs to.

  **Fixed** by splitting install into prepare / commit / finish: everything that can take a lock
  happens while the process is still running, and the window holds a single fourteen-byte store.

- [x] **M-T10.10** -- **Mark a cancelling group release's repairs after the cancellation, not
      before.** Found by the third review round.

  **The defect.** `CleanupGroup::close_members(true)` marked each member's pool as owing a repair
  *before* calling `CloseThreadpoolCleanupGroupMembers`, justified in a comment by the claim that
  the native release frees the member contexts. It does not -- this crate frees them, in the loop
  immediately after. So the justification was false and the ordering it bought was harmful: the
  healer could see the mark, find the pool still dispatching, clear it as repaired, and then the
  real cancellation would happen with no mark outstanding. An unrepaired wedge from a **single**
  cancellation, where the overlapping-cancellation race already recorded in `M-T9.2` needs two.

  **Guarded.** `a_cancelling_release_marks_its_pool_after_the_cancellation` installs a test-only
  hook that clears the mark at the instant before the native release, standing in for the healer's
  tick landing there -- which is necessary because both orderings leave a mark outstanding once
  the release has returned, so no end-state assertion can tell them apart. Verified by sabotage:
  restoring the old ordering fails it.

- [x] **M-T10.11** -- **Give the worker-factory snapshot named fields.** Found by the third review
      round.

  **The defect.** `worker_factory_snapshot` returned `(usize, u32, u32, u32)` and its doc comment
  described `(handle, total, waiting, pending)` -- every field after the handle shifted by one
  from what the code returns, which is the handle, the thread *maximum*, the total workers and the
  waiting workers. A caller who believed the comment read the configured maximum as the number of
  workers that exist, so a cold or stalled pool looked fully staffed: the exact inverse of the
  reading the data is gathered for. There is no `pending` field at all.

  **Fixed at the build rung, not in the comment.** The return type is now
  `WorkerFactorySnapshot` with named fields, so the mismatch is unrepresentable. Nothing could
  have caught the tuple version: both halves type-check, and a tuple carries no statement about
  which field is which for anything to check against. The consuming tests destructured correctly,
  which is why the suite stayed green and the defect lived only in what a reader was told.

- [x] **M-T10.12** -- **Make the lifecycle trace test run, and expect what teardown actually
      calls.** Found by the third review round; the same inertness as `M-T10.2`, which was fixed
      without sweeping for siblings.

  **Two defects, one hiding the other.** The test early-returned when the trace filter was unset,
  which is every ordinary run and every CI run. Underneath that, it expected
  `WaitForThreadpoolWaitCallbacks(cancel)` and `WaitForThreadpoolTimerCallbacks(cancel)` -- forms
  that only `try_cancel_pending` makes, and that teardown stopped making when it changed from
  cancelling to draining. Run alone with the filter armed it failed; run in a full armed suite it
  passed, because sibling tests supplied those records into the one process-wide buffer and
  `counted` only asks for a non-zero count.

  **Fixed** by re-executing the one test single-threaded in a child with the filter set, and by
  correcting the two expectations. The child exits with a distinctive code, so a filter that
  matched nothing -- a rename -- fails loudly instead of exiting 0 and looking like a pass.
  Verified by sabotage in both directions: the old labels fail the body, and a wrong test name
  fails the parent.

  **The sweep this time:** two tests had the early-return shape;
  `the_exception_observer_notes_a_first_chance_exception` passes when armed.

- [x] **M-T10.13** -- **Restore page protection once per page, not once per stub.** A defect
      introduced by `M-T10.9`'s own fix, found by the fourth review round.

  **The defect.** `VirtualProtect` reports the previous protection of the whole **page**, not of
  the byte range asked about. Splitting install into prepare / commit / finish moved protection
  to one save-and-restore per stub, so two stubs sharing a page had the second save the writable
  state the first had just installed -- and the last restore left `ntdll` executable **and
  writable** for the rest of the process's life. `park`, `set-info` and `shutdown` share a page on
  this host. The single-phase install it replaced did not have this, because it protected and
  restored around each store in turn.

  **Guarded** by `stubs_sharing_a_page_are_opened_once_and_the_page_is_restored`, hermetic on a
  page the test allocates rather than on `ntdll` -- the real stubs are patched once per process,
  so whichever test ran first would decide the answer. Verified by sabotage.

- [x] **M-T10.14** -- **Read `AlreadySignaled` only on a successful association.** The
      `associate` hook read its out-parameter unconditionally after forwarding the call. A failed
      `NtAssociateWaitCompletionPacket` need not have written through that pointer, or validated
      it, so the instrument could turn an ordinary error return into an access violation raised
      by the tracing facility itself. A failed association has nothing to report there anyway:
      no packet was associated, and the `-leave` record already carries the status.

- [x] **M-T10.15** -- **Make the periodic-timer exercise reach its second re-arm entry point.**
      The callback switched on a `load` taken *after* its own `fetch_add`, so the second firing
      compared 2 against 1 and the `rearm_at` arm was unreachable: the exercise finished a firing
      early having driven only `rearm_after`, while its comment claimed both. Both entry points
      emit the same event tag, so neither the test nor the trace assertion it feeds could notice.
      Now switches on the value `fetch_add` returns.

- [x] **M-T10.16** -- **Run every trace assertion in a trace-armed child, from one site.** The
      sweep for `M-T10.12` asked the wrong question -- it grepped for early `return`, while
      `io::tests`' two trace blocks are `if wants(..) { .. }` -- so both were still inert.

  **Fixed at one site rather than a fourth copy.** `trace::in_a_trace_armed_child` now owns the
  parent/child protocol; the lifecycle test and both `io::tests` blocks go through it. The first
  two hand-written copies had already disagreed about how to distinguish a child that ran from
  one whose filter matched nothing. Verified by sabotage: removing the `start-cancelled` record
  fails the child on its assertion and the parent on its exit-code check.

  **The real population**, found by grepping for `wants(` rather than for `return`: four sites.
  Two were these; one is `the_filter_narrows_to_the_targets_named`, where the gating *is* the
  subject; one is the hook records already queued as `M-T10.5`.

- [x] **M-T10.17** -- **Bound the warm-up's teardown on the path where its callback never
      arrived.** `prewarm_default_pool` timed out after two seconds and then called
      `stop_and_drain`, which waits with no deadline for the very callback whose absence caused
      the timeout -- so the bounded check was followed by an unbounded one and the `false` it
      exists to return could never arrive. It now cancels the queued invocation first, on that
      path only.

  **Not covered by a test**, and stated plainly rather than papered over: reaching the timeout
  needs a pool that has stopped dispatching, which is the stall this repository has spent `M-T7`
  failing to produce on demand.

- [x] **M-T10.19** -- **Handle the failures the patch path assumed away, and check the branch the
      trampoline does not rebuild.** Three findings from the fifth review round, all in
      `trace/hook.rs`, two of them the same recorded rule broken twice.

  **Two unchecked failable calls**, against [A failable call has its failure handled,
  always](../../DESIGN-NOTES.md#a-failable-call-has-its-failure-handled-always). The trampoline
  page's `VirtualProtect` result was discarded and the pointer published regardless, so a failure
  left the hook jumping into a page the processor will not execute. And `with_others_suspended`
  skipped past a thread it could not open or suspend and patched anyway -- while that thread was
  still running through the bytes being overwritten, which is the precondition the suspension
  exists to establish. Both now refuse, with their own `Refusal` variants.

  **The refusal had to learn a distinction, and a test is what taught it.** Refusing on every
  failure made `install_by_label` fail in an ordinary test process, because the thread snapshot is
  taken while the process runs and a pool starting and finishing workers routinely leaves a listed
  thread gone before `OpenThread` reaches it. `ERROR_INVALID_PARAMETER` names that case, and a
  thread that does not exist cannot be executing the bytes -- so it is skipped and everything else
  refuses.

  **An assumption the module did not admit to.** The recognised stub tests a shared-data byte and
  branches past `syscall`; the trampoline rebuilds only the fall-through, so "the same call by
  construction" held while that bit is clear and not otherwise. It is clear on ordinary x64, which
  is why it went unstated -- an observation about this machine rather than a property of the
  technique. `syscall_path_is_direct` now reads it, derived from the recogniser's own bytes rather
  than written out a second time, and an install refuses when it is set.

- [x] **M-T10.20** -- **Stop the fail-fast and the cancellation safety claim promising more than
      they detect.** Two findings from the fifth review round that are not defects in the code but
      in what it says.

  **The fail-fast** records an outstanding *arming*, not "this `Drop` blocked". A dispatch settles
  it for a wait and a one-shot timer while `Drop` still waits for that callback, so dropping
  during an executing callback drains without reporting -- and `CleanupGroup` carries no
  obligation flag at all. Both now stated at the one site that owns the message.

  **`try_cancel_pending`** is documented as safe because the crate repairs the pool afterwards.
  On a pool whose repair item could not be created there is no entry and nothing repairs it, and
  `owe_repair` returned in silence. It now records `cancel-untracked`, and both statements of the
  claim name the exception.

- [x] **M-T10.21** -- **Make the thread enumeration fail closed before anything is patched.** The
      sixth review round. `with_others_suspended` refused when a thread could not be opened or
      suspended, but every way of failing to *find* the threads reported success: a snapshot that
      could not be taken, an enumeration that could not be started, and a thread count past `ROOM`
      each left `ids` empty or short, which read as "everything is suspended" and patched fourteen
      bytes of live code with nothing stopped. Strictly worse than the per-thread case fixed one
      block above it in `M-T10.19`. An `enumerated` flag now gates `quiesced`, and the suspend loop
      is skipped entirely when it is false, so a refused install perturbs nothing.

- [x] **M-T10.22** -- **Stop a transient healer-start failure disabling self-heal for the process.**
      `ensure_running` cached its result in a `OnceLock<Option<Healer>>` built with `.ok()?`, so one
      failed `ThreadpoolPool::new()` -- a transient condition -- latched `None` permanently and
      silently, and no later cancellation anywhere in the process was ever repaired. Now an
      `AtomicBool` fast path over a `Mutex<Option<Healer>>`: a failure records `healer-start-failed`
      and leaves the slot empty, so the next cancellation retries.

- [x] **M-T10.23** -- **Isolate the heal tests from every other tick in the process.** `tick` walks
      the whole registry, so a private pool does not isolate a test: another test's tick, or the
      background healer's, can clear a mark between the cancellation that sets it and the assertion
      about it. A `TICK_GATE` now brackets each critical section.

  **Reproduction, stated honestly.** The review saw this fail on run 19 at 32 test threads. 60 runs
  here with the gate disconnected, and 60 more with a thread calling `tick` in a loop, did not
  reproduce it -- the window is microseconds wide, so missing it settles nothing. What settles it is
  placing a tick in the window by hand: inserting `tick_inner()` between
  `a_cancelling_group_release_marks_a_wait_members_pool`'s `close_members_cancelling()` and its
  assertion fails that test every time, on the assertion the review named.

- [x] **M-T10.24** -- **Retry a refused quiesce instead of weakening the refusal.** Found while
      verifying `M-T10.23`, not by the review: the full lib suite at 32 test threads failed 13 of 30
      runs in `trace::tests::a_hooked_stub_records_both_ends_and_still_performs_its_syscall` with
      `NotQuiesced`. Instrumenting rather than guessing named the cause -- `SuspendThread` returning
      `ERROR_ACCESS_DENIED`, enumeration intact -- which is a thread that is *terminating*.

  This was `M-T10.19` half-converted: the benign/dangerous distinction was given to `OpenThread` and
  not to `SuspendThread` one line below. The carve-out was **not** extended, because the two cases
  differ -- an id that names nothing is gone, whereas a terminating thread may still be running its
  exit path, which is the property the refusal establishes. The refusal stands and the whole attempt
  is retried outside the suspended window. Measured 13/30 before, 0/30 after.

- [x] **M-T10.25** -- **Split the cancelling group release out of `close_members(bool)`, and make
      posting fabricated packets `unsafe`.** Two findings of the same shape: a hazard gated by the
      build for one entry point and by prose for another.

  **`close_members(cancel_pending: bool)`** was safe and ungated in every configuration, while the
  per-object `try_cancel_pending` it reaches is `self-heal`-gated and `unsafe` without the feature.
  With the feature off, `close_members(true)` cancelled every wait member and landed on a no-op
  `owe_repair`. Now `close_members()`, `close_members_cancelling()` (gated), and
  `close_members_cancelling_no_heal_tracking()` (ungated, `unsafe`) -- a `bool` cannot be `cfg`-gated,
  an item can.

  **`poke_completion_ports`** is now `unsafe fn`. It posts a zero key and a null `OVERLAPPED` to
  every completion port in the process above a depth, including ports this crate does not own; the
  "**Destructive** ... only for a process that has already failed" rule was enforced by prose alone.

  Both breaking; both recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

- [x] **M-T10.18** -- **DECIDED 2026-10-02: a cancellation registers the pool itself, and reports
      it when even that fails.** `heal::register` is best-effort, so a pool whose repair item could
      not be created had nothing for a later `try_cancel_pending` to mark.

  **The decision.** The cancellation retries the registration at cancel time rather than giving up.
  Allocating at the moment of need is what the pre-created repair object exists to avoid, but that
  argument does not reach this path: the alternative is not "allocate earlier", it is "never repair
  this pool at all". If the retry also fails, `cancel-untracked` is recorded **with the pool key**
  and, under `fail-fast`, the call panics.

  **A prior defect surfaced while deciding it, fixed in the same change.** `Registration` held only
  `Option<Arc<PoolEntry>>` and discarded the key, so an object whose registration failed could not
  find an entry another object later created for the same pool -- the pool was repairable, a healthy
  entry existed, and the cancellation still reported it untracked. The same omission made
  `cancel-untracked` record `0` rather than naming the pool. `Registration` now keeps its key.

  **Where the fail-fast fires is load-bearing.** `CleanupGroup`'s cancelling release marks members
  between the native release and the loop that frees their contexts, so a panic raised there would
  unwind past the frees and leak every context. The report is accumulated and acted on after the
  frees, matching `fail_fast_if_owed`'s rule that the panic must report a violation and not cause
  one.

  **The error edge is now reachable from a test.** It runs only when `CreateThreadpoolWork` fails,
  which no test can arrange, so a test-only `FORCE_REPAIR_FAILURE_FOR` forces it -- keyed to one
  pool, because a global flag failed an unrelated test's registration on the first run. Both
  directions are asserted, and sabotaging the retry fails the recovery test by name.

  Recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md#a-cancellation-allocates-its-own-repair-rather-than-giving-up).

- [x] **M-T10.26** -- **Release a trampoline page when the batch that prepared it is refused.** The
      seventh review round, and its only finding. `prepare` allocates a 4 KiB executable page per
      hook and publishes it into `TRAMPOLINES[index]` before the batch knows whether it will commit,
      because the patch it builds points at that page. Both post-preparation refusals -- `open_pages`
      failing, and the quiesce failing after all its attempts -- returned with the pages still
      allocated and still published. Bounded at one page per hook and once per process, but a leak.

  **The obvious fix would have introduced a use-after-free, which is why it is not the one taken.**
  `prepare` publishes unconditionally, so if an index were already installed its live, patched stub
  would be jumping through the slot this preparation just overwrote; freeing there would release a
  page a running hook is inside. `discard_prepared` therefore releases only when it found the slot
  **empty**, and restores the displaced pointer otherwise. Only the test-only `install_by_label` can
  reach the non-empty case -- `install_requested` runs the batch once per process behind `DONE` --
  but the branch exists because the function must be correct for its callers rather than for the one
  that happens to exist today.

  **Verified on the real path rather than by reading.** Forcing `open_pages` to return `None` and
  running the hook test printed `DISCARD index=5 previous=0 slot_now=0` and reported `NotWritable`:
  the cleanup runs, takes the releasing branch, and clears the slot before the free. Neither refusal
  branch is reachable from a test without that kind of injection, which is pre-existing and remains
  true.

  The macro's `SAFETY` note that the slot is "never cleared" was corrected in the same change; it is
  now cleared on exactly one path, and the comment states why that cannot race a call already inside
  the hook body.

- [x] **M-T10.27** -- **Run the exception-observer assertion in a trace-armed child.** The eighth
      review round's headline finding, and this branch's own signature defect recurring:
      `the_exception_observer_notes_a_first_chance_exception` opened with
      `if !wants("exception") { return; }`, so in every ordinary run and in CI it returned having
      raised nothing and asserted nothing. `M-T10.12` and `M-T10.16` exist to remove exactly this,
      and the remedy they introduced -- `in_a_trace_armed_child` -- was already applied at three
      other sites. Measured before the fix: the test passed in 0.00s with the filter unset, and
      since its only other exit is a `panic!`, passing was itself the proof it returned early.
      Measured after: 0.06s, and sabotaging the recorder fails it by name.

- [x] **M-T10.28** -- **Stop the healer parking on the one pool it is trying to repair.** `tick`
      submits a repair, clears the mark, then calls `retire_idle` in the same pass. An entry whose
      objects are gone is then idle, so the last `Arc` drops, `RepairWork::drop` drains the work
      object -- and that drain cannot return until the pool dispatches. The pool is by construction
      the one suspected of not dispatching, and the healer is built with `set_max_threads(1)`, so
      one wedged pool parked the only thread the facility has and no pool in the process would ever
      be repaired again.

  `retire_idle` already moved the drop out of the registry lock for this exact reason, and its
  comment says so; that protected every *other* pool's registration from the wait but not the
  healer's own thread. Retirement now also requires `repair_settled()` -- a submission counter
  paired against the existing `runs` -- so an entry is retired only once the pool has given every
  repair back. An entry kept alive for a pool that never dispatches is the cheaper failure by a
  wide margin. The counter is incremented *before* the submit, because a repair can be dispatched
  the instant it is handed over and a count taken afterwards could be overtaken by the run it is
  meant to be paired against.

- [x] **M-T10.29** -- **Bound the writable window to one attempt, not sixteen.** A defect introduced
      by `M-T10.24`'s retry. `install_batch` opened the `ntdll` pages, called the retrying
      `with_others_suspended`, and restored them only afterwards -- so a contended install held live
      code pages `PAGE_EXECUTE_READWRITE`, with every other thread running, across all sixteen
      attempts and their snapshots, where one attempt holds them for microseconds. The retry now
      lives in `install_batch` and opens and restores per attempt; `quiesce_once` is a single
      attempt again and its doc records why the retry cannot wrap it. `NotWritable` is not retried,
      because a page this process cannot make writable will not become writable a millisecond
      later. Re-verified that the flake `M-T10.24` fixed stays fixed: 0 failures in 30 runs at 32
      test threads.

- [x] **M-T10.30** -- **Carry the factory handle on the two records the investigation turns on, and
      five smaller corrections.** `counts-waiting` and `counts-pending` passed a literal `0` while
      `read_one`'s doc promised every record carries the handle -- and those two are precisely what
      the facility exists to read. `counts()` reads every factory in the process rather than
      guessing which is the default pool's, so without the handle a two-factory capture could only
      be attributed by row adjacency. Both now carry it, and the doc names the records that
      genuinely spend the slot on a partner value.

  Also: the module doc linked `../../../windows-ioring-sys/STALL-TIMELINE.md`, a path `b4a0a886`
  emptied when it re-homed that file here; `commit`'s safety contract credited `prepare` with
  opening the page, which `prepare` explicitly does not do (`open_pages` does, per `M-T10.13`);
  `fail-fast` appeared in neither the README nor the crate docs, leaving its build-unification
  hazard stated only in a `Cargo.toml` comment; the `compile_fail` guarding the feature-off API
  shape called a made-up method and so asserted only that `rustc` rejects unknown names -- it now
  names the real method under `cfg_attr(not(feature = "self-heal"))`, verified bidirectionally by
  forcing the block on with the feature enabled and watching it fail; and `Win32_System_IO` under
  `trace` was redundant against the unconditional dependency feature and justified by a comment
  calling it test-only, which `poke_completion_ports` being public contradicts.

- [x] **M-T10.31** -- **Claim a member's repair before the release that can free its pool.** The
      ninth review round, and a use-after-free introduced by `M-T10.18`'s cancel-time retry. A wait
      whose pool could not be registered holds nothing that keeps that pool alive -- the repair work
      object, which would be a bound object deferring `CloseThreadpool`, was never created. A
      cancelling group release frees the last bound object and *then* marks, so the retry inside
      `owe_repair` could call `CreateThreadpoolWork` against freed memory, reachable through the
      safe API. `release_members` now takes each member's claim **before** the native release and
      holds it across, which both creates that object and keeps the pool alive. The per-object path
      was never exposed: `self` is a live bound object for the whole call.

- [x] **M-T10.32** -- **Give retirement one predicate instead of two.** `M-T10.28` added the
      outstanding-repair condition to `retire_idle` and not to `release`, so the hazard it was added
      for -- `RepairWork::drop` draining an undispatched repair on the healer's only thread -- was
      still reachable through the other site. Both now call `is_retirable`. This is the fourth
      partial fix of this shape on this branch; the answer is one definition, not a third careful
      reading.

- [x] **M-T10.33** -- **Make `close_members_cancelling_no_heal_tracking` actually not track.** It
      called `release_members(true)`, whose repair pass was unconditional, so the method named for
      *not* tracking marked pools, allocated repair objects, started the process-lifetime healer,
      and could panic under `fail-fast` -- a contract its own documentation denies, and the opposite
      of what a caller diagnosing a stall asks for. Tracking is now a parameter the safe form passes
      and this one does not, which also stops the fail-fast firing for a caller who took the
      obligation deliberately.

- [x] **M-T10.34** -- **Check the patch-window precondition rather than asserting it from
      placement.** The module documented the hazard exactly -- a suspended thread's instruction
      pointer can be inside the bytes being replaced -- and then claimed the supported path avoids
      it "because it patches when the process has no other threads", justified by running from a
      `.CRT$XCU` initialiser. Placement does not establish that: an initialiser ordered earlier may
      have started threads, and in a DLL the same initialiser runs at attach inside a running
      process. No instruction pointer was ever read.

  Each suspended thread's `Rip` is now compared against the ranges about to be written, and the
  install refuses when one is inside. Verified by sabotage: forcing the comparison to match refuses
  with `NotQuiesced`. The `CONTEXT` buffer is wrapped in a 16-byte-aligned type because
  `GetThreadContext` documents that alignment on this architecture while `align_of::<CONTEXT>()`
  from `windows-sys` measures 8 -- it worked without the wrapper, which is incidental behaviour
  rather than a contract.

- [x] **M-T10.35** -- **Stop the hook tests failing whenever hooks are enabled.** The initialiser
      installs whatever `WINDOWS_THREADPOOL_TRACE_HOOKS` names before any test runs; the two
      installation tests then installed again, met their own patch, and failed on the recogniser's
      correct `NotAStub` refusal. The suite was green only because nothing ever set the variable.
      Reproduced at `exit 101` before the fix. They now install only when `installed_by_label`
      reports the stub is not already patched, which does not weaken rejection of a foreign patch.

  One assertion was wrong in the same configuration for a different reason: it required
  `factory_handle()` to be zero, but `FACTORY` is process-wide and the worker-factory hooks
  legitimately write it, so with every hook installed a real handle is already learned. The claim is
  that *this* call contributes none, so it now compares against the value captured before the call.
  Measured after: 299 pass with hooks unset, `*`, `selftest`, and `associate`.

- [ ] **M-T10.7** -- **Decide whether CI should check intra-doc links in private items.**

  **The gap.** The `docs` job runs `cargo doc --no-deps --all-features` without
  `--document-private-items`, so rustdoc never resolves links written in the docs of private
  modules and functions. Two broken ones had accumulated unnoticed and were found only because
  `M-T10.4` ran the stricter form by hand: `callback_env.rs` linked a bare `set_pool` where the
  associated-item path was needed, and `hook.rs` linked a `#[cfg(test)]` function that does not
  exist in a documentation build. Both are fixed; nothing stops the next two.

  **Why it is a decision rather than an obvious yes.** `--document-private-items` also surfaces
  links that are correct for a maintainer reading the source but meaningless in published docs,
  and this crate's private modules carry a lot of prose. Turning it on in CI may mean either
  accepting that noise or rewriting those links to a form that satisfies a build nobody reads.
  Worth weighing against simply running it by hand when a private module's docs are edited.

- [ ] **M-T10.5** -- **Make the hook tests' trace-record assertions reachable, so a sabotage can
      reach them.**

  **The gap.** `sabotage.json`'s `notCoveredHere` records that the trace records a hook emits are
  deliberately absent from the sweep: they are asserted only when `WINDOWS_THREADPOOL_TRACE` is
  set, which neither the harness nor CI does, so a sabotage removing those `record` calls would
  be reported SURVIVED for a guard that exists. The manifest's note cited `M-T4.1` for this, which
  is a completed decision about the name of the synchronous close and never covered it -- so the
  work was, in practice, queued nowhere.

  **Target.** Adopt the technique `M-T10.2` put in the tree: `tests/obligation_report.rs`
  re-executes its own binary as a child with the filter set, which makes a traced assertion run
  under a plain `cargo test`. Apply it to the hook tests, then add the record sabotages to
  `sabotage.json` and confirm each is caught rather than survived.

## M-inf -- Diagnostic work with no gating deliverable

- [ ] **M-T-inf.1** (was `M26.14.4`) -- **Find the threshold the close races.** `M26.14.2` used 1ms because it is
  about 80x the natural 12us gap; nothing establishes what the minimum is. A sweep -- 0, 10us,
  50us, 100us, 500us, 1ms, interleaved at 20000 each -- would say whether the window is
  microseconds or milliseconds, which is itself evidence about what the close races.

  **Moved here 2026-10-01, when its gate resolved the "never ships" way.** This item was gated on
  `M26.14.3` and said so itself: if the draining teardown were adopted, the sleep never ships and
  the sweep is only of diagnostic interest. It was adopted, so that is where this now sits -- not
  cancelled, because the question it asks is about the *kernel* window the close races, which the
  drain avoids rather than explains, and that remains the one thing about this stall nobody has
  been able to see directly.
