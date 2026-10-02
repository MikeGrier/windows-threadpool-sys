# What a realizer would need from this crate -- the M27.1 census

2026-10-01. The output of `M27.1`: a **gap list, not an API**. Nothing here proposes surface,
because the plan vocabulary is not settled and binding to a draft is the failure this item was
written to avoid.

## What was walked

A plan states four things about a domain ([EP-D-5](../topology-planner/DESIGN-NOTES.md#ep-d-5)):
which processor it pins to, which memory node its pool allocates from, how many queues of which
types, and where each channel's buffer lives. Each is walked to the public API that would realize
it, against the crate as it stands.

## 1. Which processor a domain pins to -- **GAP, and not this crate's**

**This crate has no thread.** An `IoRing` is a kernel object plus bookkeeping; the only execution it
causes is completion delivery, and that runs on a thread pool reached through one seam:

```text
EventDelivery::new(ring, on_completion, env: Option<&mut CallbackEnviron<'_>>)
```

So "pin this domain to processor P" has to be expressible on that `CallbackEnviron`, or on the pool
behind it. Measured, it is expressible on neither:

| type | entire public surface |
|---|---|
| `ThreadpoolPool` | `new`, `set_min_threads`, `set_max_threads` |
| `CallbackEnviron` | `new`, `set_pool`, `clear_pool`, `set_priority`, `set_runs_long` |

A realizer can therefore give a domain its **own pool** with a bounded thread count, which is a real
isolation lever and is already reachable. It cannot say which processor that pool's threads run on.

**The gap is in [windows-threadpool-sys](../windows-threadpool-sys/CHECKLIST.md), not here**, and it
should be closed there rather than worked around by this crate growing a thread of its own. Recorded
without proposing the shape, because what a pool-affinity surface should look like is that crate's
question and is bound up with Win32 offering no pool-affinity call -- a realizer's alternatives are
per-callback `SetThreadAffinityMask` or a dedicated thread outside the pool, and choosing between
those is a design decision this census does not take.

## 2. Which memory node the pool allocates from -- **half covered, half not closable here**

**Caller-supplied buffers: covered.** `NumaBuffer::new(len, node: Option<NumaNode>)` is re-exported
from this crate, and [numa_buffer_io.rs](src/numa_buffer_io.rs) implements `IoBuf` and `IoBufMut`
for it. A node-bound allocation is therefore pushed, registered and returned exactly like a
`Vec<u8>`; no new surface is needed, and a plan that says "this channel's buffers come from node N"
is realizable today.

`M27.1` called `NumaBuffer` "one half of the pool answer". This census narrows that: it is the
*whole* of the caller-buffer answer, and none of the ring-memory answer.

**The ring's own submission and completion queues: not closable here.** `CreateIoRing` takes a
version, the two queue sizes, and a `IORING_CREATE_FLAGS` pair of required/advisory words -- and no
placement parameter of any kind. The kernel allocates that memory. This is a **platform limit, not
a gap in this crate**, and the distinction matters: a gap is something to close, and this is
something to state.

Whether that memory's placement is worth caring about is unmeasured and is not asserted either way
here.

## 3. How many queues of which types -- **covered**

- **Count.** A realizer constructs as many `IoRing`s as the plan names; nothing in the crate limits
  or couples them.
- **Depth.** `IoRing::new(submission_queue_size, completion_queue_size)`, with
  `with_version`, `with_inventory` and `with_version_and_inventory` as the other constructors.
- **Type.** `with_version` pins the `RingVersion` a ring is built against; `capabilities()` and
  `IoRing::supports` report what the host actually offers, so a realizer can refuse a plan the
  machine cannot satisfy rather than discovering it mid-run.

What this crate does *not* hold is which ring serves which domain. That is the realizer's own
bookkeeping, not a capability it would ask this crate for, so it is not recorded as a gap.

## 4. Where each channel's buffer lives -- **covered**

The same answer as the first half of (2), and for the same reason: a buffer's address is chosen by
whoever allocates it, and this crate accepts any `IoBuf`.

## The summary a reader wants

| plan fact | status | owner |
|---|---|---|
| processor a domain pins to | **gap** | `windows-threadpool-sys` |
| memory node -- caller buffers | covered | -- |
| memory node -- ring's own queues | platform limit | Win32 |
| how many queues, how deep, which version | covered | -- |
| where a channel's buffer lives | covered | -- |

**One gap, and it is not in this crate.** That is the census's actual finding, and it was not the
expected one: `M27.1` was written expecting to find this crate short, and what it is short of is a
thread-placement expression that belongs a layer down.

## What this census deliberately does not do

It does not propose API. It does not rank the gap against others, or claim the ring-memory limit
matters. And it is taken against the plan vocabulary *as described in `M27.1`* -- the four facts
above -- rather than against a settled type, because there is not one yet.
