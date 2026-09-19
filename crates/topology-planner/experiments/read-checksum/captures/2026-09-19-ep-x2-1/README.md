# EP-X2.1: available core/cache placements

**Local implementation and capture recorded; cross-NUMA timing and result review remain open in parent [CHECKLIST.md](../../../../CHECKLIST.md) -> `EP-X2.1`.**

Captured on 2026-09-19 UTC. This is explicitly authorized offline research under
[EP-D-10](../../../../DESIGN-NOTES.md#ep-d-10), not startup characterization.
The protocol is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RC-D10`.

The [plan](plan.json) retains discovery, selected pairs, unavailable relationships,
the exact case order and every configuration. Each case's name identifies its raw
JSON report in this directory. The [summary](summary.json) retains executable/script
digests and per-arrangement/orientation throughput, CPU, latency and residency status.
Raw reports retain individual trials, queue pressure, batches, allocation backing,
before/after page observations and build/source identity. No timing samples were removed.

| Pair | Observed relationship | Workloads |
|---|---|---|
| pair-00 | SMT siblings sharing the reported data/unified caches and memory domain | [buffered opening control](buffered_file-pair-00-00-heap-control.json), [buffered node preference](buffered_file-pair-00-01-node-0.json), [generated opening control](generated-pair-00-00-heap-control.json), [generated node preference](generated-pair-00-01-node-0.json) |
| pair-01 | Distinct cores with separate L1/L2 and shared L3 and memory domain | [buffered opening control](buffered_file-pair-01-00-heap-control.json), [buffered node preference](buffered_file-pair-01-01-node-0.json), [generated opening control](generated-pair-01-00-heap-control.json), [generated node preference](generated-pair-01-01-node-0.json) |

Reversed-order node-preference retakes and closing heap controls are also retained;
the plan is the complete index. Both worker-role directions were captured at every
point, including the single-worker reference and independent direct workers.

## Observations and limits

All trials passed per-block reference validation and I/O/buffer rundown. Requested
worker bindings matched observations at both ends of each measured phase. Explicit
node requests used NUMA-backed allocations, as checked from the actual payload owners.
All sampled payload pages, including the heap controls, were resident on node 0
before and after timing. No truncated node labels were reported in this capture.

This host exposes one Windows memory node. Consequently these observations compare
allocation backing and core/cache sharing within that node, not local versus remote
memory. Both selected pairs share L3; no L3-separated comparison was captured.
No device attachment, physical storage service or network steering was measured.

The generated companion fills payloads on the producer inside timing. It removes
file service but adds explicit producer computation and writes; it is not a
prefilled-buffer transport benchmark. The buffered companion follows a reference
read and does not establish physical-device bandwidth. These evidence classes are
not substituted for each other when comparing placement.

Individual samples and opening/closing controls vary. Pair groups were captured
sequentially, not interleaved across the whole host, and background load was not
controlled. Per-case arrangement positions and role directions are balanced, but
that does not eliminate temporal confounding between pair groups. No global
locality ranking or production queue/default choice is inferred from these rows.

## Remaining work and disposition

The paths remain experimental. Their current disposition and later review are
recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md#local-implementation-disposition).

Cross-NUMA evidence is **unrun**, not a passing synthetic substitute. A host exposing
distinct processor-associated memory nodes must run both worker directions with
payload preference on each endpoint's node, and residency observations must establish
what actually occurred. Unknown or mismatched pages cannot be reported as local.
The engineer approved local implementation/capture while that hardware remains absent.

Return to parent `EP-R1.7` for discussion of this evidence and remaining hardware work.
`EP-X2.1` stays unchecked; `EP-X2.2` and other paused MX work are not implicitly resumed.
The generated-buffer prerequisite is now implemented here; `EP-X1.5` still owns its
working-set and device-path comparisons.

## Verification and reproduction

The package suite exercises deterministic fixture equivalence, per-block integrity,
heap/NUMA-backed file and generated paths, short tails, unknown-node refusal, and
the actual discovery-driven campaign matrix. Synthetic cases cover core/cache/node
relationships, sparse node labels, missing/ambiguous membership, offline processors,
processor groups and usable pairs coexisting with unknown labels.

[sabotage.json](../../sabotage.json) verifies that wrong generated offsets, fabricated
node-zero labels and silent heap fallback are rejected; its non-defect control survives.
The local raw captures were independently checked against configuration, source identity,
budgets, bindings, allocation backing, residency and summary arithmetic. One release
executable was used throughout; the checkout is labelled dirty while this work was built.

See [README.md](../../README.md) for the discovery and capture commands. A new capture
uses a new scratch directory and never overwrites this one. Preserve the reported
unavailable relationships when moving the experiment to another host.