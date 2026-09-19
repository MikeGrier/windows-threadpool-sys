# EP-X2.3 stateful ownership demonstration

Captured on 2026-09-19 UTC with one release executable. The
[raw report](stateful.json) holds source/build identity, ordered configurations,
traces, per-key/per-worker outcomes, credit events, intermediate lookup/update
results, final states and latency/CPU observations. The checkout was dirty during
implementation; the report's source/manifests fingerprint identifies the measured
code. No observation has been filtered into a selected timing result.

The protocol is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RR-D5`; commands and
field definitions are in [README.md](../../README.md#stateful-comparison).
This is unpinned in-memory service, not a physical locality or startup measurement.

## Observations

All normal paired runs produced the same final table and passed independent
per-key serial replay, including the intermediate lookup results. Lookups did not
change state. The empty case drained with no admitted work. Recorded outstanding
credits stayed within their configured bounds.

Key-owned runs kept each key at its assigned worker. Hot/all-hot keys concentrated
work accordingly and produced lane-full admission observations. Shared workers
could process requests for the same key but still committed them in per-key order;
their assignment counts varied across same-code retakes. One all-hot shared retake
was handled entirely by one worker; another used more than one. Shared eligibility
is therefore recorded separately from the worker assignment actually observed.

Cancellation stopped admission at its declared boundary. Each admitted request
had one completed or cancelled outcome, and the remainder stayed unadmitted.
The completed/cancelled split varied, while each final state matched replay of its
completed effects. The capture contains no proof of a faster default, universal
key partition or real NUMA placement. Short-run CPU observations retain the OS's
accounting granularity, including zero samples.

## Behavioral Verification

Unit tests force out-of-order preparation on a shared key, verify commit sequence
and intermediate values, and cancel before effects occur. They exercise wrapping,
sparse request IDs, non-power-of-two key/worker counts, unknown owners, poisoned
state, worker panic/service/CPU failures, and peer rundown. Synthetic report tests
accept reordered replies and reject incorrect state, ownership, sequence, timestamps
and credits. CLI tests replay every recorded report through the same verifier and
compare per-key summaries with their derivation.

[sabotage.json](../../sabotage.json) verifies detection of incorrect updates,
mutating lookups, effects on cancellation, wrong owner routing, ignored sequences
and bypassed final verification. The equivalent keyed service-chunk control
survives. Existing stateless checks remain present and passed as well.

The current path disposition is [DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> `RR-D6`.
Parent [CHECKLIST.md](../../../../CHECKLIST.md) -> `EP-R1.7` owns contract synthesis;
subsequent paused work is not resumed by this record.