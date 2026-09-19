# EP-X1.1: repeatability and processor-role controls

Captured on 2026-09-19 UTC on the development host. This is buffered-file,
closed-loop evidence after reference reading, not an isolated device measurement.
The protocol is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RC-D7`;
[capture-ep-x1-1.ps1](../../capture-ep-x1-1.ps1) reproduces the sequence.

| Artifact | Role |
|---|---|
| [control-before.json](control-before.json) | Initial one-pass control, both processor orientations. |
| [compute-heavy.json](compute-heavy.json) | Same fixture and resource ceilings with repeated checksum work. |
| [control-after.json](control-after.json) | Same-code repeat of the initial control after the heavy case. |
| [summary.json](summary.json) | Derived ranges, nearest-rank medians and within-repetition paired differences; executable and capture-script SHA-256 identities. |

Each raw report retains its complete configuration, source/build identity, topology,
individual trials, budget labels and requested/observed bindings. Every trial passed
per-block reference verification and buffer/I/O rundown. All reports used the same
release executable and selected processor pair; the build was labelled dirty while
this item was implemented. The embedded harness/manifests fingerprint identifies
the measured source. No measured source changed during the sequence.
The script digest covers its CRLF form at execution; the committed script was
subsequently normalized to LF. Restoring CRLF reproduces that digest exactly.

## Observations

Independent-worker throughput exceeded pipeline throughput in every paired comparison,
in both orientations and both checksum workloads. In each orientation of each
capture, their throughput ranges were disjoint. Independent workers performed
checksum work on both CPUs; the pipeline had one checksum worker while its reader
polled. I/O ownership and payload handoff also differ between those paths.

In the one-pass controls, pipeline throughput exceeded the direct reference in every
paired comparison. In the compute-heavy case, pipeline and direct throughput ranges
overlapped in both orientations, and the pipeline-minus-direct paired difference
had both signs. The direct reference used one CPU; the pipeline used two. These
observations do not isolate the contribution of stage overlap.

The repeated one-pass control shifted within overlapping same-treatment ranges.
Both orientations include variation, and the initial control and compute-heavy
capture include low-throughput samples retained in the raw reports. No samples were
discarded. There was no exclusive-host or background-load control.

CPU-per-wall observations in short trials sometimes exceed the participating CPU
count. These are ratios of separately sampled wall time and coarse OS thread CPU
accounting, not evidence that the worker ceiling was exceeded. Raw CPU times and
latencies remain in the reports. The longer compute-heavy trials distinguish the
pipeline's polling reader from useful checksum work on both independent workers.

## Disposition and handoff

The path-retention decision and evidence requirements are in
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RC-D8`, not a ranking for other workloads.
Return to parent [CHECKLIST.md](../../../../CHECKLIST.md) -> `EP-R1.7`.
The next experiment remains `EP-X1.2`, with same-code and role-swapped controls
carried into its one-factor sweeps. No production planner contract is frozen here.

Verification included package tests and doctests, the workspace Clippy gate, and
the [sabotage manifest](../../sabotage.json). Defects in actual role binding and
schedule consumption were caught; the equivalent chunking control survived.