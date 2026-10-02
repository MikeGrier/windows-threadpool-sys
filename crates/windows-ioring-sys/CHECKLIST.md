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

**No milestone is open.** The plan these belonged to is recorded in
[COMPLETED-PLANS.md](COMPLETED-PLANS.md); reopen this file by adding a milestone here and a row in
[PLANS.md](PLANS.md) when new work is planned.

**`M26.13`, the pool stall, is not archived -- it was transferred.** The fault was never this
crate's, so the question, its timeline and its 33 measurement captures now live in
[windows-threadpool-sys](../windows-threadpool-sys/CHECKLIST.md) as `M-T7.1`. The reproducer stays
here, because the ring is what reaches the state.

**An `M{n}+` heading is parked rather than pending** -- the convention is noted here because the
archived groups use it: such a heading is gated work with no current obligation, not an unfinished
milestone. When this file carries milestones again, which of them are open is what the headings
say; this preamble deliberately does not restate it, because a second copy is one nobody updates.
