# Experimental contract

## RC-D1: isolate candidates and hold work constant

The experiment compares one direct owner, a reader and processor joined by bounded
SPSC queues, and two independent direct owners. All use the repository's safe
overlapped-file/IOCP backend and the same per-block checksum. Separate orchestration
paths remain separate; shared file, checksum and instrumentation primitives are not
a shared scheduler.

Configuration explicitly names the fixture, block size, total depth, handoff capacity,
checksum passes, repetition count and timeout. Depth is an even power of two so two
readers receive equal halves. Every variant allocates the same total payload-buffer
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
