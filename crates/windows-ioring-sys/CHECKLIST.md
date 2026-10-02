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
[here](COMPLETED-CHECKLIST.md#moved-2026-10-01-m26-and-m28).

**`M26.13`, the pool stall, is not archived -- it was transferred.** The fault was never this
crate's, so the question, its timeline and its 33 measurement captures now live in
[windows-threadpool-sys](../windows-threadpool-sys/CHECKLIST.md) as `M-T7.1`. The reproducer stays
here, because the ring is what reaches the state.

**An `M{n}+` heading is parked rather than pending** -- see the `M{n}+` convention: it is gated work
with no current obligation, not an unfinished milestone. Which milestones are open is what the
headings below say; this preamble deliberately does not restate it, because a second copy is one
nobody updates.

## M27 -- Placement questions a consumer can answer on their own hardware

**Re-planned 2026-10-01, and most of this milestone retired.** M27 previously asked what this crate
owes a topology realizer, and carried a census (`M27.1`) and a gap-closing item (`M27.2`) against
the plan vocabulary. Both are retired: see
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-10-01-m27-retired).

**The reason is structural, not a judgement that the work is unimportant.** The realizer is a
distinct component -- [EP-D-5](../topology-planner/DESIGN-NOTES.md#ep-d-5) places it outside this
crate, depending on `topology-model` and the runtime crates -- and it does not exist. This crate
fundamentally delivers a safe API over `IoRing`. If a realizer turns out to need API it does not
have, that is the ordinary lifecycle of a dependency asking its layer for something, worked out
when it happens by the component that discovered the need. It is not a standing obligation this
crate carries on behalf of a consumer nobody has written.

[D-8](DESIGN-NOTES.md#d-8) is untouched by all of this: policy stays out of this crate.

- [x] **M27.1** -- Retired with `M27.2`; the census it produced found no gap in this crate, and its
  one finding is queued where it belongs. ->
  [retired 2026-10-01](COMPLETED-CHECKLIST.md#moved-2026-10-01-m27-retired)

- [x] **M27.2** -- Retired: a gap-closing item for gaps a non-existent consumer has not asked for.
  -> [retired 2026-10-01](COMPLETED-CHECKLIST.md#moved-2026-10-01-m27-retired)

- [ ] **M27.3** -- Give a consumer the means to answer placement questions on their own hardware.
  **Not gated on the planner** -- it is the client-side half of the thesis, and it is what lets a
  developer disagree with any plan they are handed. `cache_domains.rs` now prints every cache level
  beside the heuristic's pick; the equivalent for placement is a sample that reports what a chosen
  arrangement costs and what the alternatives would have cost, on the machine in hand.
  [ring_copy](examples/ring_copy) is the natural host, being already policy-selectable. **Do not ship
  a verdict** -- report the observation and let the consumer conclude, per OPTION INTEGRITY.
