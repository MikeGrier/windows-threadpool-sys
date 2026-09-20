# EP-X2.4 ordered ingestion demonstration

Captured on 2026-09-19 UTC with one release executable. The
[raw report](ingest.json) holds source/build identity, ordered configurations,
traces, per-record stage timestamps, outcomes, credit events, publication logs and
per-worker observations. The checkout was dirty during implementation; the
report's source/manifests fingerprint identifies the measured code. No observation
has been filtered into a selected timing result.

The protocol is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RR-D7`; commands and
field definitions are in [README.md](../../README.md#ordered-ingestion-comparison).
This is unpinned in-memory ingestion, not a physical locality or startup measurement.

Twelve scenarios each ran four positions in serial/staged/staged/serial order:
empty, single, steady, burst, cheap, transform-heavy, publish-heavy, slow-first,
narrow-window, aborting, pressure and cancellation.

## Observations

**Every drained scenario produced a byte-identical publication log across all four
positions, in both arrangements.** That held with transforms overlapping across
three workers, with a one-slot resequencing window, under credit pressure, and with
a first record given eight times the transform budget. The `aborting` scenario
published 52 of its 64 records and aborted 12, each at the stage its trace injected,
and its log was identical across positions too. Every drained run passed the
independent serial replay, which recomputes each published value and checks the log
against the declared order rather than against the other candidate.

The serial owner reported no resequencing buffer in any run: occupancy and blocking
observations were zero throughout, which is what having no buffer looks like rather
than a measurement of one. The staged runs reached their configured window in every
non-empty scenario -- three slots where three were configured, one where one was --
and recorded transform workers blocking on it in every case, including the
`narrow-window` and `pressure` points where the window was one slot.

Head-of-line delay separates the two arrangements and is reported per record as
`published_ns - ready_ns`. Under the serial owner it is the gap between two adjacent
timestamps on one thread. Under the staged pipeline it is the wait for predecessors
and is larger at both the median and the maximum in every non-empty scenario; the
per-scenario distributions are in the raw report. The publish-heavy point, where the
publisher holds the cursor longest, and the empty point, where there is nothing to
wait for, are the two ends of that range. No candidate is selected from these
figures, and nothing here establishes which arrangement a planner should choose.

Cancellation is the one scenario whose logs differ between positions, and that is
the contract rather than a defect: admission stopped at the declared boundary with
16 admitted and 48 unadmitted in all four positions, while the published/cancelled
split varied with scheduling. Every admitted record still reached exactly one
terminal outcome, every run's outcomes were a published prefix followed by an
all-cancelled suffix, and each log matched replay of its own published records.

## Behavioral Verification

Unit tests compare both arrangements over eleven trace shapes at one, two and three
workers, including empty, single-record, cheap, transform-heavy, publish-heavy and
slow-first traces, and assert the publication log equals the declared-order
reference in each. Further tests pin publication advancing in position order while
transforms overlap, a one-slot window still draining every record, the serial owner
reporting no buffer, injected failure at each stage boundary, cancellation leaving a
cancelled suffix, cancellation overriding an injected failure it reaches first,
external cancellation admitting nothing, credit ceilings under pressure, and
rejection of invalid configurations, duplicate identities and out-of-order arrivals.
Service and clock failures are injected to confirm every worker joins and the error
propagates. Report-level tests mutate real reports to confirm the verifier rejects
reordered logs, wrong values, missing and extra entries, aborts naming the wrong
stage, cancellation without a stop, inverted stage timestamps, position and identity
mismatches, occupancy outside the window, and credit or worker census drift.

[sabotage.json](../../sabotage.json) verifies detection of publishing whatever is
ready rather than in declared order, a drifting transform result, cancellation no
longer ending publication, an ignored publish-stage failure, an unbounded reorder
window, and a runner that skips verification. An equivalent service-chunk control
survives. The cancellation sabotage initially survived: the publisher's cancelling
flag is only load-bearing where cancellation reaches a record that also carries an
injected failure, and no test combined the two. That case is now covered, and the
whole manifest -- all twenty sabotages across the three paths -- behaves as declared.

Both candidates are retained. The disposition is
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RR-D8`. Parent
[CHECKLIST.md](../../../../CHECKLIST.md) -> `MR2` owns contract synthesis; paused
work is not resumed by this record.
