# Design session: the completion event as a ring primitive

**2026-08-28**

Decisions produced: [D-19](../DESIGN-NOTES.md#d-19) (the completion event is edge-triggered),
[D-20](../DESIGN-NOTES.md#d-20) (`IoRing::completion_event`), [D-21](../DESIGN-NOTES.md#d-21)
(auto-reset, single waiter), [D-22](../DESIGN-NOTES.md#d-22) (optional `threadpool` feature).
Amends [D-3](../DESIGN-NOTES.md#d-3). Work queued as M11 in [CHECKLIST.md](../CHECKLIST.md).

## How it started

An external consumer -- a storage substrate building a Windows backend over `IoRing` -- sent a
proposal against published 0.1.2 titled "A third delivery shape". Their claim: the crate pairs
*who owns the ring* with *whether the completion event is reachable*, and the combination they
need (caller owns the ring, caller waits on the event) is unreachable. They named the three
source facts that close it off:

- `SetIoRingCompletionEvent` is called in exactly one place, inside `EventDelivery::new`;
- `EventDelivery::new` takes the ring **by value** and hands the event to `ThreadpoolWait`;
- `IoRing::raw_handle()` is `pub(crate)`.

All three verified. Their motivating case is real and is one this crate's own scope statement
already concedes: they issue `FSCTL_SET_ZERO_DATA` / `FSCTL_FILE_LEVEL_TRIM` through
`DeviceIoControl` because the op table has no ioctl, and must order those against ring writes.
`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` cannot express that ordering, so it has to be enforced in host
code -- and doing so without blocking requires waiting on both completion sources at once.

## The reframing

It is not a third architecture. It is **Model B with a multiplexed wakeup source.**

D-3's "Model B is a pinned thread parked directly in `SubmitIoRing`; no event, no wait object"
is true only while the ring is the *only* thing that thread waits on. Add non-ring I/O, a
shutdown signal, or a timer, and the fused park is unusable -- the thread needs a wait it can
compose. Ownership, submission, and draining are unchanged; only what the thread blocks on
differs.

So the defect was narrower than "a missing model" and more interesting: D-3 fixed each
architecture's *wakeup mechanism* as part of its identity, and that coupled reaching the
completion event to surrendering the ring. The consumer read the coupling as deliberate. It was
not. The completion event is a primitive of the ring; it had become a private implementation
detail of one delivery object. This is the PLATFORM-INTEGRITY failure mode viewed from inside:
a platform narrowed to the shape of its currently-visible consumer.

## Their proposed signature, and why it could not ship

```rust
pub fn set_completion_event(&mut self, event: BorrowedHandle<'_>) -> io::Result<()>;
```

The borrow ends at return; the kernel retains the handle for the ring's life. Drop the event and
the kernel signals a stale handle value, which after handle reuse names an arbitrary other object
in the process -- a use-after-free reachable from safe code. `unsafe` would close it but reimports
exactly the cost they used to argue against promoting `raw_handle()`. They accepted this
immediately and noted the inconsistency in their own proposal without prompting.

The counter-proposal inverts ownership: the ring creates and owns the event and returns an
`OwnedHandle` duplicate. No lifetime hazard, no `unsafe`, capability check stays inside, "exactly
one completion event per ring" becomes structural rather than conventional, and `EventDelivery`
becomes a consumer of it rather than the only route to it. Returning an owned duplicate rather
than a `BorrowedHandle<'_>` also avoids borrowing the ring across a wait, which would contend with
the same thread's submissions -- a point that turned out to matter more for them than for us,
since their backend serializes ring state behind an interior lock.

## The spike, and what it found

Before committing to a signature we measured `SetIoRingCompletionEvent` directly against Win32
(version 400, real kernel ring, no `UM_EMULATION`), because `EventDelivery` only ever calls it on
a fresh ring and therefore exercised almost none of it.

The permission questions all came back favourably: it may be called at any time including with
ops in flight; a second call replaces; `NULL` clears and leaves the ring usable; a duplicated
handle still signals after the original is closed.

Then an unplanned case -- setting the event while operations were outstanding -- produced a
result that made no sense under the assumed model: the event did not signal, yet four completions
were drainable. Isolating it gave [D-19](../DESIGN-NOTES.md#d-19): **the event is edge-triggered
on the CQ going empty -> non-empty.** A completion into an empty queue signals; into a non-empty
queue it does not; drain to empty and the next one signals again.

This mattered more than the API question. The consumer's planned loop would have drained the ring
only on the pass where the ring's handle signalled, and drained "what is there" rather than to
empty -- which under an edge trigger stalls permanently the first time the wait returns for an
overlapped completion. For them that is the *common* case, not an edge, since an FSCTL completing
is a routine reason for that wait to return.

### It was already a bug here

`EventDelivery::new` attaches and arms with no initial drain, and its rustdoc claims delivery
covers completions "already queued when `ring` was handed over". Under the edge trigger that is
false: attaching to a non-empty CQ strands the backlog permanently, because nothing drains the
queue back to empty and no later completion can signal. A repro against the shipped crate
confirmed it -- submit a read, let it land, hand the ring over, and the callback never runs.

The existing M4 test hands over a *fresh* ring, which is why it passed. The consumer made the
sharper observation: they did not believe they would have caught it in an integration test
either, because the failure needs a second wakeup source to become visible, and that
configuration does not exist in the crate today. M11.3 adds it deliberately for that reason.

The fix generalises into the new API rather than sitting beside it: `completion_event` signals
the event once before returning, which costs one spurious wakeup at setup and removes the whole
attach-time lost-wakeup class for every consumer. That is the clearest argument against the
`pub raw_handle()` alternative -- a consumer wiring the raw call themselves gets no capability
check, no lifetime guarantee, *and* none of this.

## Auto-reset and the single waiter

[D-21](../DESIGN-NOTES.md#d-21) is forced by D-19 rather than chosen. Manual-reset leaves a stale
signal after the drain and spins the waiter; two waiters cannot be made correct, because the drain
that restores the empty state -- and so re-arms the edge -- must run to empty exactly once.

They confirmed a single waiter: their ring trait is `Send + Sync` with `&self` methods, but the
backend serializes all ring state behind an interior lock held across the blocking wait, so
different threads may be the waiter on different calls, never simultaneously. They volunteered a
caveat worth keeping: their own notes aspire to "shared directly across threads/cores without an
external per-queue lock", and acting on that would reopen the question. They explicitly did not
ask us to accommodate it. Recorded in D-21 so that a future multi-waiter request is recognised as
a genuine design change rather than a flag.

## The feature gate, and a rationale we declined to accept

They asked for `windows-threadpool-sys` to become optional, arguing that linking an unused thread
pool is a "correctness-of-posture" problem for a codebase under a no-implicit-threads discipline.

That is false, and we said so: linking the crate creates no threads, because the Win32 default
pool is a process-wide facility instantiated lazily on first use. Worth correcting rather than
accepting quietly, because they might have blocked an evaluation on a belief about our runtime
behaviour that was not true. They withdrew the argument and confirmed they are not blocked.

The gate ships anyway, on layering grounds alone ([D-22](../DESIGN-NOTES.md#d-22)). The cost
neither side raised initially is that CI must cover both feature combinations.

## Reflection

Two things are worth remembering from this exchange.

**The bug was found by measuring a dependency's behaviour instead of assuming it.** The spike was
run to answer a narrow signature question -- may this be called with ops in flight? -- and the
finding that mattered was in a case nobody had thought to ask about. D-2 and D-6 were both
established by spike for the same reason; this is now the third time measurement has overturned a
reasonable assumption in this crate.

**The consumer's report was more valuable than a bug report and cheaper to act on, because it
named its evidence.** Three specific source facts, each individually reasonable, and a claim about
what they jointly prevent. That structure is what made the diagnosis verifiable in minutes rather
than a negotiation about whether the problem was real.
