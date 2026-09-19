# EP-X1.2: controlled parameter sweeps

**Implementation and capture complete; result discussion remains open under parent [CHECKLIST.md](../../../../CHECKLIST.md) -> `EP-X1.2` / `EP-R1.7`.**

Captured on 2026-09-19 UTC on the development host. This is buffered-file evidence
after reference reading, with synthetic checksum work and closed-loop saturation.
No device-path, offered-arrival or exclusive-host measurement is claimed.

The protocol is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RC-D9`.
The [matrix](matrix.json) contains every configuration, changed field, ordered case
name and fixture size. Each case name identifies its raw JSON report in this directory;
all raw reports are retained, including reversed-order retakes and controls. The
[summary](summary.json) contains per-case/per-orientation throughput, stage latency,
CPU, pressure and occupancy summaries plus pooled point/control throughput envelopes.
The report configurations, not abbreviated labels below, specify the measured values.

| Sweep | Opening control | First point | Second point | Closing control |
|---|---|---|---|---|
| Block size, fixed payload bytes | [control](block-00-control.json) | [smaller blocks](block-01-first.json) | [larger blocks](block-02-second.json) | [control](block-05-control.json) |
| Checksum passes | [control](compute-00-control.json) | [intermediate work](compute-01-first.json) | [heavy work](compute-02-second.json) | [control](compute-05-control.json) |
| Read depth, fixed buffer count | [control](depth-00-control.json) | [shallower](depth-01-first.json) | [deeper](depth-02-second.json) | [control](depth-05-control.json) |
| Queue capacity | [control](queue-00-control.json) | [narrower](queue-01-first.json) | [wider](queue-02-second.json) | [control](queue-05-control.json) |
| Scheduling batch size | [control](batch-00-control.json) | [intermediate](batch-01-first.json) | [larger](batch-02-second.json) | [control](batch-05-control.json) |
| Queue/batch interaction | [control](queue-batch-00-control.json) | [queue only](queue-batch-01-queue.json), [batch only](queue-batch-02-batch.json) | [both](queue-batch-03-both.json) | [control](queue-batch-07-control.json) |

## Observations

The added-checksum-work points have lower throughput ranges than their one-pass
controls in every arrangement and orientation. In the block-size sweep, some
point/control ranges are disjoint in one orientation while overlapping in the
other. Block count, outstanding byte ceiling and per-block overhead change with
block size even though the allocated payload-byte ceiling stays fixed.

Every depth, queue, batch and selected interaction throughput envelope overlaps
its own bracketing control envelope. This describes the observed ranges; it is
neither proof of equivalence nor a statistical significance test. Outliers and
control drift are retained. Direct and independent paths do not use the handoff
queue, so their queue-sweep variation is same-code variation, not a queue effect.

Throughput overlap does not imply unchanged latency or occupied capacity. In the
original orientation, the deeper-read pipeline captures have higher per-reader
lease peaks and longer median post-admission p99 latency than their controls.
The wider-queue pipeline captures in that orientation have longer median handoff
p99 latency and higher lease peaks. The heavy-checksum pipeline captures show
longer queue latency and more failed full-queue attempts. Those counts are retry
observations, not unique stalls or time spent waiting.

CPU observations retain the OS's coarse accounting granularity, including short
trials whose CPU/wall ratio exceeds the participating processor count. The
capture does not convert those ratios into additional worker capacity. Batching
observations name the actual nonempty batches, including partial batches; they
do not imply bulk OS or atomic queue operations.

## Proposed disposition and next cases

Retain all three isolated schedulers and the independent read/buffer limits and
batch control. No path is merged or deleted, and no queue/batch value becomes a
planner default. Review their eventual production disposition at `EP-X3.3` as
already queued in parent [CHECKLIST.md](../../../../CHECKLIST.md).

For `EP-X1.3`, propose the baseline, narrow-queue, deep-read and heavy-checksum
configurations above, each held fixed while changing idle policy. They expose
different observed queue, buffer and compute pressure rather than a selected
throughput winner. Keep both orientations and bracketing controls. Discuss this
selection under `EP-R1.7` before closing `EP-X1.2` or starting the next item.

## Provenance and verification

All raw captures share one optimized executable, build identity and processor pair.
The checkout was dirty while the item was implemented; its embedded harness and
manifest fingerprint identifies the measured Rust source. Each trial passed
per-block reference checks and rundown; saved configurations, bindings, budgets,
batch observations and throughput summary arithmetic were also validated.

The first derived effects table incorrectly merged sweeps because PowerShell's
property-based grouping ignored ordered-dictionary keys. Raw reports and their
per-case groups were unaffected. The committed summary was regenerated from
those same reports with explicit key grouping, not from a new timing run.
It retains the original capture-script digest separately from the analysis-script
digest. A replay regression checks distinct sweeps and both overlapping/disjoint
ranges; [sabotage.json](../../sabotage.json) reintroduces the grouping defect.

Package tests include deterministic batches, invalid inputs, cancellation, live
short tails and independent budgets, the actual matrix's isolation rules, and
summary replay. The sabotage manifest also covers matrix budget normalization,
pending-read enforcement and batch-quantum consumption, with an equivalent control.
Reproduction and create-new summary replay are documented in
[README.md](../../README.md); original scratch fixtures are not required for replay.