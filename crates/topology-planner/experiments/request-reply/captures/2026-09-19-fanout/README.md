# EP-X2.5 fan-out and join demonstration

Captured on 2026-09-19 UTC with one release executable. The
[raw report](fanout.json) holds source/build identity, ordered configurations, traces,
per-unit membership with its computing worker and timestamps, outcomes, credit events,
join logs and partial-state counters. The checkout was dirty during implementation; the
report's source/manifests fingerprint identifies the measured code. No observation has
been filtered into a selected timing result.

The protocol is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RR-D9`; commands and field
definitions are in [README.md](../../README.md#fan-out-and-join-comparison). This is
unpinned in-memory fan-out, not a physical locality or startup measurement.

Twelve scenarios each ran six positions in serial/owned/scattered/scattered/owned/serial
order: empty, single, scatter-only, broadcast-only, mixed-shapes, wide-fan-out,
narrow-fan-out, skewed-width, slow-unit, aborting, pressure and cancellation.

## Observations

**Every drained scenario produced a byte-identical join log across all six positions and
all three arrangements**, including wide and skewed fan-out, a deliberately slow unit,
and credit pressure. Every run passed the independent serial replay, which recomputes
each unit result and each joined value and checks that every join consumed its declared
unit set exactly once. Membership was exact in every trial; no run reported a duplicated,
missing or foreign unit.

**Partial state across a thread appeared only under `ScatteredJoin`.** The serial owner
and the record owners reported zero partial records and zero partial units throughout,
which is what assembling inline looks like rather than a measurement of a buffer. The
scattered joiner's partial-unit peak tracked fan-out width across the scenarios -- lowest
on the narrow two-unit trace and highest on the skewed, slow-unit and pressure traces --
while its partial-record peak stayed below the credit ceiling in every run. Residual
partial state was zero in all 72 trials, so nothing was left held at the end of any run.
How close the partial-record peak comes to the ceiling varies between runs of the same
code; the ceiling is what bounds it, and the bound is what the verifier checks.

**Reclamation on abort was identical across arrangements.** The aborting scenario
discarded the same number of gathered units in all six positions, which is the behaviour
the contract asks for: a record that does not join releases a complete unit set, and the
release is counted rather than dropped. Scatter and broadcast records both appear in that
scenario and neither shape changed the census.

Cancellation is the one scenario whose logs differ between positions, and that is the
contract rather than a defect: admission stopped at the declared boundary, every admitted
record reached exactly one terminal outcome, outcomes formed a joined/aborted prefix and
an all-cancelled suffix, and each log matched replay of its own joined records. The
discarded-unit count varied with how far each run had progressed when cancellation
landed, and residual state remained zero. No arrangement is selected from any of these
figures, and nothing here establishes which a planner should choose.

## Behavioral Verification

Unit tests compare all three arrangements over eleven trace shapes at one, two and four
workers and assert the join log equals the declared-order reference in each. Further
tests pin exact membership per record, scatter and broadcast staying distinct at every
unit count from one to sixteen, owned routing sending every unit of a record to its owner,
partial state existing only where a join crosses a thread, injected unit failure discarding
a complete unit set, cancellation reclaiming branches, cancellation overriding an injected
failure it reaches first, external cancellation admitting nothing, credit ceilings under
pressure, and rejection of zero-unit records, out-of-range slow and failing unit indices,
oversized unit counts and invalid worker or credit configurations. Service and clock
failures are injected to confirm every worker joins and the error propagates. Report-level
tests mutate real reports to confirm the verifier rejects missing, duplicated and foreign
units, drifted join values, reordered logs, a scatter record read as a broadcast, owned
joins assembled elsewhere, unit timestamps outside their record's span, aborts naming the
wrong unit, discard and residual census drift, and credit or worker census drift.

[sabotage.json](../../sabotage.json) verifies detection of a join that loses a unit, a
drifting unit result, the two shapes collapsed into one formula, owned routing collapsing
to worker zero, an abort that keeps its gathered units, cancellation no longer ending
joining, and a runner that skips verification. An equivalent service-chunk control
survives. All twenty-eight sabotages across the four paths behave as declared.

Two defects were found during implementation rather than by review. `OwnedJoin` first used
one shared record queue, so any worker could take any record and its routing promise was
incidental; it now has one queue per owner. The scattered joiner sorted its unit reports
without sorting the parallel kinds vector, so a unit's outcome could be attributed to the
wrong index -- the abort tests caught it. The worker census originally carried joined,
aborted and cancelled tallies that no worker can know, since the coordinator resolves
outcomes; those fields were removed rather than backfilled.

All three arrangements are retained. The disposition is
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RR-D10`. Parent
[CHECKLIST.md](../../../../CHECKLIST.md) -> `MR2` owns contract synthesis; paused work is
not resumed by this record.
