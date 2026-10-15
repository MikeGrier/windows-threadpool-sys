# Plans: windows-ioring-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Design decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

Add a row below, and a milestone in [CHECKLIST.md](CHECKLIST.md), when new work is planned.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | `M-R1` stall post-mortem after the hooking facility's removal; `M29` corrects the `epoch_log` sample's claim that a later successful commit repairs a failed one; `M31` specifies `EventDelivery`'s callback re-entrancy and concurrency; `M32` states and pins that write-through needs an unbuffered handle; `M33` adds a wake-only delivery for a caller that owns its ring; `M34` asks a registration's span check without pushing; `M35` moves the kernel responder from one thread to one ring; `M36` moves `CompletionWake`'s ring-opening tests to `tests/`; `M37` gives a consumer that forbids `unsafe` a safe responder that fails a ring's submits; `M38` widens it to any seam call, a completion's result, and a stalled submit | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
