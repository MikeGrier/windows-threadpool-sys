# Topology-first planning and startup cost

Recorded 2026-09-18 23:49:18 -07:00. Discussion summary, not a verbatim transcript.
Resulting decision: [DESIGN-NOTES.md](../DESIGN-NOTES.md) -> `EP-D-10`.
Follow-up: [CHECKLIST.md](../CHECKLIST.md) -> `EP-R1.7.1`.

## How the question arose

After read/checksum implementation and parameter sweeps, the engineer asked what
had happened to the proposed sequence of roughly five experiments. The research
sequence covered read/checksum, independent requests, key-owned state, ordered
ingestion and network processing. The expanded checklist had placed extensive
characterization of the first workload ahead of the other workloads.

The assistant distinguished measurement controls needed for comparisons from
exhaustive tuning of read/checksum. EP-X1.2 was then implemented and captured at
the engineer's request. Its result discussion proposed retaining the experimental
paths and selecting fixed cases for an idle-policy sweep; that selection was not
accepted before the product-scope question below arose.

## The engineer's expectation

The engineer expected discovery of the actual memory topology, extremely limited
experiments or none, and a pattern matched from application design to available
architecture. Memory domains and I/O device attachment were expected to be primary
inputs. NVMe locality motivated placing buffers near the endpoint and placing
workers that parse or fill those buffers near the memory. Work distribution and
queues would connect those anchors to other processors and memory areas.

The engineer asked whether NIC attachment had comparable NUMA implications,
without assuming an answer. The assistant distinguished physical attachment,
receive steering, driver processing, application scheduling and buffer residency;
the exchange was not a measurement of NIC versus NVMe locality costs.

Work-item size and granularity sweeps were unexpected. The engineer considered
stopping, but explicitly left open that this investigation might be correct.
The concern was that high-quality guidance might require a substantial startup
workload campaign, contrary to the original expectation.

## Clarification and agreement

The assistant separated architectural matching from parameter tuning and noted
that the selected-pair buffered-file captures had not tested endpoint attachment
or cross-NUMA placement. Their variation did not establish a need for runtime
size-mix validation. It proposed keeping the experiments as offline research,
supporting zero-probe planning, limiting optional probes to particular unanswered
questions, and pausing further implementation for scope reconciliation.

The engineer agreed and clarified: some extremely small cost for planning and
realizing the topology is expected, but it must not be proportional to running
workloads long enough to validate mixes of sizes. No numerical startup budget
was supplied, and no particular queue, batch or block size was selected.

The current contract is in [DESIGN-NOTES.md](../DESIGN-NOTES.md), not in this
historical summary. Existing code and captures remain retained; the agreement
does not silently complete EP-X1.2, delete experimental paths or collapse the
measurement foundation into another layer. The follow-up item reconciles the
budget, missing-evidence policy, acceptance criteria and remaining research queue.

## Follow-up: consistent faux NUMA

After the available EP-X2.1 capture, the engineer asked whether NUMA artifacts could
be injected through gathering and questioned the repeated statement that work
remained open for physical timings. The assistant distinguished behavioral testing
from cost measurement and acknowledged that it had made the latter a completion
gate without that following from the clarified product goal.

The engineer accepted a shared fidelity limitation until actual multi-node hardware
can be obtained: use a consistent "faux NUMA" model uniformly, while acknowledging
that it cannot mimic memory-distance effects. The engineer rejected leaving every
item and milestone undone for the same unavailable hardware. High-quality NUMA
behavioral testing is still required; the physical gap is one uniform problem.

The resulting policy is [EP-D-11](../DESIGN-NOTES.md#ep-d-11). Its follow-ups are
the shared faux environment, `EP-R1.7.2`, and one non-blocking physical-hardware
validation item, `EP-HW.1`, in [CHECKLIST.md](../CHECKLIST.md). No synthetic results
are renamed as hardware measurements, and no existing captures are rewritten.