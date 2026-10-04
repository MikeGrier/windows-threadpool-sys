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

## M-R2 -- The outstanding-operation ledger, after the PR #113 review

Opened 2026-10-04. The review found that a raw push could still reach a false quiesce; the fix is
[D-78](DESIGN-NOTES.md#d-78).

- [x] **M-R2.1** -- **Track outstanding operations by identity rather than by count.** Done
      2026-10-04. `Accounting` keeps the set of minted, unretired `user_data`; `record_completion`
      and `cancel_reservation` take the identity; quiescence is that set being empty, for owned
      and raw pushes alike. Pinned by `a_completion_that_retires_nothing_is_not_quiescence`
      (both push kinds, both directions) and by the `PR #113` case in
      [sabotage.json](sabotage.json), which restores count semantics and is caught.

- [ ] **M-R2.2** -- **Decide whether the ring should now answer `UnexpectedCompletion`.** Not
      started, and a decision for the engineer rather than something to take in passing.

  [D-76](DESIGN-NOTES.md#d-76) kept conservation checking external partly because the ring
  *could not* tell a `_raw` push's completion from one it never minted, and named "`_raw` pushes
  gain entries" as what reopens it. `M-R2.1` gave every push a ledger identity, so the ring now
  knows: `record_completion` returns whether the identity was minted, and the pop discards that
  answer. D-76's other limbs -- `DuplicateCompletion` needs a finished-identity history, and
  `BufferStillInUse` reads a count the caller owns -- are unchanged. Options include surfacing
  the bit on `Completion`, reporting it to a `RingContract`, or leaving it internal; leaving it
  coarse until a consumer's need is visible is also an answer.
