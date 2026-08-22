# Design session -- fault protocol, doorbells, and backpressure (2026-08-21)

Decisions produced: **D-16 overturned and replaced**, **D-25 replaced**, and new
**D-27 ... D-31**. Held while M2.3 was complete and M2.4 was about to start, so
none of it required unwinding shipped code.

> Tier-3 record. [DESIGN-NOTES.md](../DESIGN-NOTES.md) is authoritative and wins
> on any conflict.

---

## How it started: a false invariant

D-2 said "no client code ever runs on a monitor/threadpool thread". The engineer
rejected the statement outright: the process thread pool is not this crate's, and
a client that drains the notification queue from its own `ThreadpoolWait`
callback is running client code on a pool thread -- legitimately, and by design,
since this crate's whole premise is that nobody should have to own threads.

The enforceable property is about the **call graph**, not threads: the crate
never invokes client code, so nothing the client does can stall or unwind our
cadence. Corrected in place; D-11's substance (reject a client-implemented
`NotificationSink`) survived, only its justification had been phrased through the
false property.

The correction immediately exposed a gap. If a client is expected to integrate
with its own thread pool, a queue drainable only by a blocking `recv()` forces it
to dedicate a thread -- contradicting the premise. Hence a doorbell.

## The doorbell, and three wrong turns

**First proposal: a `Doorbell` trait the client implements.** Argued on the
grounds that an event handle composes only with Win32 waits, while a method
composes with a semaphore, an IOCP post, or an async `Waker`.

**First correction -- dispatch.** `Arc<dyn Doorbell>` versus `<D: Doorbell>` was
raised; the generic was chosen, which also settled that the trait mandates no
allocation (a ZST implementor costs nothing, stored inline in the queue's
existing `Arc<Shared<D>>`).

**Second correction -- the trait was unnecessary.** The engineer asked whether a
doorbell needs to be a separate type at all, or should just be a method on the
queue. It should. On Windows the HANDLE *is* the universal waitable currency --
`WaitForSingleObject`, `WaitForMultipleObjects`, `MsgWaitForMultipleObjects`,
`ThreadpoolWait`, alertable waits all take one -- so an event is the native
composition point rather than a lowest-common-denominator compromise. The one
case it does not reach, an async `Waker`, is a ten-line bridge the client writes
**on its own pool**, which is exactly where it belongs; abstracting it into a
trait would have dragged the client's wake onto our cadence path, importing the
problem D-2 exists to prevent.

Owning the doorbell eliminated: the D-2 exception entirely, the generic virality
(`Monitor<D>`, `Session<D>`, `Sender<D>`), the panic contract, and the allocation
question -- and it made the reset discipline an internal invariant instead of a
client obligation.

**Third correction -- there are two doorbells, not one.** Framed as sq/cq, the
request queue needs a wake as much as the notification queue does. It already had
one and it had simply not been named: `ThreadpoolWork::submit()` *is* the SQ
doorbell. The mechanisms differ because the **waiter** differs -- on the CQ the
client owns its waiting strategy so we hand out a handle; on the SQ we are the
waiter, so queuing work to the pool is strictly better than an event plus
something to wait on it.

## Backpressure: the problem two rings does not solve

Raised next: queues are finite, so they fill, and the design must survive it. An
assistant proposal to split control and observation into two rings was rejected
with the decisive observation that **two rings gives you two rings that can both
fill** -- the problem is unchanged, and ring count is a taxonomy question, not an
answer.

The real constraint is that neither producer may be handled the obvious way:

- **Blocking is a deadlock, not backpressure.** A full ring that suspends its
  writer blocks a pool thread when that writer is an I/O completion. The client's
  drain may itself be running on a pool thread -- that is the doorbell
  integration we had just designed -- so the cadence can block pool threads
  waiting for a drain that needs one. Not slow: wedged.
- **Dropping is unavailable for control.** A dropped batch is recoverable (that
  is what `Desync` is for); a dropped completion is a liveness bug, because the
  client waits forever for something that already happened and will not be
  re-sent.

The resolution is to throttle each producer somewhere **other than at the
enqueue**:

- Control comes from request processing, so stop draining the SQ. Requests back
  up, and backpressure lands on the client's own `subscribe()` call, on the
  client's own thread. No pool thread blocks.
- Observation comes from the I/O completion, so **stop re-arming the read**.
  Nothing blocks, nothing is dropped, and backpressure propagates into the
  *kernel's* change buffer -- a grace period the design did not previously have.
  If the client drains in time, nothing is lost at all; if not, the kernel
  overflows, which is `Desync { Overflow }`, an already-specified signal with
  existing semantics.

Desyncs must therefore be **latches, not queued items**, generalising what D-11
already did for `QueueFull`: reporting "the ring filled" cannot itself require
ring space.

A pleasing consequence: `QueueFull` largely stops occurring. We stop reading
rather than overfill, so the loss that does happen is genuine kernel overflow --
the client learns "the OS dropped changes", which is true, rather than "the crate
dropped changes because you were slow", which was our choice.

## Overturning D-16

The engineer then produced the case the whole model had to serve: the read fails
because the directory was deleted, the security descriptor changed, or the remote
fileserver went away -- and the natural question is "given this error, how long
should I wait before trying again?" That is a queued event expecting a response.

D-16 had forbidden exactly this. Its two recorded objections:

1. *"The hook is in the queue."* A synchronous callback would run on the pool
   thread.
2. *"There must be no race."* A reactive answer arriving while the retry timer is
   already scheduled is, by construction, a race.

Objection 1 was written against a **synchronous callback** and does not touch a
queued exchange; the D-2 correction weakened it further. Objection 2 only holds
if a timer is *already scheduled* when the answer arrives -- and if the monitor
schedules nothing until answered, there is no timer to race. The race was an
artifact of wanting to keep retrying while asking.

**D-16 overturned.** Its replacement is per-watch, selected at registration:
defaults, or interactive.

The remaining hazard was that a fault report is control data generated **on the
cadence**, so neither throttle applies to it. Resolved the same way as desyncs: a
fault is **watcher state, not a queued item**. A watcher cannot be faulted twice
concurrently because it is not running, so the report is one error code plus one
bit, latched on the watcher, costing no ring space and unable to block. The
suspended-not-re-arming state then does double duty for backpressure and for
fault, which is a sign the model is coherent rather than accreting cases.

## What `m` actually did

The engineer noted the prior art should already have been gathered. It had not:
the repository recorded `m`'s *shape* ("directory probe" retry timer) but never
its **values**. Rather than invent plausible numbers and attribute them to `m`,
the public `Azure/m` repository was read directly.

From `src/libraries/filesystem/src/platforms/windows/directory_watcher.cpp`:

```cpp
m_default_retry_delay(500ms),
m_minimum_retry_delay(50ms),
```

and the reduction, which is precisely the rule the engineer had already specified
independently:

```cpp
std::chrono::milliseconds retry_duration = std::chrono::milliseconds::max();
for (auto&& rw: m_registered_watches)
{
    auto r  = rw.m_change_notification->on_directory_access_failure(issue_time, m_path, error);
    auto ms = r.value_or(requeue_directory_access_attempt{m_default_retry_delay}).m_milliseconds;
    if (ms < retry_duration)
        retry_duration = ms;
}
retry_duration = std::max(retry_duration, m_minimum_retry_delay);
```

Ask every registered watch, take the earliest, clamp to a floor, and treat a
declined answer as the default. The newer PIL implementation keeps 500 ms and
splits it into separate open-failure and arm-failure defaults, matching D-15's
reopen-retry / rearm-retry classification.

Two places where `m`'s documentation contradicts its code were found:

- The header says returning `std::nullopt` **cancels the watch**; the code does
  `value_or(default)` and retries at 500 ms.
- The header says the floor is *"typically 1000ms, not less than 500ms"*; the
  code sets 50 ms.

Ruling: **the code wins, the documentation and comments lag.** So a declined
answer means *use the default*, and the floor is 50 ms.

Incidental confirmations: `m`'s PIL notification buffer is `64 * 1024`, matching
the 64 KiB chosen independently in M2.2. Its notify filter is narrower than the
placeholder used in M2.2 -- `FILE_NAME | DIR_NAME | SIZE | LAST_WRITE` against
all seven -- which matters only until M4 unions per-subscription filters.

## Request completions

Separately established: no *notification* needs acknowledgement, but *requests*
need completions and nothing carried them. M3.5 already demanded one without
saying so ("assert no delivery after cancellation completes"), which is
unprovable if the client cannot observe that its cancellation was processed.

An assistant suggestion to give completions only to requests "whose contract
needs it" was withdrawn: it rested on the claim that subscribe cannot permanently
fail, which D-22 had already contradicted three commits earlier. Subscribing to a
regular file, or to a path containing an interior NUL, is permanent -- so
fire-and-forget subscribe means the client never learns the watch will never
fire. All requests get completions.

---

## Addendum: the reservation discipline (D-33)

Recorded after the main session, correcting an over-generalisation in D-28.

The engineer supplied the formulation the whole backpressure discussion had been
circling: **control-type messages submitted on the SQ must have pre-allocated
their CQ messages**, so reliable delivery of the completion is guaranteed by
construction.

This is stronger than what had been recorded. D-29 originally had the monitor
*check* whether the notification queue could accept a completion before draining
a request. Reservation removes the check entirely: the slot is already the
sender's, so delivery cannot fail, and backpressure lands on the client's own
thread at submit time rather than being discovered later somewhere it cannot be
handled. It is the discipline `io_uring` follows when sizing its completion ring
against its submission ring.

The engineer was then explicit about the boundary: **file change notifications
were deliberately *not* given guaranteed delivery.** That is what makes the two
tiers principled rather than ad hoc. Reliability is a property of *reserved
capacity*, not of message type -- one line, *reserved is guaranteed, unreserved
is best-effort*, rather than a per-message-type table. And it is justified by
what the two carry: a lost batch is re-derivable by re-scanning, which is exactly
what `Desync` says, whereas a lost completion is a liveness bug.

### What this corrected

D-28 as first written made **every** `Desync` a latch. That silently contradicted
D-12 and D-26, which promise that a client seeing a `Desync` knows exactly which
changes preceded it -- an out-of-band latch destroys precisely that ordering. The
mistake was generalising from the one case that genuinely cannot use a slot
(`QueueFull`, where saying "the queue is full" cannot itself require a slot) to
every case.

Under D-33 the tiers separate cleanly:

- **Faults** are control, so they take a standing reservation at registration --
  one slot suffices, since D-28's own observation holds that a watcher cannot be
  faulted twice concurrently. They are ordinary queued items, in order.
- **Desyncs** are observation, so they ride the queue in order like any other
  notification, and the latch is the **fallback** used only when the observation
  tier cannot enqueue. At that point ordering is already compromised by the loss
  the latch is reporting, so nothing survives to be given up.

### The residual drop path

Because observation holds no reservation, a batch can still arrive to a full ring
even with the arm-time throttle: a control reservation may have taken the room
since the read was armed. That batch is dropped and the loss reported by the
latch. It is the one path where a notification is discarded, and it is now a
named consequence of the tiering rather than an unexamined default.
