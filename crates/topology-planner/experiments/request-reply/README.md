# Request/reply experiment

Unpublished offline experiment for parent [CHECKLIST.md](../../CHECKLIST.md) -> `EP-X2.2`.
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