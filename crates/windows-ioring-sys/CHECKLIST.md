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

- [x] **M27.1** -- Census done: one gap, and it is not in this crate. Caller-buffer placement is
  already covered by `NumaBuffer`; the ring's own queue memory is a Win32 limit; what is missing is
  thread placement, which belongs a layer down. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m271)

  Full census: [REALIZATION-CENSUS.md](REALIZATION-CENSUS.md).

  > **-> CROSS-COMPONENT HANDOFF:** the gap is queued as component `crates/windows-threadpool-sys`
  > -> `M-T8` -> `M-T8.1` (`Decide whether this crate expresses thread placement, and if so where`).
  > See [CHECKLIST.md](../windows-threadpool-sys/CHECKLIST.md).

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
