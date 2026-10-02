# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
Everything through M18 is archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md): M1-M6
[here](COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m6-ring-lifecycle-through-consumer-documentation),
M7 [here](COMPLETED-CHECKLIST.md#moved-2026-08-23----m7-ring-copy-a-topology-aligned-sample), M11-M14 in their
own dated groups, M8-M10
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m8-through-m10-handle-lifetime-cross-ring-identity-and-the-contract-audit),
and M15-M18
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m15-through-m18-the-testing-strategy-response-to-eight-defects).

M19 is archived [here](COMPLETED-CHECKLIST.md#m19). `M20` through `M24`, and the `M21+`, `M22+` and
`M28+` buckets, are archived
[here](COMPLETED-CHECKLIST.md#moved-2026-10-01-m20-through-m24).

**An `M{n}+` heading is parked rather than pending** -- see the `M{n}+` convention: it is gated work
with no current obligation, not an unfinished milestone. Which milestones are open is what the
headings below say; this preamble deliberately does not restate it, because a second copy is one
nobody updates.

## M26+ -- The wakeup window review opened

- [x] **M26.12** -- Not a race: the setup signal was owed only to the call that attached the event, so a caller that attached earlier got no wakeup at all. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2612)

- [ ] **M26.13** -- **Find why the delivery stall's dispatch is delayed.**
  Re-opened and substantially narrowed on 2026-09-26; the measurements are in
  [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md) and are not repeated here, and one
  captured occurrence is walked through record by record in
  [STALL-TIMELINE.md](STALL-TIMELINE.md).

  **What changed.** The stall was believed fixed by [D-68](DESIGN-NOTES.md#d-68). It is not: it
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
  [measurements/2026-09-26-what-releases-the-stall/](measurements/2026-09-26-what-releases-the-stall/README.md).
  So the remaining question is a **pool** question -- why a queued wait callback waits for an
  unrelated `SubmitThreadpoolWork` -- where this item had been framed as a **ring** question.

  **Re-planned again 2026-09-27 after `M26.13.4`, and the item is no longer about this crate.**
  During the stall the pool dispatches **no callback of any kind**: a wait, a timer, and an I/O
  object each created and armed *during* the stall, with no connection to any ring, all sit
  undispatched for two seconds and then run only once a work item is submitted. Fifteen captures in
  [measurements/2026-09-27-which-poke-releases-the-stall/](measurements/2026-09-27-which-poke-releases-the-stall/README.md).
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

     > **-> CROSS-COMPONENT HANDOFF:** a remedy, if one is adopted, most likely lands in
     > [windows-threadpool-sys](../windows-threadpool-sys/CHECKLIST.md) or in what this crate passes
     > it as an environment -- not in the ring. Decide where before building it.

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

- [x] **M26.13.1** -- Paired entry/exit records added to all five pool trampolines, the re-arm, and the test's post-mortem path; a sabotage sweep confirms each is load-bearing. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m26131)

- [x] **M26.13.2** -- The buffer is per-process, so the population was never "the full suite"; the capturing binary emits 89 records, the threadpool crate's own 14061, and an overflow now announces itself. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m26132)

- [x] **M26.13.3** -- The stall ends when a work item is queued to the pool, at no other time, and the five-second coincidence is the probe's timing rather than a timer. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m26133)

- [x] **M26.13.4** -- Experiment 3: nothing else releases it, and nothing else dispatches either -- a wait, a timer and an I/O object armed during the stall all sit undispatched, so the fault is pool-wide and not this crate's. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m26134)

- [x] **M26.13.5** -- Experiment 2: a private pool does not stall in 12000 runs, but the thread minimum is not why -- the no-minimum arm is already clean, so the supply reading it was written to test is unsupported. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m26135)

- [x] **M26.13.6** -- The pool has 6 threads while stalled and 8 or 9 right after the work submit, so it has no worker and makes one; runs-long does not change that, and no callback of ours is holding a thread. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m26136)

- [x] **M26.13.7** -- With call-boundary and exception tracing on, no Win32 call blocks during the stall: 707 bracketed calls across 24 captures all returned, slowest 220us. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m26137)

- [x] **M26.13.8** -- The reproducer's own 5000ms submit timeout is not the five seconds: changed to 4000ms, dispatch still resumed at five in 14 of 14. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m26138)

- [x] **M26.13.9** -- It never self-releases. With the probe removed the delivery never arrives in 65s, so this is a permanent hang and the `delayed dispatch` claim was an artifact of the instrument. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m26139)

- [x] **M26.13.10** -- Not a missed wake: re-signalling the event the wait is armed on releases nothing in 5 of 5, with every SetEvent's success recorded. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261310)

- [x] **M26.13.11** -- A dump taken while stalled shows three pool workers parked idle in `ZwWaitForWorkViaWorkerFactory`, so the pool is not starved of threads and `M26.13.6`'s reading was wrong. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261311)

- [x] **M26.13.12** -- A self-rearming timer armed while the pool was healthy, due inside the stall window, fires only at the release in 10 of 10 -- so the fault is dispatch, not registration. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261312)

- [x] **M26.13.13** -- At a 100ms period the heartbeat becomes a clock: its first expiry is already missed and no pool callback of any kind is dispatched before the release, in 18 of 18. The pool never starts.  -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261313)

- [x] **M26.13.14** -- At the 15.625ms system tick the heartbeat misses 320 consecutive expiries and the trace window is empty end to end, in 13 of 13; the onset is bracketed to the first 15.7ms, and the sub-millisecond period item 0 asked for is not reachable this way. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261314)

- [x] **M26.13.15** -- Delaying only the `SubmitThreadpoolWork` call by up to 2000ms moves the delivery with it in 99 of 99, the released worker serves the queued wait ahead of the work that woke it, and an `ntdll` census corrects "user-mode queue push" to "the only path that calls `NtReleaseWorkerFactoryWorker`". -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261315)

- [x] **M26.13.16** -- No thread alive at the stall ever runs a callback, in 12 of 12, while a passing run serves the delivery on a pre-existing thread in 30 of 30: the parked workers are present and unused, so `M26.13.11`'s "not starved" survives but the inference drawn from it does not. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261316)

- [x] **M26.13.17** -- Inline ntdll hooks and a worker-factory scan: the process holds two factories, and the default pool reports zero workers while stalled against one while healthy, so `M26.13.11`'s three parked workers were the *other* factory's all along. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261317)

- [x] **M26.13.18** -- Hooks installed before `main` show the pool's first worker is never created: a healthy run makes one 0.24-0.31ms after the delivery is armed, a stalled run makes none before the release in 8 of 8, and the `AlreadySignaled` race is refuted. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261318)

- [x] **M26.13.19** -- An ETW kernel trace confirms the missing worker independently of the ntdll hooks: across 900 traced runs exactly one process has a gap over a second, the failing one, at 5008.7ms against a healthy 0.232-17.928ms. -> [completed 2026-09-27](COMPLETED-CHECKLIST.md#m261319)

- [x] **M26.13.20** -- Re-verified the reproducer's premises from a clean build: the 3-test reduction matches the full binary (16 vs 18 in 4000), the trigger is necessary (0 in 4000 without it), one victim suffices at about half the rate, and "fail together, never singly" was wrong. -> [completed 2026-09-28](COMPLETED-CHECKLIST.md#m261320)

- [x] **M26.14.1** -- The trigger's *teardown* is what poisons: built and never dropped it gives 0 in 4000 against a live control's 11, and a ring dropped without a delivery also gives 0. -> [completed 2026-09-28](COMPLETED-CHECKLIST.md#m26141)

- [x] **M26.14.2** -- Closing the wait too soon after disarming it is the poison: a 1ms gap between the disarm and the close, or a true drain in place of a cancel, each give 0 in 20000 against a control's 10. -> [completed 2026-09-28](COMPLETED-CHECKLIST.md#m26142)

- [x] **M26.14.3** -- Taken, and taken the way this item proposed: `windows-threadpool-sys` adopted
  the draining teardown. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m26143)

- [x] **M26.15** -- Re-measured: control 12 in 4000, current 0 in 12000, and the stall's entry moved
  to [RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md). The item's own premise was wrong and is
  corrected in the archive. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m2615)

- [x] **M26.14** -- A self-removing `TempPath` guard, shared by 14 test files: an all-passing suite
  run went from 25 leaked files to 0, and a test panicking with its handle open now leaves none.
  The item's "none clean up" was wrong; 12 of 15 did, just not on the panicking path. ->
  [completed 2026-10-01](COMPLETED-CHECKLIST.md#m2614)

## M27 -- What this crate owes the topology planner

**Re-planned 2026-09-23, the same day it was written.** M27 was originally "Adaptivity: the benefit
without the architectural commitment", and asked whether *this crate* should derive a partition for a
consumer who expresses no preference. That was the wrong owner, and the checklist rules require
saying so rather than quietly rewriting it. The adaptivity the
[adoption thesis](../../DESIGN-NOTES.md#the-adoption-thesis) asks for is delivered by
[topology-planner](../topology-planner/COMPONENT.md), which takes a dataflow description of the
application and returns one or more suggested realizations
([EP-D-6](../topology-planner/DESIGN-NOTES.md#ep-d-6)). Had the original M27.1 been answered here it
would have grown a second, weaker policy surface beside the one that component exists to provide --
the `outermost_partitioning_cache` defect again, where a policy answer lands in a crate whose job is
something else.

> **-> CROSS-COMPONENT PREREQUISITE:** `M27.1` and `M27.2` are gated on component
> `crates/topology-planner` -> `M1+` -> `EP-1+.5` and `EP-1+.6`, which decide the plan vocabulary
> this crate would be realized from. See [CHECKLIST.md](../topology-planner/CHECKLIST.md).

**What survives here is the realization end, not the policy end.** The planner emits a plan; the
outward adapter realizes it as buffers, rings and threads
([EP-D-5](../topology-planner/DESIGN-NOTES.md#ep-d-5)). That adapter is a separate crate, but it can
only build what this crate exposes, and nothing has ever checked that what it exposes is sufficient.
[D-8](DESIGN-NOTES.md#d-8) is untouched by all of this: policy stays out of this crate, and being
*constructible from* a policy decision made elsewhere is the opposite of taking one.

- [ ] **M27.1** -- **Census what a realizer would need from this crate, against the plan vocabulary,
  and name what is missing.** A plan states which processor a domain pins to, which memory node its
  pool allocates from, how many queues of which types, and where each channel's buffer lives. Walk
  each of those to the public API that would realize it and record the gaps. `NumaBuffer`
  ([D-51](DESIGN-NOTES.md#d-51)) is one half of the pool answer and arrived this month; the ring's
  own construction takes no placement input at all. **The output is a gap list, not an API** --
  proposing surface before the plan vocabulary is settled would be binding to a draft.

- [ ] **M27.2** -- **Gated on `M27.1` and on the planner's `EP-1+.6`.** Close the gaps the census
  names, as ordinary capability on this crate with no policy attached. Each gap is an input a caller
  supplies, never a choice this crate makes. Verify the way the thesis demands rather than the
  convenient way: construct from a plan built against a *synthetic* machine, since the planner is
  mockable by construction and this crate should be realizable without the hardware the plan
  describes.

- [ ] **M27.3** -- Give a consumer the means to answer placement questions on their own hardware.
  **Not gated on the planner** -- it is the client-side half of the thesis, and it is what lets a
  developer disagree with any plan they are handed. `cache_domains.rs` now prints every cache level
  beside the heuristic's pick; the equivalent for placement is a sample that reports what a chosen
  arrangement costs and what the alternatives would have cost, on the machine in hand.
  [ring_copy](examples/ring_copy) is the natural host, being already policy-selectable. **Do not ship
  a verdict** -- report the observation and let the consumer conclude, per OPTION INTEGRITY.

## M28 -- The ring owns the pending inventory

Queued by [D-55](DESIGN-NOTES.md#d-55), taken 2026-09-23 after the `M23.3` exploration. The break
is accepted deliberately: `IoRing` becomes generic so the inventory cannot drift from the ring,
because a consumer never holds a token to lose. The exploration and everything it falsified is in
[DESIGN-SESSION-2026-09-23-pending-inventory.md](design-sessions/DESIGN-SESSION-2026-09-23-pending-inventory.md);
`src/pending.rs` is the working spike and is the shape the internal map starts from.

**Sequenced so each step compiles.** The published crate is at 0.3.1, so this is a major bump and
every consumer names the type -- which means the migration order matters more than usual.

- [x] **M28.1** -- Decided: a push returns a `Copy` identity that owns nothing, and the ring returns the payload at its own pop -- `Token` is split, not moved. Recorded as [D-71](DESIGN-NOTES.md#d-71). -> [completed 2026-09-25](COMPLETED-CHECKLIST.md#m281)

- [x] **M28.2** -- `RingContract` is bounded by operations in flight: terminal entries are retired, and a capped history keeps a duplicate distinguishable from an unrecognised completion. Recorded as [D-72](DESIGN-NOTES.md#d-72). -> [completed 2026-09-25](COMPLETED-CHECKLIST.md#m282)

- [ ] **M28.3+M28.4** -- **Make `IoRing` generic, move the inventory inside, and migrate every
  consumer, as one commit.** Gated on [D-71](DESIGN-NOTES.md#d-71), which settled what a caller
  receives.

  **Merged deliberately, and the coupling is acknowledged rather than disguised.** The milestone
  header requires each step to compile, `M28.4` requires converting all consumers or none, and
  `M28.3` changes `IoRing`'s shape -- so the three cannot all hold with the items separate. The
  alternative considered and rejected was landing `M28.3` additively, with an `OperationId` path
  beside the existing `Token` one: it compiles at every step, but it leaves two token models live
  at once and the old one still lets a consumer lose a token, which is the defect `D-55` exists to
  remove. Never having both is worth one large commit.

  **Carry the sidecar.** The census found two thirds of consumers keep per-operation data beside
  the token, so an inventory holding only tokens serves a minority. Mixed-shape consumers use a
  closed `enum`; [generated_sequences.rs](tests/generated_sequences.rs) is the worked example.

  **Soundness already settled:** [`IoBuf`](src/buf.rs) is an unsafe trait whose contract requires
  the address to survive a move, so the ring may hold buffers in a map.

  Sequenced so the work is resumable, since it does not compile in the middle:

  - [x] **M28.3.1** -- `OperationId`: `Copy`, no `Drop`, a name and not a capability (`D-71`).
  - [x] **M28.3.2** -- `IoRing<T = ()>` carrying `inventory: HashMap<usize, T>`, and
        `Batch<'ring, T>` with it. A default keeps a payload-free consumer from naming `()`.
  - [x] **M28.3.3** -- Push stores the payload and returns an `OperationId`; pop returns the
        payload with the completion. Shape settled by [D-73](DESIGN-NOTES.md#d-73):
        `IoRing<T, X = ()>`, with the file guard held in a concrete internal `Held` rather than
        made generic, which is sound because `FileTarget` is sealed.

        **Do the `Drop` half in this step, not later.** Moving buffers into the ring removes the
        protection `Token`'s leak-on-unclaimed-drop provides on the path where `run_down` fails --
        which `Drop for IoRing` takes best-effort, closing anyway. The inventory must be
        *forgotten* rather than dropped there, or the close frees memory the kernel may still be
        writing into. Landing the inventory without this is a use-after-free, so it is one step.

        The tokenless shape is `M28.5`'s and is only accommodated here, not answered.
  - [x] **M28.3.4** -- Carry the parameter through `EventDelivery`, `RingScope` and the contract
        wiring.

        **The tree stops compiling here, as this plan said it would.** The delivery callback is
        now `Fn(Completion, Option<(T, X)>)`, so ten call sites across
        [event_delivery.rs](tests/event_delivery.rs), [handover.rs](tests/handover.rs),
        [checkpoint.rs](examples/epoch_log/checkpoint.rs) and
        [model_a_delivery.rs](examples/model_a_delivery.rs) take a one-argument closure and no
        longer build. The library and its own unit tests are green; the integration and example
        targets are `M28.4.1`'s to migrate. Contract wiring is untouched deliberately -- what
        becomes of the oracle's leak rules is a decision `M28.4.1` carries, per
        [D-73](DESIGN-NOTES.md#d-73).
  - [x] **M28.4.1a** -- Restore the tree: every delivery callback takes the payload, so the
        build is green again on every feature set.

        **Two call sites were invisible to an ordinary sweep**, and both are worth remembering
        rather than rediscovering. One lives in a **doctest**, found only because this repository
        compiles its prose. The other is in [failure_paths.rs](tests/failure_paths.rs), which
        compiles only under `--all-features`, so a default-feature check could not see it. A
        migration sweep here has to run `--all-features` **and** `--doc` before it means anything.

  - [x] **M28.4.1b** -- All ten push shapes have an inventory form: `read_raw_owned`,
        `write_raw_owned`, `read_owned`, `write_owned`, `flush_owned`, `cancel_owned`, and the
        four registered variants. `Held` is populated by the guarded pushes and
        `Held.registration` by the registered ones, so both halves are exercised and neither
        needs a dead-code marker any longer. `read_owned` is the only
        entry point that stows, so "migrate the consumers" has no destination for the other ten
        shapes yet -- writes, flushes, cancels, and the registered variants. Give each an
        inventory form first, populating `Held` for the guarded ones, which is what retires the
        `#[expect(dead_code)]` on `Held` and `FileGuard`.

  - [x] **M28.4.1c** -- Decided in [D-74](DESIGN-NOTES.md#d-74): the leak rules **retire** rather
        than narrow, because a push that hands back only an `OperationId` leaves nothing to drop
        unclaimed. `State::Pushed` and `State::PushedTokenless` collapse with them. The
        conservation they approximated becomes `held() == outstanding()`, which the ring can check
        about itself. `RingContract` keeps the four claims that are about the kernel rather than
        about a caller's bookkeeping.

  - [ ] **M28.4.1d** -- Migrate every consumer onto the inventory, then retire the token API.

        **Measured before planning, and it is larger than "36 files" suggested**: 172 push call
        sites and **116 `claim_if` sites** across 35 files. `claim_if` is not a substitution --
        it is how each test *drives* its ring, so converting restructures control flow rather
        than replacing a call.

        **What a conversion actually does, which is why it is worth it.** The caller's
        `HashMap<usize, (sidecar, Token<..>)>` *disappears* at each site: the push carries the
        sidecar as `X`, and the pop returns `(payload, sidecar)` together. That is `D-55` paying
        off rather than a cost being paid.

        **Batched, and the reason that is legitimate.** Both APIs coexist today, so a
        partly-converted tree still compiles and every batch is a green commit. `M28.4.2`'s
        "convert all of them or none" governs the **shipped** state -- never two token models in
        a release -- not the path to it. The final batch is what makes that true, and nothing is
        released in between.

  - [x] **M28.4.1d.1** -- [bounded_pop.rs](tests/bounded_pop.rs) converted as the worked
        pattern. The `Token<Vec<u8>>` threaded through `push_pending_read`, `settle` and six call
        sites is gone; `PipeRing = IoRing<Vec<u8>>` holds the buffer instead. The conversion
        found a real defect in rundown, which is recorded as `M28.4.1d.1b`.

  - [x] **M28.4.1d.1b** -- Decided: there should be one pop, not two. **Decide what `try_pop`
        and `pop_within` mean on a ring that holds payloads.** Found by converting the first file: `drain_for_rundown` popped with `try_pop`
        and never reclaimed, so rundown stranded every entry it reaped. Fixed there -- rundown is
        teardown, so dropping is right, and the completion is the proof that makes freeing safe.

        **The same gap is open on the public paths, and the answer is that the split should not
        exist.** An earlier note here offered three ways to manage a permanent distinction
        between `try_pop` and `try_pop_held`. That was wrong, and the symmetry is the argument:
        `try_pop_held` sits beside `try_pop` for exactly the reason `read_raw_owned` sits beside
        `read_raw` -- the inventory was added *beside* the token API rather than replacing it.
        Both are the same transitional duplication, and the push side is already settled as
        ending with one family.

        **Nothing needs a non-reclaiming pop.** Measured: the only internal callers are
        `drain_for_rundown` (now reclaims), `pop_within_with`, and one other -- and once the
        token pushes are gone, *every* operation has an inventory entry, including the tokenless
        ones, which stow `payload: None`. There is no operation a pop could legitimately find
        nothing for.

        So `try_pop` becomes the reclaiming pop, `try_pop_held` disappears as a name, and
        `pop_within` returns the same shape. Preserving the distinction would be a rule against
        an impossible act, which [D-74](DESIGN-NOTES.md#d-74) already names as worse than no rule
        at all.

        **This does not gate the conversion.** Consumers migrate onto the reclaiming pop either
        way; the rename lands in `M28.4.1d.3` beside the push retirement.

  - [x] **M28.4.1d.2** -- Every consumer that can be converted before the token API is retired now is; three plus `append.rs` are blocked on `M28.4.1d.3`. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d2)

  - [x] **M28.4.1d.2b** -- Documented the two ways round a mixed payload on `IoRing::with_inventory` and in `D-73`; no `IoBuf` enum helper built. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d2b)

  - [x] **M28.4.1d.3** -- Token API retired, `D-74` applied, one reclaiming pop per shape; the break is closed. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d3)

  - [x] **M28.4.2** -- Five inventory sabotages added (push, both pops, both appender halves); the sweep also found `d.3` had broken the manifest. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2842)

- [x] **M28.5** -- `observe_tokenless_push` retired; the outer `None` has two causes the caller distinguishes, recorded as `D-75` and asserted both ways. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m285)

- [x] **M28.6** -- Swept what the break made false: `D-4` and `D-55` amended, six live example claims corrected, and three defects found in `d.3`'s own prose sweep. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m286)

## M-inf -- Diagnostic work with no gating deliverable

- [ ] **M26.14.4** -- **Find the threshold the close races.** `M26.14.2` used 1ms because it is
  about 80x the natural 12us gap; nothing establishes what the minimum is. A sweep -- 0, 10us,
  50us, 100us, 500us, 1ms, interleaved at 20000 each -- would say whether the window is
  microseconds or milliseconds, which is itself evidence about what the close races.

  **Moved here 2026-10-01, when its gate resolved the "never ships" way.** This item was gated on
  `M26.14.3` and said so itself: if the draining teardown were adopted, the sleep never ships and
  the sweep is only of diagnostic interest. It was adopted, so that is where this now sits -- not
  cancelled, because the question it asks is about the *kernel* window the close races, which the
  drain avoids rather than explains, and that remains the one thing about this stall nobody has
  been able to see directly.
