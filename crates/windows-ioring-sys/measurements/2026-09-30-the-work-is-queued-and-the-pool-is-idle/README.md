# The work is queued, and the pool sits idle beside it

2026-09-30. `M-T5.1`. Fifteen stalls captured out of 8400 processes, on the
`hand-spin-3us` arm. Counts in [captures.csv](captures.csv); one full trace in
[stalled.txt](stalled.txt).

## The question

A worker factory creates a thread when its completion port has work outstanding.
So the port's queue depth during a stall decides between two readings that
nothing measured so far could separate, and which send the investigation in
opposite directions:

- **depth above zero** -- the work is queued and undelivered while the factory
  reports itself willing and idle. The factory was entitled to make a thread and
  did not. The question becomes *what should have asked it*.
- **depth of zero** -- the work is not there. The factory is behaving correctly
  on the information it has, and the fault is earlier, in delivery.

## The answer

**Depth 2, in 15 captures of 15**, on the pool under test. Paired with that
pool's own counters, read in the same instant:

| quantity | value, all 15 |
|---|---|
| completion port depth | **2** |
| total workers | **0** |
| may create | **1** |
| create in progress | **0** |

Two packets is exactly right: the trigger's own wait is torn down before the
stall, and the two victims each arm one. So the work that never arrives is
sitting in the port, and the pool that would run it has no threads, is permitted
to make one, and is not making one.

The second factory in the process (the private pool the reproducer's control arms
create) reports depth 0 throughout, which is the expected reading for a factory
with nothing to do and is recorded as the within-capture contrast.

## What this settles

The fault is **not** in delivery. The packet reaches the port. Every reading that
located the fault upstream of the factory is excluded.

It also closes off the benign explanation of the counters. `may_create` being 1
with `create_in_progress` 0 and no failed creation had been consistent with "the
factory correctly sees no work"; it is not, because the work is there.

## What it does not settle

**Why nobody asks.** The factory's create test would approve -- the port is
non-empty -- so the stall is not a decision to decline. It is the absence of the
question. What normally prompts it after work is queued, and why that prompt does
not arrive here, is `M-T5.2`.

Note also what a depth of 2 rules out on the *timing* side: the packets are still
queued five seconds after they were posted, so nothing dequeued and discarded
them. This is a pool that never woke, not one that woke and mislaid the work.

## Method, and why the ordering is the measurement

Both reads happen in the post-mortem, and **both happen before the liveness
probe**. That ordering is not tidiness: the liveness probe submits a work item,
and submitting work is measured to release this stall every time. Reading
afterwards would describe a pool that the question had already repaired.

The depth is read with `NtQueryIoCompletion`, which reports it **without
dequeuing**, so observing cannot consume the packet whose presence is the whole
question -- a hazard this investigation has already been caught by more than once.

Ports are found by asking each candidate handle whether it is one, the same way
the factory scan works and for the same reason: the default pool exposes no way
to reach its port, and a stalled process is precisely one where no hooked call has
carried a handle.

**The instrument has a positive control, and the control is sabotage-verified.**
`the_port_scan_finds_a_completion_port_and_reads_its_depth` posts a known number
of packets to a port it makes itself and requires the scan to report that number
back. Both halves were sabotaged and both went red: a scan that finds nothing
fails the first assertion, and a scan that finds ports but misreads the depth
fails the second. This matters more than usual here, because a silently broken
probe reports "no ports" or "depth 0" -- which is exactly the finding that would
have sent the investigation the other way.

Measured against ntoskrnl 10.0.26100.9457, after the update that superseded the
9444 build the earlier analysis was read from. The worker-factory surface is
unchanged across that update; the reproducer's rate is unchanged.
