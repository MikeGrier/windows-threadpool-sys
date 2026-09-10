# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

No open milestones. Completed work is in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M3 -- Carried over from `main` when the deferred-namespace-ops branch was merged

The rest of M2 completed on this branch and is archived. This item did not: it was raised on `main`
during PR #86 and describes a defect this branch never touched, so the merge carries it forward
rather than letting the archive swallow it.
- [ ] **M3.1** -- Stop `request_cost` calling a cross-host ratio "the finding". (Raised on `main` as `M2.9`; renumbered here because M2 is archived on this branch.)

  [src/request_cost.rs](src/request_cost.rs) ends its module doc with "Absolute values are
  host-specific; the **ratios against the doorbell and the atomic** are the finding." The ratios that
  [src/bin/request_cost.rs](src/bin/request_cost.rs) actually prints divide THIS host's measurement by
  `DOORBELL_NS_REFERENCE` / `ATOMIC_NS_REFERENCE`, which are constants measured on the Snapdragon X2
  development machine. A ratio with this host's numerator and another host's denominator is neither a
  same-host ratio nor a portable finding, and the emitted report says as much two lines later:
  "re-read that probe on this host before trusting them". So the module doc promotes to "the finding"
  exactly the number its own output tells the reader not to trust.

  **Pre-existing, and deliberately not fixed in the `GetFullPathNameW` peel (PR #86) that found it.**
  It arrived in `ae1e39f`, is already on `main`, and is outside that branch's diff; folding it in
  would have put an unrelated behavioural change into a documentation peel that had already run to
  nineteen review rounds.

  The fix is a decision, not a sweep, which is why this is queued rather than taken: either compute
  both figures on the same host and run (the probe would have to measure the doorbell itself, or read
  a companion artifact), or keep the fixed references and demote them in the prose from "the finding"
  to a labelled cross-host comparison. The first is more useful and more work; the second is honest
  and cheap. Same defect class as PR #86's subject -- a claim stated more strongly than the evidence
  supports -- so whichever is chosen, the wording has to end up matching what the numbers can carry.
