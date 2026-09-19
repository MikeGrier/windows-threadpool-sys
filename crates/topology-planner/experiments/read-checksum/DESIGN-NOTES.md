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

Every repetition contains all arrangements, with their order cycling through the six
permutations. Untimed warm-up runs precede recorded repetitions. Report individual
samples rather than a winner or a ratio to an unstated baseline.

## RC-D3: placement is observed, not presumed

Select two active processors on distinct cores with matching reported efficiency
class in a reported memory domain, or accept an explicit pair
from configuration. Pin each worker with group-aware affinity and check the actual
processor at the start and end. Record the machine topology and each role's binding.
The one-worker arrangement uses the first processor. The independent-worker control
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
