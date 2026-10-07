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
[here](COMPLETED-CHECKLIST.md#moved-2026-10-01-m20-through-m24); `M26+` and `M28`
[here](COMPLETED-CHECKLIST.md#moved-2026-10-01-m26-and-m28); and `M27`
[here](COMPLETED-CHECKLIST.md#moved-2026-10-01-m27).

The plan these belonged to is recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md); add a milestone
here and a row in [PLANS.md](PLANS.md) when new work is planned.

**`M26.13`, the pool stall, is not archived -- it was transferred.** The fault was never this
crate's, so the question, its timeline and its 33 measurement captures now live in
[windows-threadpool-sys](../windows-threadpool-sys/CHECKLIST.md) as `M-T7.1`. The reproducer stays
here, because the ring is what reaches the state.

**An `M{n}+` heading is parked rather than pending** -- the convention is noted here because the
archived groups use it: such a heading is gated work with no current obligation, not an unfinished
milestone. When this file carries milestones again, which of them are open is what the headings
say; this preamble deliberately does not restate it, because a second copy is one nobody updates.

## M-R1 -- The stall post-mortem after the hooking facility was removed

Opened 2026-10-03. `windows-threadpool-sys` deleted its inline hooking facility, because this
workspace ships no undocumented APIs and no assumptions about undocumented behaviour -- see
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#no-undocumented-apis). The reproducer in this crate was its
real consumer, so what that costs the post-mortem is recorded here rather than left to be inferred
from an absence.

- [x] **M-R1.1** -- **Strip the post-mortem probes that depended on the hooking facility.** Done
      2026-10-03.

  **What was removed from `tests/event_delivery.rs`:**

  - the worker-factory counter and completion-port depth reads taken at the moment of failure, in
    `hand_rolled_trigger`, deliberately ordered *before* the liveness probe because submitting
    work is the one action measured to release this stall and asking afterwards would describe a
    pool the question had already repaired;
  - `poke_the_stalled_port`, gated on `IORING_POKE_PORT`, which posted a packet to a stalled port
    and read the factory's worker count either side. This is the experiment behind the recorded
    answer that **an ordinary completion-port arrival has never been seen to recover this stall**;
  - `capture_healthy_factory_state`, gated on `IORING_CAPTURE_HEALTHY`, the healthy-arm contrast
    for the same reading.

  **What the apparatus can no longer answer.** Whether a stalled factory has zero workers while
  holding queued work -- the signature the investigation turned on -- and whether poking the port
  changes that. The reproducer still detects the stall and still reports outstanding counts, ring
  state and pool liveness; what it has lost is the ability to say *why* the pool did not dispatch.

  **What survives, and where.** The captures taken with the facility are committed under
  `windows-threadpool-sys/measurements/`, and the central finding -- the pool's first worker is
  never created -- was reproduced independently of the hooks by an ETW kernel trace across 900
  runs (`M26.13.19`). ETW is the documented route and is where any rebuild should start.

- [ ] **M-R1.2** -- **Decide whether to rebuild the two experiments on ETW.** Not started, and
      deliberately not scoped yet.

  The two questions `M-R1.1` retired are worth answering and are not answerable in-process on
  documented ground. An ETW kernel trace reaches both: thread-create events say whether the
  stalled factory ever made a worker, and the same session spans the poke, so the before/after
  contrast survives the move. Whether it is worth building depends on whether `M-T7.1` is resumed,
  which is a decision for the engineer rather than something this item should assume.

## M29 -- The `epoch_log` sample treats a failed commit as repairable by a later one

Opened 2026-10-05 from the durable-ioring design session
([DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md),
Scenarios 1 and 2). Queued here rather than in [durable-ioring](../durable-ioring/CHECKLIST.md)
because the sample is wrong whatever becomes of that crate
([DI-D-7](../durable-ioring/DESIGN-NOTES.md#di-d-7)).

- [ ] **M29.1** -- **Stop letting a later successful commit report an earlier failed epoch durable.**
  `Committer::claim` in [commit.rs](examples/epoch_log/commit.rs) argues under "A failed commit is
  not permanent, and that is not a loophole" that commit *N+1*'s covering flush settles a failed
  commit *N*, and advances `durable_through` past it. That holds only if the writes in the suspect
  window -- every write pushed before the failure was observed that no earlier successful covering
  flush covers -- are re-issued before
  the later flush. Without that, a lower layer that discarded the data on the failed flush (the
  documented Linux behaviour, and undocumented either way on Windows) leaves *N* reported durable
  and gone. Correct the code and its rustdoc so the watermark stays at the last epoch below the
  failure, and update `commit/tests.rs`, which presumably pins the current behaviour. **Sweep the
  same reasoning** in [checkpoint.rs](examples/epoch_log/checkpoint.rs) (its `settle` advances by
  `max`; a later checkpoint rewrites the whole record, which is the re-issue case, so it may be
  correct -- argue it either way in the code) and in
  [strategy.rs](examples/epoch_log/strategy.rs)'s `settle`, and check
  [contract.rs](examples/epoch_log/contract.rs) for a statement that relies on it.

## M31 -- `EventDelivery`'s callback contract is specified, not incidental

Opened 2026-10-05 from the durable-ioring design session
([DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md),
"DI-2.3: completion routing"). durable-ioring's design ([DI-D-18](../durable-ioring/DESIGN-NOTES.md#di-d-18))
needs two properties of the
`on_completion` callback. Today they hold only as implementation: the comments on the private
`drain` and on the callback body say so, but `EventDelivery::new`'s rustdoc does not. Relying on
them as they stand would bind a consumer to incidental behaviour. The engineer's framing: using
it "without permission" is not better than getting it properly supported.

This does **not** make the ring a synchronization provider. That is out of this crate's scope, and
no consumer may share the ring's lock. It states two properties of this crate's own callback that
any callback API owes its users.

- [ ] **M31.1** -- **State and pin `on_completion`'s re-entrancy and concurrency on the public
  surface.**
  - **Re-entrancy:** `on_completion` is called with the ring's lock released, so it may call
    `EventDelivery::scope` and submit without deadlocking.
  - **Concurrency:** two invocations may run at once on different pool threads, and completions
    handed to concurrent invocations carry no order relative to each other. A consumer that needs
    an order must impose it. The drain-rearm-drain shape is what produces the overlap.
  - Both go in `EventDelivery::new`'s rustdoc, and in [DESIGN-NOTES.md](DESIGN-NOTES.md) as a
    decision, so a later change to the delivery loop has a stated contract to answer to.
  - **Tests, in both directions:** a callback that calls `scope()` and pushes completes, which a
    lock held across the callback would turn into a deadlock -- run it with a bounded wait so a
    regression fails rather than hangs. The concurrency statement is a permission, not a
    promise, so it is asserted by documentation; do not write a test that requires overlap to be
    observed.
  - **Sabotage:** hold the ring lock across `on_completion` in `drain`, and confirm the
    re-entrancy test fails.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M3` -> `DI-3.2` (dioring over `windows-ioring-sys`), whose Model A front end relies on this item's contract. See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

- [x] **M31.2** -- **Withdrawn 2026-10-06: "every `Completion` is real" is already contract.**
  `try_pop`'s rustdoc hands a payload back by that call alone, and [D-79](DESIGN-NOTES.md#d-79)
  makes a completion for anything not in flight a panicking defect; the restatement added nothing
  a consumer could rely on, and its `compile_fail` doctest would have pinned an implementation
  detail.

- [x] **M31.3** -- `SharedFile` is removed; a safe push's file is `win-shared-os-owned-handle`'s `SharedHandle`. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#m313)

## M31+ -- ETW for the ring's own lock (withdrawn)

Opened and withdrawn 2026-10-06 in the durable-ioring design session.

- [x] **M31+.1** -- **Withdrawn 2026-10-06: the ring crate publishes no ETW provider of its own.**
  It would have timed contended acquisitions of `EventDelivery`'s mutex. The engineer: "the etw
  events we publish give a surface for diagnosis of issues; clients don't want to search around
  across providers to try to correlate issues." durable-ioring publishes the one surface, and sees
  this lock's contention as a slow call into the ring, timed at its boundary.
