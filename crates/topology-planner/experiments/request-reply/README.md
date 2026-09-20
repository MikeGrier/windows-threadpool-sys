# Request/reply experiment

Unpublished offline experiment for parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-X2.2`.
The separate EP-X2.3 stateful path is described under [Stateful Comparison](#stateful-comparison)
below; the original `capture` command and stateless contracts remain unchanged.
The contract is [DESIGN-NOTES.md](DESIGN-NOTES.md); the
[recorded demonstration](captures/2026-09-19/README.md) is behavioral evidence,
not a performance baseline or a production startup operation.

Build the `windows-request-reply-experiment` package in release mode, then run from
the workspace root with an existing scratch directory and a report path that does
not already exist:

```powershell
.\target\release\windows-request-reply-experiment.exe capture .scratch\request-reply-retake.json
```

The CLI uses one output writer for structured status/errors and create-new report
semantics. It executes a fixed bounded matrix in shared/assigned/assigned/shared
order. Each pair has identical traces, worker count, credit ceiling, service work,
idle policy and cancellation condition. A failure produces a non-success report
instead of labelling partial observations successful. The original source/build
identity and all individual trials are retained. The FNV source fingerprint is a
change identifier, not cryptographic authentication.

## Candidate semantics

`shared` uses one bounded MPMC request queue; cloned receivers compete for work.
`assigned` uses one bounded queue per worker and routes by logical lane. There is
no work stealing in the assigned path and no promise of a particular worker in
the shared path. Both use crossbeam-channel for requests and replies; neither
adapts an SPSC/MPSC API into an unsupported multi-consumer topology.

Admission walks the scheduled trace in order. If the next assigned lane is full,
later requests wait even when their own lanes have room. That admission head-of-line
effect is part of this candidate, not concealed by rerouting or skipping a request.
All admitted requests count against a single credit ceiling until their reply is
collected, including completed-but-uncollected work. Both candidates reserve the
same aggregate channel slot counts; per-queue/thread bookkeeping is additional.
Empty queues block. The coordinator checks cancellation while waiting, using the
same fixed interval for both candidates; this is not an idle-policy comparison.

Worker placement is unpinned. A logical lane means request ownership, not processor,
cache, memory or device locality. No NUMA facts are gathered or synthesized here.
This in-memory backend models application request service, not kernel I/O completion
dispatch, network transport or the Windows thread pool. Those layers are not inferred
from the observed worker assignments.

## Results and accounting

Requests carry unique IDs, a lane, absolute scheduled offset, input value and work
count. The computation adds the integers through the work count with wrapping u64
arithmetic; the reference uses the independent closed-form sum. Replies retain IDs
regardless of completion order. Each admitted request has one terminal completed or
cancelled reply; all other IDs are explicitly unadmitted. No rejection/drop policy
is used. A completion racing cancellation may remain completed.

Each reply records `scheduled_ns`, `offered_ns`, `admitted_ns`, `started_ns`,
`finished_ns` and `collected_ns` relative to the common start gate. First-offer
lateness includes coordinator scheduling and earlier admission blocking. Admission
waiting is separate. Response latency is finished minus scheduled, never finished
minus admitted. Completed and cancelled distributions are separate; unadmitted
requests have no invented response time. They remain present in the trace/census.

`credit_events` is the coordinator's ordered admission/collection history. The
common `verify` function checks correlation, reference results, lane ownership,
timestamp ordering, trace-prefix admission, credit ceilings, per-lane peaks,
exact terminal/unadmitted membership and per-worker totals before success returns.
`response_summary` validates the report and derives aggregate or logical-lane
nearest-rank distributions. It does not classify a result by throughput.

Worker `active_ns` is the sum of service intervals, including OS preemption while
inside service. Dividing by `wall_ns` gives a service-active fraction, not OS CPU
utilization. `handled`, `completed` and `cancelled` expose the worker distribution.
Lane peaks count outstanding credits, not simultaneous queue occupancy. Full
observations count failed admission attempts/checks, not unique stalls or durations.
Final wall time includes coordinator execution and joined worker teardown, not
channel construction and spawning. No thread priority or affinity is changed.

Cancellation stops admission, cooperatively terminates computation at bounded
chunks, drains terminal replies and joins workers. Deadline expiry follows the
same rundown with `stop: deadline`. The optional `cancel_after_admitted` test
condition makes its admission boundary deterministic; completed/cancelled races
are still allowed. Worker panic/error aborts the run and joins peers without a
successful report. OS thread-spawn/resource exhaustion is handled but not forced
by tests; the suite injects worker failures and channel disconnections directly.

## Validation

Run `cargo test -p windows-request-reply-experiment` for deterministic model and
real-thread unit cases plus the live process/filesystem CLI test. Synthetic report
fixtures assert exact timestamp arithmetic and accept reordering while rejecting
corruption, ownership errors and credit defects. Prefilled channels make queue-full
and cancelled-drain tests independent of scheduler speed. No test asserts a speed
ranking or samples random inputs at runtime.

[sabotage.json](sabotage.json) challenges actual routing, reply identity, latency
origin, global credit enforcement and the runner's binding to its verifier, with
an equivalent compute-chunk control. Use
[run-sabotage.ps1](../../../../tools/run-sabotage.ps1) for that manifest.

## Stateful Comparison

EP-X2.3 uses [stateful.rs](src/stateful.rs) and its separate
[engine.rs](src/stateful/engine.rs). Both candidates run the same zero-initialized
counter/lookup contract: `lookup` returns the current value, and `add` wraps modulo
the u64 range and returns the new value. Every key follows trace order, independent
of request ID sorting or worker completion order. A serial replay checks each
intermediate reply and final table, including read-only, mixed and wrapping cases.

`shared_state` uses competing receivers and one mutex-protected table with a
condition variable. Workers prepare bounded service work outside the lock, then
wait for that key's next sequence to commit. The table lock also serializes the
short commit sections across different keys. `key_owned` routes to `key % workers`
and keeps the corresponding table partition private in that worker. It does not
acquire the shared table lock or steal keys. This is one pair of concrete designs,
not a claim that all shared-state designs have one global lock.

Cancellation before commit leaves the key's value unchanged and advances its
sequence; later admitted requests for that key drain as cancelled. Commit is the
state-change boundary. Cancellation after it cannot erase a completed effect or
its reply. Consequently different candidates may have different final states on
cancelled runs, but each must equal its own serial replay of completed operations.
All non-cancelled paired runs must agree. An error/panic aborts the experiment,
cancels peers and joins them; poisoned-state or missing-owner errors cannot produce
a successful report. Tests inject these failures; OS thread/resource exhaustion
is propagated but not manufactured on the developer host.

The worker count, global credit ceiling, total channel slots, state-entry count,
trace, service work and idle choices match within each comparison. Mutex/condition
variable, partition-index, thread and queue metadata are extra allocations. A
credit covers queued, executing and terminal-but-uncollected work. Admission walks
the trace without skipping a full key-owned lane. Hot-key serialization and hot-lane
admission pressure are therefore visible, not hidden by rerouting. Both candidates
are unpinned and report that explicitly; ownership implies no NUMA locality.

The stateful report adds `sequence`, `committed_ns`, `final_state`, key/owner-lane
peaks, and per-worker `cpu_ns`. CPU values come from Windows thread kernel/user
accounting; zero values and coarse granularity are retained. `active_ns` includes
preparation, ordering waits and commit wall time, so it is not interchangeable with
CPU time. Aggregate, per-key and logical-owner-lane summaries retain scheduled
arrival as response origin; cancelled distributions remain separate. No response
time is invented for unadmitted requests. `service_and_order_wait` ends at commit
and does not isolate mutex cost from service or OS scheduling.

The bounded demonstration varies uniform/hot/all-hot keys, lookup/add mix, cheap
and uneven service, steady/burst arrivals, pressure and cancellation. Each point
runs shared/owned/owned/shared without selecting a fastest default:

```powershell
.\target\release\windows-request-reply-experiment.exe capture-stateful .scratch\stateful-retake.json
```

Build the same package in release mode first. The destination must not exist.
The CLI retains full trace/configuration and build provenance using the existing
output writer. [The stateful record](captures/2026-09-19-stateful/README.md) holds
the saved observations. The protocol and disposition are
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RR-D5` and `RR-D6`. Neither is a startup
benchmark, physical-NUMA measurement or a performance acceptance criterion.

## Ordered Ingestion Comparison

A third path compares a **serial owner**, which transforms and publishes each
record itself, with a **staged pipeline**, whose transform workers overlap behind
one publisher that resequences into declared order. It separates two constraints
that the stateful path fuses: a record's transform must precede its own
publication, and publications must occur in declared trace order.

Each record carries a unique ID, an absolute arrival offset, an input value,
bounded transform and publish work, and an optional injected failure naming the
stage that fails. An effect is visible only through the in-memory publication
log; nothing is written outside the report, so this path performs no real output
I/O and makes no durability claim.

Every admitted record reaches one terminal outcome: published with its reference
value, aborted at its injected stage, or cancelled. An aborted or cancelled
record still advances the publication cursor and contributes no log entry, so
published identities form a strictly increasing subsequence of the trace. The
publisher is authoritative: once it observes cancellation it cancels the cursor
record and every record after it, so outcomes are a published/aborted prefix and
an all-cancelled suffix.

The staged candidate's resequencing buffer is a window of `reorder_capacity`
positions starting at the cursor, so the record the publisher is waiting for can
always be staged and the pipeline cannot deadlock against its own bound. A
transform worker whose record falls outside the window blocks, and that blocking
is counted in `reorder_full_observations` rather than removed by reordering or by
an unbounded buffer. `published_ns - ready_ns` is head-of-line delay alone. The
serial candidate has no buffer and reports zero for both fields.

The serial candidate runs one owner thread and the staged candidate runs the
configured worker count plus a publisher, so thread counts differ by design and
are labelled rather than claimed equal. Request slots, reply slots, admitted
credits, per-record work and the idle policy are matched within a pair.

```powershell
.\target\release\windows-request-reply-experiment.exe capture-ingest .scratch\ingest-retake.json
```

[The ingestion record](captures/2026-09-19-ingest/README.md) holds the saved
observations. The protocol is [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RR-D7`.
This is unpinned in-memory ingestion, not a startup benchmark, physical-NUMA
measurement or a performance acceptance criterion.