# Request/reply contract

## RR-D1: two candidates and one request contract

Requests have unique IDs, logical lanes, scheduled arrival offsets, a deterministic
input value and bounded integer service work. No external effects are performed.
Each admitted request produces exactly one correlated terminal reply: completed with
the reference result, or cancelled. Replies may reorder; lane assignment promises
one processing owner, not a new application ordering requirement. Unadmitted requests
are counted and identified separately. Admission waits under pressure, never silently
rejects, reroutes or drops a request. Invalid configuration/trace is rejected before
workers are started. Empty input is legal.

Shared service uses a single bounded crossbeam-channel request queue with competing
worker receivers. Assigned lanes use the same backend with one receiver per lane and
no work stealing. Logical lanes equal the worker count, including for shared service.
Workers are unpinned: logical ownership does not claim CPU, cache or NUMA locality.
Neither path is built by treating an SPSC/MPSC API as multi-consumer.

Equal global credits bound admitted requests from admission through reply collection,
including queued, executing and terminal-but-uncollected work. Credits are released
only by collecting a verified terminal reply. Both candidates reserve the same total
request-channel slots: shared capacity equals the credit ceiling; lane capacities
divide it equally. The ceiling must be divisible by worker count. A reply channel
of the same total capacity cannot deadlock a worker holding an admitted credit.
Report per-lane outstanding peaks and admission-full observations, not a fabricated
simultaneous sum of queue peaks. Input traces and final records are separately bounded.

## RR-D2: arrival, response and rundown

Steady traces use evenly spaced absolute offsets from one start gate. Burst traces
assign the same offset to each group and advance by a fixed inter-burst interval.
Never reset the next scheduled time to the previous completion: generator lateness
and pre-admission waiting are part of response time. Service work and lane skew are
deterministic inputs; the computation, result oracle and budgets are identical for
both candidates. Per-request records include scheduled, admitted, processing-start,
terminal and collected offsets; aggregate and logical-lane latency distributions
use scheduled arrival through terminal outcome, with cancelled outcomes counted separately.
First-offer time is also recorded so generator lateness and subsequent admission
waiting are separately visible. The coordinator admits in trace order: a full
assigned lane can block later arrivals for other lanes. That mechanism is recorded,
not mistaken for an aggregate capacity shortage or silently removed by rerouting.

Both candidates block on empty request queues. The coordinator waits for either
the next arrival, reply or a bounded cancellation check interval. The fixed policy
is not an idle-policy sweep. Each worker checks cancellation during bounded compute
chunks. Cancellation stops new admissions and causes queued/active work to return
terminal outcomes; a request already completed stays completed. An explicit test
hook after a declared admission count makes cancellation coverage deterministic.
Deadline expiry has the same rundown, recorded separately from successful completion.
All spawned workers must join; errors/panics cancel peers and drain/drop queued work.

Record per-worker active service time and handled outcomes plus experiment wall time.
Active-time/wall ratios are scheduler observations, not OS CPU accounting or proof
of physical utilization. Store per-request identities and per-worker assignments so
correlation and lane ownership can be checked independently of timing. No performance
threshold, ranking or physical NUMA timing is needed for behavioral acceptance.

## RR-D3: bounds and validation

Configuration ceilings: 1..=16 workers, worker-count..=4096 credits, at most 16384
requests, at most 1000000 service steps per request, scheduled span at most 1 second,
and a 1..=5000 ms cooperative trial deadline. The executable has a fixed demonstration
matrix: empty, steady, burst, skewed uneven-work burst, pressure and cancellation.
Use small fixtures and balanced shared/assigned order plus reversed retakes. Report
individual observations, same-code spread and configuration, never a fastest default.
Only a new report under the user's chosen path is written; no networking, elevation,
device changes, thread affinity or application data access is authorized.

Unit tests check ten or more deterministic shapes, invalid IDs/lanes/limits/times,
pressure, reply reordering, cancellation, deadline exhaustion and rundown. Use an
explicit synthetic timeline in report fixtures for deterministic schedule/accounting
edge cases; real-thread timing tests make no speed assertion. Persistent sabotage
must change actual routing, correlation, credit or timing-origin behavior and be
caught, alongside a non-defect control. The common result verifier owns the invariants.

Retain both paths through the result review; merge/delete only by an explicit later
decision. This experiment owns no topology or allocation policy, so it does not create
a second faux-NUMA model. Future placement consumers must adopt parent EP-D-11's
shared environment rather than infer hardware locality from these logical lanes.

## RR-D4: behavioral findings and disposition

Retain both isolated queue layouts and the common request/result contract. Merge or
delete neither: shared competition and assigned ownership are distinct legal candidates
for independent requests. No candidate is selected by timing. Revisit their eventual
production disposition under parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-X3.3`.
The [demonstration](captures/2026-09-19/README.md) retains the observed outcomes.

Candidate descriptors must distinguish logical request lane from actual worker,
shared eligibility from assigned ownership, aggregate admitted-work credits from
per-queue capacity, and admission order from completion order. Assigned ownership
without stealing exposes hot-lane pressure; a shared backend distributes eligible
work without promising an individual worker. Neither fact establishes NUMA locality.
Report the admission head-of-line behavior explicitly when it is part of a candidate.

Cancellation is not a missing result: admitted work has correlated terminal outcomes,
while unadmitted work stays identified. The completed/cancelled split may depend on
scheduling without breaking that census. Evidence must carry its response-time origin
and distinguish service-active wall time from CPU use. Preserve these requirements
when materializing the future planner/runtime contracts; no production API is created
by this experiment. The next workload remains subject to EP-R1.7.1 scope reconciliation.

## RR-D5: stateful shared-state and key-owned comparison

The engineer authorized EP-X2.3 as standalone offline behavioral work. Keep RR-D1's
stateless paths unchanged. Add separate stateful schedulers over the same bounded
crossbeam-channel backend: shared competing workers with one synchronized table,
and key-owned workers with private partitions and one queue per owner. Owner is
`key % workers`, fixed for the run, never a CPU or memory-domain identity. Record
placement as unpinned host scheduling; this experiment makes no locality claim and
introduces no second NUMA model or production startup workload.

Every request names a unique ID, key, absolute arrival offset, bounded service work,
and either lookup or wrapping-u64 add. Keys range over a bounded table initialized
to zero. The specified per-key order is the trace order, including lookups. Replies
may reorder across keys and retain the observed post-operation value. Shared workers
may prepare work concurrently but wait for each key's next sequence before commit;
key owners receive their partitions in trace order and need no shared state lock.
Shared locking is part of that candidate, not a lock added to the owned candidate.

Cancellation stops admission and resolves all admitted identities. A request cancelled
before its commit leaves the value unchanged but advances that key's sequence so
later admitted work can drain. Commit linearizes under the shared lock or exclusive
owner; cancellation racing after that point cannot undo the effect or erase its
completed reply. A cancelled suffix per key must never resume committing. Final
state and every reply value are verified by an independent serial replay over the
trace and reported outcomes; final equality alone is insufficient to verify lookups.

Both candidates use RR-D3's worker, trace, service-work and deadline ceilings. Add
1..=4096 keys. Match the total request slots, reply slots, admitted-credit ceiling,
table entries, service work and fixed idle policy within a pair. Table value/sequence
storage is equal; mutex, condition-variable, queue and report metadata are additional
and labelled. Credits last until verified collection; never release them at dequeue
or commit. Track requests from scheduled arrival and distinguish offer/admission,
queue/service, terminal and collection timestamps. Report aggregate, per-key and
per-owner-lane response distributions, outstanding peaks and admission-full attempts.

Use deterministic steady/burst arrivals, uniform/hot/all-hot key traces, lookup-only,
add-only and mixed operations, cheap/uneven service, pressure and cancellation. No
random input, speed threshold or hardware requirement. Empty input and short traces
are legal. Include non-power-of-two key/worker counts and wrapping arithmetic.
Bound all state by the declared keys, credits and trace size; abort/error paths join
workers and release queued payloads. Waiting for per-key order must observe peer
failure and cancellation, never strand a worker behind an abandoned sequence.

Use service-active wall time plus actual per-worker Windows kernel/user CPU time as
observations; accounting granularity can make short runs noisy. CPU time is not a
credit budget or physical-locality observation. No performance baseline is selected.
The CLI will execute a small fixed matrix with shared/owned/owned/shared retakes,
one executable, full configuration/provenance and raw outcomes. Do not overwrite
earlier captures. Retain both speculative paths until an explicit disposition review.
Test at least ten normal shapes and all new error edges; sabotage actual routing,
commit order, lookup/state updates, cancellation effects and verifier binding,
with a non-defect control. OS resource exhaustion is propagated but not manufactured.

## RR-D6: state-ownership findings and disposition

Retain the shared synchronized table and private key-owned partitions as separate
stateful candidates. Keep the stateless request/reply paths unchanged. Neither
stateful candidate becomes a planner default, and neither is merged or deleted
from a timing observation. Eventual production disposition stays queued under
parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-X3.3`.
The [stateful demonstration](captures/2026-09-19-stateful/README.md) retains the
observations supporting this review.

Candidate contracts must state key identity, partition function, ownership, commit
ordering and cancellation effect semantics. Shared eligibility is not concurrent
unordered mutation: this candidate coordinates each key's commit sequence.
Key ownership removes that shared lock from its path but retains serialization
within the owner and pressure when keys concentrate there. Key skew is workload
input, not a machine-topology fact; an unpinned logical owner is not a NUMA domain.

Final state alone does not verify a stateful service. Replay each key's intermediate
lookup/update outcomes in declared order, with cancellation leaving no value change
before commit and no rollback after it. Cross-key reply order may differ. Aggregate
credits include terminal replies not yet collected, independent of state ownership.
Record queue/admission pressure and per-key/owner-lane observations rather than
discarding hot-key waiting. Carry these requirements into the future plan/runtime
contract; this experiment does not create that production API or a hardware gate.

## RR-D7: serial and staged ordered ingestion

The engineer authorized EP-X2.4 as standalone offline behavioral work. Keep RR-D1's
stateless paths and RR-D5's stateful paths unchanged. Add a separate ingestion path
over the same bounded crossbeam-channel backend with two candidates: a serial owner
that transforms and publishes each record itself, and a staged pipeline whose
transform workers overlap while one publisher resequences into declared order.
Workers are unpinned; a stage is a logical role and never a CPU or memory domain.

Two constraints are deliberately distinct, and the experiment exists to separate them.
The **per-record dependency** is that a record's transform precedes its own publication.
The **global order** is that publications occur in declared trace order. A serial owner
satisfies both by construction; the staged pipeline satisfies them independently, so a
record whose transform finished early waits for its predecessors. Never describe one
constraint as implying the other, and never reorder publication to raise throughput.

Every record names a unique ID, an absolute arrival offset, a deterministic input value,
bounded transform and publish work, and an optional injected failure stage. Declared
order is trace order. The reference result for a published record is a pure function of
its input and transform work, identical for both candidates. An effect is visible only
through the publication log, which is in memory: this experiment performs no real output
I/O and makes no durability claim, so the item's effect/durability contract is declined
rather than invented. The log records published identities and values in publication order.

Each admitted record reaches exactly one terminal outcome: published with its reference
value, aborted at a named stage, or cancelled. **An aborted or cancelled record still
advances the publication cursor**, or the pipeline stalls behind work that will never
publish; it contributes no entry to the log. Published identities therefore form a
strictly increasing subsequence of the trace, with no gap before the admitted prefix ends.
A record that fails at transform never publishes. A record that fails at publish leaves
no effect. Cancellation stops admission and resolves every admitted identity; a record
cancelled before its publication leaves no effect, and one already published keeps both
its effect and its terminal outcome. A cancelled suffix never resumes publishing.

The staged candidate's resequencing buffer is bounded and its capacity is part of the
candidate, not an incidental allocation. When the next record to publish is slow, later
finished records occupy that buffer and transform workers block: that is head-of-line
delay and is reported, never removed by reordering or by an unbounded buffer. Report
per-record ready and published offsets so the delay between them is separately visible,
along with buffer occupancy peaks, buffer-full observations and admission pressure.
The serial candidate has no such buffer; report zero rather than omitting the field.

Both candidates use RR-D3's worker, trace, service-work and deadline ceilings, and
RR-D1's global admitted-credit rule: credits last until a verified terminal outcome is
collected, never released at transform or publication. Match the request slots, reply
slots, credit ceiling, transform/publish work and fixed idle policy within a pair.
The serial candidate runs one owner; the staged candidate's transform worker count is
the configured worker count. Resequencing buffer entries, mutex, condition-variable and
report metadata are additional and labelled, not hidden inside a claimed equality.

Verification is an independent serial replay over the trace and reported outcomes: it
recomputes each published value, checks the publication log equals the published
subsequence in declared order, checks each record's own transform-before-publish
timestamps, and checks abort/cancellation left no effect. Log equality alone is
insufficient, exactly as final state was for RR-D5. Credit conservation, worker census
and outcome census are checked as in RR-D1 and RR-D5.

Use deterministic steady/burst arrivals, cheap and uneven transform work, publish-heavy
and transform-heavy mixes, injected failure at each boundary, pressure and cancellation.
No random input, speed threshold or hardware requirement. Empty input and short traces
are legal; include non-power-of-two worker counts, a single-record trace and a trace
whose slowest record is first. Bound all state by the declared credits, buffer capacity
and trace size; abort/error paths join workers and release queued payloads. Waiting for
publication order must observe peer failure and cancellation, never strand a worker
behind a record that will never arrive.

Test at least ten normal shapes and all new error edges; sabotage actual publication
order, the cursor advance on abort, cancellation effects, the transform result and the
verifier binding, with a non-defect control. Retain both candidates until an explicit
disposition review. This experiment owns no topology or allocation policy and selects
no timing winner; ordering cost is a workload property here, not a machine fact.

## RR-D8: ordered-ingestion findings and disposition

Retain the serial owner and the staged pipeline as separate ingestion candidates, and
keep the stateless and stateful paths unchanged. Neither becomes a planner default and
neither is merged or deleted from a timing observation. Eventual production disposition
stays queued under parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-X3.3`. The
[ingestion demonstration](captures/2026-09-19-ingest/README.md) retains the observations.

**A candidate contract must state the per-record dependency and the global publication
order as two rules, not one.** The serial owner satisfies both by construction and so
can report nothing about their separate cost; the staged pipeline satisfies them
independently, which is what makes the resequencing window, the blocking on it, and the
resulting head-of-line delay observable at all. A descriptor that records only "ordered"
cannot distinguish the two, and a planner reading it would have no basis for choosing.

**Declared publication order survived overlap in every drained scenario**, including a
one-slot window, credit pressure and a deliberately slow first record. That is a
property of the resequencing rule rather than of the schedule: the same runs recorded
transform workers blocking on the window throughout, so the order was preserved against
real reordering pressure rather than in its absence. Ordering cost is therefore a
workload property to be described, not a machine fact to be measured once.

**The bound is part of the candidate and must be stated with it.** The window is
`reorder_capacity` positions from the cursor, which is what keeps the staged path from
deadlocking against its own bound while still letting the bound bite. An unbounded
resequencer would hide head-of-line delay inside memory growth and report neither.

**An aborted or cancelled record must still advance the publication cursor.** Without
that, a pipeline stalls behind work that will never publish. Abort and cancellation are
distinct: an abort names the stage its record failed at and leaves the rest of the run
publishing, while cancellation ends publication for every later record. Publication is
authoritative for both, so outcomes are a published/aborted prefix and an all-cancelled
suffix; a cancelled suffix never resumes, and an effect already published is never
retracted. The completed/cancelled split may vary with scheduling without breaking that
census, exactly as RR-D4 requires for replies.

**Log equality alone does not verify an ordered ingestion.** Each published value must
be recomputed and each log entry checked against the declared order, because a candidate
that published the right multiset in the wrong order, or the right order with a drifted
value, produces a log of the right length either way. Carry these requirements into the
future plan/runtime contract; this experiment creates no production API, no durability
contract and no hardware gate.