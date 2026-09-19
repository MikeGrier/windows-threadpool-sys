# First read/checksum capture

Captured on 2026-09-19 UTC. Configuration, timestamps, build provenance, topology,
placement samples and individual trials live in the JSON artifacts, not in
hand-maintained tables here. All captures used the same release executable.
The recorded checkout was dirty because the experiment was being implemented;
the harness-and-manifests fingerprint identifies the measured source.

| Artifact | Purpose |
|---|---|
| [small-block.json](small-block.json) | Small blocks and a narrow handoff queue. |
| [large-block.json](large-block.json) | Larger blocks with a larger handoff queue. |
| [compute-heavy.json](compute-heavy.json) | Repeated checksum work per block. |
| [deeper-pool.json](deeper-pool.json) | Larger total read/buffer allowance and handoff capacity. |
| [small-block-repeat.json](small-block-repeat.json) | Same-code repeat of the first configuration after the other runs. |

## Observations

Independent direct workers had higher median throughput than the handoff arrangement
in every captured configuration. Individual samples still varied: the repeated
small-block control contains an independent-worker sample below the direct-owner
range. This was a development host, not an isolated performance environment.

In the compute-heavy capture, direct-owner and handoff throughput ranges overlap,
while the handoff uses more worker CPU per unit of wall time. Independent workers
perform checksum work on both participating workers; the handoff arrangement has
one checksum worker. The mechanism is visible in [experiment.rs](../../src/experiment.rs);
the arrangements' definitions remain in [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

All sampled payload pages were on one reported node. This capture does not establish
cross-NUMA behavior. The reference read warmed the buffered-file path; it does not
establish device bandwidth. Latency is measured after admission, not from an
independently paced arrival stream.

Several configuration dimensions change between the illustrative cases. They are
not a factorial sweep and do not isolate the effect of queue capacity from read
depth or block size. No platform-general ranking is assigned.

## Disposition

The retain/revise decision is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RC-D6`.
Next design discussion returns to parent [CHECKLIST.md](../../../../CHECKLIST.md)
`EP-R1.7`, which remains open. Expanded experiments are tracked as `RC-2` in the
experiment's [CHECKLIST.md](../../CHECKLIST.md).

To reproduce, regenerate the fixture at the path and byte count in an artifact,
then use its `config` object as the executable's configuration. The commands and
measurement field definitions are in [README.md](../../README.md). A retake is a
new artifact, never an overwrite of these observations.
