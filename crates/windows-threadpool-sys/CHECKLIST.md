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

- [ ] **M-T9.2** -- **Replace the repair mark with three derived timestamps, and point the
      trampoline at the entry.**

  **The defect.** `owe_repair` records a cancellation with `compare_exchange(0, at)` -- a no-op
  while a mark is already outstanding, which is the deliberate "keep the earliest cancellation"
  policy. But `clear_repair` is an unconditional `store(0)`. The two do not pair, so a
  `try_cancel_pending` landing between `tick`'s read of the mark and its clear is recorded
  **nowhere**: the CAS fails because the slot still holds the first stamp, and the clear then erases
  both. The repair `tick` submitted happened strictly *before* that second cancellation, so it
  cannot repair whatever it severed. A pool is left wedged with nothing scheduled, silently -- the
  exact failure this feature exists to prevent. Both `tick` branches are affected; the
  `dispatched_since` skip is arguably worse, because there nothing was submitted at all.

  **The shape, decided 2026-10-02 (the engineer's design).** Stop storing health and derive it from
  three monotonic stamps, each written by an **unconditional** store that cannot fail the way a CAS
  can:

  ```text
  cancel:      last_cancelled.store(now)
  tick:        if last_cancelled > last_started      // nothing has run since
                  && last_cancelled > last_submitted // and one is not already in flight
               { submit; last_submitted.store(now) }
  trampoline:  last_started.store(now)

  healthy  <=>  last_started > last_cancelled
  ```

  No clear, no CAS, no lost update. `unhealed` is the *name* of that comparison rather than a field,
  which is [prefer a derived fact to a restated
  one](../../.github/copilot-instructions.md) applied to state instead of prose. It also replaces a
  weaker signal: `dispatched_since` depends on *user* objects' trampolines stamping the entry, so a
  pool with no other activity is indistinguishable from a wedged one, whereas our own repair
  dispatching is direct evidence.

  **The trampoline takes a pointer to the `PoolEntry`**, not to a side allocation. All four values
  then live on the entry the callback is about. The current `Box<AtomicU64>` never bought any
  safety -- its premise was that the box's lifetime was easier to guarantee than the entry's, and
  since `Drop` did not drain, the box was dangling too. It made the use-after-free *smaller*, not
  absent, and it is what disguised it.

  > **This requires moving `repair` to the FIRST field of `PoolEntry`.** The struct has no `Drop`
  > impl, so its fields drop in declaration order, and `repair` is currently declared **last** --
  > meaning the timestamps would be freed before `RepairWork::drop` drains the work object that
  > writes to them. First-field placement makes the drain run before anything it protects is freed.
  > Order the fields the way [`EventDelivery`](../windows-ioring-sys/src/event_delivery.rs) orders
  > `wait` before `ring`, and say why at the field.

  **Two details to decide deliberately rather than discover:**

  - **Equal stamps are common, not rare.** `QueryInterruptTime` has system-tick resolution
    (about 15.6 ms), so `last_cancelled == last_started` will happen often. Treating equality as
    healthy risks declaring health when the cancellation actually followed the dispatch inside one
    tick. Require strictly `>`, erring toward a redundant repair -- which this crate already
    establishes is the safe direction.
  - **A wedged pool never retries.** With `last_cancelled > last_submitted` as the guard, one
    repair is submitted per cancellation; if the pool is genuinely wedged that submit never
    dispatches and nothing tries again. The current code has the same property, so this is not a
    regression -- but decide whether a repair unstarted after N ticks should be re-submitted.

- [ ] **M-T9.1** -- **Write the test that would have caught the repair-object use-after-free.**

  **What the defect was.** `RepairWork::drop` called `CloseThreadpoolWork` without draining first,
  then freed the `Box<AtomicU64>` whose *address* is that work object's callback context.
  `CloseThreadpoolWork` does not wait -- it frees the work object asynchronously once outstanding
  callbacks finish -- so a repair submitted and not yet dispatched would `fetch_add` through freed
  heap. Fixed by draining before the close, mirroring `ThreadpoolWork::drop`, which had always done
  it correctly.

  **Why nothing caught it, which is the part worth fixing.** Every test in `heal/tests.rs` holds its
  own `Arc<PoolEntry>` clone, and usually a live object too, so the entry is never retired while a
  repair is in flight -- the precise condition the defect needs. The sabotage manifest inherited the
  same blind spot. A guard has to drop the last `Arc` between the submit and the dispatch.

  **`windows-guard-alloc` is the instrument**, and is already a workspace member: a guarded
  allocation for the entry would fault on the write rather than silently corrupting whatever took
  the freed block. Reaching the window reliably may need the repair trampoline to be delayed under
  a test-only hook, since the race is microseconds wide.

  **Sabotage it**: with the drain removed the guard must fail, and the failure must be the write
  through freed memory rather than a timeout -- a crash-caught mutant is treated as uncovered here.

  **This guard covers `M-T9.2` as well**, and is the reason the two belong together: pointing the
  trampoline at the `PoolEntry` makes the field-order invariant above load-bearing, and this
  repository has been bitten by a field-order assumption twice in one evening already
  (`flush_barrier_stress`'s `Fixture`, and the `TempPath` ordering corrected in
  `test(ioring): order the temp-file guard before the handle it protects`). A comment is not a rung;
  this test is.

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

- [ ] **M-T10.4** -- **DECISION TO RAISE: whether hot-patching a running process is a hazard this
      crate accepts.**

  **The finding.** `trace/hook.rs` overwrites a 14-byte region of a running stub after suspending
  the other threads. Suspension does not place a thread's instruction pointer *outside* that
  region: a thread stopped at an instruction boundary inside the overwritten range resumes into
  what is now jump-displacement data.

  **Why this is a decision and not a bug to fix.** The cost of closing it (relocating trapped IPs
  into a trampoline, or constraining installation to a point at which no other thread can be
  inside a stub) is real, and whether it is worth paying depends on something the engineer owns:
  whether these hooks are diagnostic-only instruments that a developer installs deliberately, or
  a facility a consumer may install under load. The reviewer's confidence that the hazard exists
  is high; this item is about what to do with it, not whether it is there.

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
