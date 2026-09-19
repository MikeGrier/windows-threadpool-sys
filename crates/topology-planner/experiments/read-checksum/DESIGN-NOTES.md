# Experimental contract

**Only offline EP-X2.1 is authorized while parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-R1.7.1` remains open.**
These protocols describe offline experiments, not required startup work. Parent
[EP-D-10](../../DESIGN-NOTES.md#ep-d-10) controls the planning and realization budget;
existing measurements and their experimental definitions remain retained.

## RC-D1: isolate candidates and hold work constant

The experiment compares one direct owner, a reader and processor joined by bounded
SPSC queues, and two independent direct owners. All use the repository's safe
overlapped-file/IOCP backend and the same per-block checksum. Separate orchestration
paths remain separate; shared file, checksum and instrumentation primitives are not
a shared scheduler.

Configuration explicitly names the fixture, block size, total depth, handoff capacity,
checksum passes, repetition count and timeout. Read depth and payload-buffer count
are separate powers of two so two readers receive equal halves; the latter defaults
to depth in older configurations. Every variant allocates the same total payload-buffer
capacity. Queue, token and result metadata are additional overhead and are identified
as such, not included in the payload-buffer figure.

A master read-only handle denies concurrent writes/deletion throughout reference
reading and trials. Each block's result is compared with a synchronous reference
outside timing. The last block may be short. Missing, duplicate, changed, or incorrect
results fail the experiment rather than producing a success record.

## RC-D2: bounded, labelled measurements

The first experiment uses buffered file reads after a full reference read. This warms
the cache but does not guarantee cache residency. It is closed-loop saturation, not
an offered-arrival latency experiment and not a measurement of raw NVMe bandwidth.
All IOCP owners poll completions and yield when no progress is available.

Payload allocation and endpoint construction precede the common start gate.
Work-wall timing starts at gate release and ends when the last worker finishes its
measured loop and I/O/buffer drain. Joined-wall timing also includes post-run page
sampling and thread teardown. Summed thread CPU times cover the measured worker phase.
Latency starts at read submission and ends at checksum completion; pipeline waiting
starts at read-completion observation and includes handoff backpressure. No claim
about pre-admission latency is made.

Every repetition contains all arrangements in both orientations under `RC-D7`'s
balanced schedule. Untimed warm-up runs precede recorded repetitions. Report individual
samples rather than a winner or a ratio to an unstated baseline.

## RC-D3: placement is observed, not presumed

Select two active processors on distinct cores with matching reported efficiency
class in a reported memory domain, or accept an explicit pair
from configuration. Pin each worker with group-aware affinity and check the actual
processor at the start and end. Record the machine topology and each role's binding.
The one-worker arrangement uses the first processor after applying trial orientation. The independent-worker control
uses the same pair as the pipeline.

Allocate and touch payload buffers on their I/O owner. Query their page residency
before and after timing; this is an observation, not a first-touch guarantee. Unresident
pages remain explicitly unknown. API failures are errors, not fabricated node zero.
Heap/queue/stack placement is not controlled or claimed. Cross-node measurements
require an explicit pair and actual page observations; a single-domain host cannot
supply them.

## RC-D4: safety, artifacts and later disposition

All queues and payload pools are bounded. Failures stop new work and propagate
cancellation to the peer; outstanding overlapped operations retain their buffers
until the owning backend completes rundown. A deadline stops admission and processing,
but cannot promise a broken driver has completed cancellation by that deadline.

Output is structured through a writer, with explicit failure status. Fixture creation
and report output use create-new semantics; existing files are never overwritten.
Local bulk captures and fixture files live under the workspace's ignored scratch
directory. A small reviewed JSON capture may be committed here with its provenance.

The first review retains or revises each experimental path explicitly. Retention does
not make it a production contract. The planner's evolving contract remains owned by
[CHECKLIST.md](../../CHECKLIST.md) `EP-R1.7`.

## RC-D5: capture identity and failure coverage

Build provenance carries the compiler/target/optimization flags, checkout revision
and state, a tracked-diff fingerprint and a harness-plus-manifests fingerprint.
These FNV fingerprints are change identifiers, not cryptographic authentication.
They are computed at build time; a dirty build is labelled dirty. The reference read
and each trial have a cooperative deadline; checksum loops check it between chunks.

The deterministic and live cases exercise data integrity, tail handling, credit
bounds, queue pressure, invalid configuration, missing/locked files, cancellation
signalling and deadline expiry. OS resource exhaustion, failed residency queries,
affinity refusals on restricted hosts, and driver cancellation that never completes
are not manufactured by this suite. Their API errors propagate; driver rundown has
no hard completion bound. A failed run writes an error record, not partial results
labelled successful.

## RC-D6: retain the comparison paths without promoting an archetype

Retain all three isolated paths: the direct owner remains the single-worker baseline,
independent workers remain the extra-CPU control, and the handoff path remains the
stage-separation candidate. Do not merge their schedulers or promote any one to the
planner's preferred arrangement from this capture. No production implementation
change is scheduled by this decision.

The observations and limitations are in the
[capture record](captures/2026-09-19/README.md). They require keeping processing
parallelism distinct from stage separation in the next design discussion. Return
to parent [CHECKLIST.md](../../CHECKLIST.md) `EP-R1.7` for that discussion.
Its `MX1` and `EP-X2.1` now schedule the additional experimental dimensions;
the experiment no longer maintains a separate action checklist.

## RC-D7: EP-X1.1 controlled repeatability

Retain the direct, pipeline and independent schedulers unchanged. Each repetition
runs each arrangement with the selected processor pair in both orientations.
A six-treatment Williams schedule balances absolute positions and ordered adjacent
treatments within each six-repetition cycle. Warm each treatment before recording.
The direct owner moves to the other processor in the reversed orientation; the
pipeline swaps reader/processor roles; independent workers swap their block lanes.

Use the deterministic 33,554,449-byte fixture, 65,536-byte blocks, total depth eight
and queue capacity four. Capture twelve repetitions per case: one checksum pass,
sixteen passes, then the same one-pass control again, using one release executable
and the same explicit processor pair throughout. This is a bounded buffered-file
comparison after reference reading, not device-path evidence. No elevation, external
endpoint or writes outside experiment-owned fixture/report files are permitted.
The payload ceiling is 524,288 bytes per trial; aggregate pending reads are bounded
by eight. Other allocation ceilings remain those enforced by `Config::validate`.
Each trial and reference phase has a 30-second cooperative deadline; stop the
capture sequence on any correctness, placement, setup or deadline failure.
Driver cancellation retains the existing unbounded rundown caveat.

Pipeline versus independent workers matches two participating CPUs, aggregate read
credits and payload bytes, while changing checksum-capable workers from one to two.
Direct versus pipeline changes both stage separation and CPU capacity: it is an
explicitly unequal-CPU reference, not an isolated estimate of overlap. Pipeline
versus independent also changes I/O ownership and handoff, so their difference is
not a pure checksum-speedup measurement. Record these limits rather than assigning
either difference to one mechanism.

Retain every trial's throughput, latency, thread CPU, correctness outcome and actual
bindings. Compare same-arrangement/orientation ranges within and between control
captures, and compare orientations separately before any aggregation. Report range
overlap and paired differences without a significance claim or universal ranking.
If same-code spread overlaps candidate differences or role reversal changes their
sign, carry the controls into EP-X1.2 and do not choose a sweep baseline by rank.
Review the observations and retain/revise/merge/delete disposition here before
returning to parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-R1.7`.

## RC-D8: retain paths and distinguish comparison resources

Retain the direct reference, pipeline and independent workers as separate paths.
None is merged or deleted after EP-X1.1; none becomes a production scheduler.
Revisit their retention at the parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-X3.3`
synthesis, or earlier if a subsequent experiment changes the tested question.
The [capture record](captures/2026-09-19-ep-x1-1/README.md) contains the observations;
the parent [DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md#ep-x11-comparison-evidence)
records why these controls were added.

Comparison descriptors distinguish participating processors, worker threads,
checksum-capable workers, read credits and payload bytes. Actual role bindings and
per-worker completed checksum work accompany those ceilings. CPU time is an
observation, not a resource allowance or a count of useful compute workers.
Record its accounting granularity; do not clamp coarse CPU/wall ratios to the
participating processor count.

Stage separation and checksum parallelism remain separate candidate dimensions.
The direct reference is intentionally unequal in CPU capacity; a comparison that
also changes I/O ownership and handoff must name those changes, not attribute its
difference solely to stage overlap or checksum replication.

EP-X1.2 retains all arrangements, role reversals, balanced positions and same-code
controls around its one-factor sweeps. Review orientations separately and retain
outliers; overlapping same-code/candidate differences remain inconclusive. No
arrangement is selected as the sole sweep baseline by its EP-X1.1 throughput rank.
This action is queued in the parent's item, not deferred to an unscheduled review.

## RC-D9: EP-X1.2 parameter isolation

Use the same deterministic 33,554,449-byte buffered fixture and pinned pair for
every case, with all arrangements and both orientations. Each case has six balanced
repetitions and untimed warm-up. Use one release executable, a 30-second cooperative
deadline per setup/reference/trial phase, and stop on any failure. No elevation,
external endpoint, device mode or writes outside owned scratch files are authorized.
Existing cancellation/rundown limits remain unchanged.

The baseline is 65,536-byte blocks, eight aggregate pending reads, 32 payload buffers,
queue capacity four, one checksum pass and batch size one. Separate pending-read
credits from payload-buffer credits; old configurations default buffer count to depth.
Every pair matches read/payload budgets. The labelled single-CPU reference remains
unequal only in CPU capacity.
Metadata is outside the payload budget and remains bounded by the fixture and pools.

| Dimension | Lower / baseline / upper values | Held constant or explicitly normalized |
|---|---|---|
| Block bytes | 16,384 / 65,536 / 262,144 | Payload pool stays 2 MiB by setting buffer count to 128 / 32 / 8; outstanding byte ceiling and block count necessarily change |
| Checksum passes | 1 / 1 / 16 | All other fields fixed; baseline also supplies the low point |
| Read depth | 2 / 8 / 32 | Buffer count remains 32, unlike the old coupled depth/pool setting |
| Queue capacity | 1 / 4 / 16 | Payload pool and read depth fixed; this setting only affects handoff |
| Batch size | 1 / 1 / 16 | All other fields fixed; include an intermediate size four |

The compute sweep includes intermediate passes four. Each one-factor sweep runs
baseline, first non-baseline point, second point, second point, first point, baseline.
The selected queue/batch interaction runs the four corners of queue {1,4} x batch
{1,16}, then reverses their order. This tests whether batching's effect changes under
a narrow handoff queue; no other interactions are claimed measured.
Direct and independent paths do not consume queue capacity, so their rows in that
sweep provide additional same-code controls rather than queue-effect measurements.

Batch size is a scheduling quantum, not an OS vectored-I/O or atomic queue operation.
Direct workers submit available reads then process up to the quantum before refill.
The pipeline reader drains up to the quantum of returns, refills, then forwards up
to the quantum; its processor processes and returns up to the quantum per turn.
Every phase stops early on empty/full, never waits to fill a batch, and checks the
cooperative budget per job. A full handoff retains at most one waiting payload.
Partial batches, including the short final block, complete without padding or loss.

Record effective configuration, per-worker read and buffer capacities, observed
read/lease peaks, buffer-pressure observations, batch sizes/partial batches, queue
high-water/full attempts, CPU, throughput and existing stage/queue latency metrics.
Per-worker peaks remain per-worker observations, not a simultaneous global peak.
Review each orientation separately against the bracketing same-code spread. Retain
raw samples and inconclusive differences; no universal queue or batch size is chosen.
Return the result to parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-R1.7` for
discussion before closing the item or starting `EP-X1.3`.

## RC-D10: offline placement comparison

EP-X2.1 is explicitly authorized offline research, not startup characterization.
Retain the existing buffered-file backend and schedulers. Add a deterministic
generated-buffer companion with the same logical fixture bytes, checksum work,
block identities and credit limits. Label generation as producer-side CPU work,
not device service. Keep the existing heap-buffer path as the default baseline;
explicit placement uses owned, page-aligned NUMA-preferred allocations and records
requested versus observed payload residency before and after the measured phase.
Do not represent an allocation preference or a synthetic topology as achieved placement.

Use the EP-X1.2 baseline: 33,554,449 logical bytes, 65,536-byte blocks, eight read
credits, 32 buffers, queue capacity four, one checksum pass and scheduling quantum
one. Every candidate uses the same payload/read ceilings; the direct reference is
still unequal in CPU capacity. Capture all three arrangements in both orientations
with six balanced repetitions. Select at most one deterministic pair for each
observed core/cache/memory relationship, without ranking those relationships.
For each pair vary payload node independently, using each endpoint's observed node;
reverse worker roles at each placement. Deduplicate identical node choices.

Selection reports absent or unknown relationships explicitly. Test selection on
synthetic single/multiple-node, cache and group layouts, but obtain timing only on
the live host. Pin actual workers and retain topology and requested/observed bindings.
NUMA allocation is a preference: residency mismatches and unresident/unrepresentable
pages remain visible, never relabelled local. An API refusal is an error, not node zero.
No cross-node conclusion is drawn unless that pair and observed residency exist.

Bracket each workload/pair's placements with same-code controls and reverse placement
order. Preserve raw trials, per-block correctness, throughput, latency, CPU, queue
pressure and page observations with source/configuration provenance. Keep file/cache
evidence distinct from generation; keep direction distinct from physical relationships.
The generator writes each leased buffer before handoff; requested residency does not
mean that buffer was initially written by the consumer.

Only an experiment-owned fixture and reports under scratch may be written. No elevation,
device path, network traffic or machine configuration changes are authorized. Payload
capacity is 2 MiB per trial, with the existing bounded metadata limits. Setup, reference
and trial phases retain 30-second cooperative deadlines and the existing driver-rundown
caveat. Stop a case on correctness, binding, allocation, residency-query or deadline
failure; preserve its error rather than emitting successful timing. Capture unavailable
placements separately from execution failures and make the campaign report name both.

The current host exposes only Windows NUMA node 0. The engineer authorized implementation
and local capture now, leaving cross-NUMA timing unrun for a multi-node host. EP-X2.1
remains open for that evidence and result review. No other MX item is implicitly resumed.
