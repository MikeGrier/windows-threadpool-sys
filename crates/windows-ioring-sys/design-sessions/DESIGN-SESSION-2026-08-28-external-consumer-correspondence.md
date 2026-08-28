# Design session: correspondence with an external consumer

**2026-08-28**

Anonymized record of a four-round exchange with an external team evaluating this crate as the
memory-safe ring layer beneath a storage backend. Company, product, and workload specifics are
removed; the technical substance is preserved because it is what produced
[D-19](../DESIGN-NOTES.md#d-19) through [D-27](../DESIGN-NOTES.md#d-27).

The consumer is referred to throughout as "the consumer". Where their design is described, it is
described only to the extent needed to make our reasoning legible.

Companion documents:
[DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md](DESIGN-SESSION-2026-08-28-completion-event-multiplexing.md)
records the decisions from rounds 1-2 in narrative form; this file records the correspondence
itself, including the rounds that followed. The two programs that settled the measured questions are
kept in [spikes/](spikes/README.md) and can be re-run on other hardware.

---

## Round 1 -- inbound: "a third delivery shape"

The consumer's opening proposal. Its structure is worth preserving as a model for how to receive a
report like this: it named three specific source facts and a claim about what they *jointly*
prevent, rather than describing an inconvenience.

The three facts, all verified:

- `SetIoRingCompletionEvent` is called in exactly one place, inside `EventDelivery::new`.
- `EventDelivery::new` takes the ring **by value** and hands the event to `ThreadpoolWait`.
- `IoRing::raw_handle()` is `pub(crate)`.

The claim: the completion event is reachable only by a caller willing to surrender the ring, so a
caller that must wait on the ring *and* on something else simultaneously has no path. Their
motivating case was ordering ring writes against operations the ring's fixed op table cannot
express, which must therefore be issued through a separate path and ordered in host code --
requiring a wait over both completion sources at once.

They asked for two things: a method to attach a caller-held completion event, and for
`windows-threadpool-sys` to become an optional dependency.

## Round 1 -- outbound: accepted, with a different signature

We accepted the diagnosis and reframed it. **This is not a third delivery architecture; it is Model
B with a multiplexed wakeup source.** [D-3](../DESIGN-NOTES.md#d-3) fixed each architecture's
*wakeup mechanism* as part of its identity, which coupled reaching the completion event to
surrendering the ring. Ownership, submission, and draining are unchanged; only what the thread
blocks on differs.

We rejected their proposed signature as unsound:

```rust
pub fn set_completion_event(&mut self, event: BorrowedHandle<'_>) -> io::Result<()>;
```

The borrow ends at return but the kernel retains the handle for the ring's life, so dropping the
event leaves the kernel signalling a stale handle value -- a use-after-free reachable from safe
code. The counter-proposal inverts ownership: the ring creates and owns the event and returns an
`OwnedHandle` duplicate ([D-20](../DESIGN-NOTES.md#d-20)).

Before replying we spiked `SetIoRingCompletionEvent` directly, because `EventDelivery` only ever
called it on a fresh ring and therefore exercised almost none of its behaviour. That spike found
[D-19](../DESIGN-NOTES.md#d-19) -- the completion event is edge-triggered on the completion queue
going empty to non-empty -- and, through it, a live bug in our own shipped `EventDelivery`.

We also declined their stated rationale for the feature gate while accepting the request: they
argued a runtime "correctness-of-posture" cost from linking an unused thread pool, which is false,
because the Win32 default pool is a process-wide facility instantiated lazily on first use. Worth
correcting rather than accepting quietly, since they might have blocked an evaluation on a belief
about our runtime behaviour that was not true.

## Round 2 -- inbound: accepted, two retractions

They accepted the counter-proposal, confirmed a single waiter per ring, and confirmed a plain
`WaitForMultipleObjects` with no alertable wait or message pump.

Two retractions, both volunteered:

- **Their signature was unsound and they said so.** They had argued against exposing the raw handle
  on the grounds that it pushes `unsafe` onto consumers, then proposed a safe-looking method with a
  use-after-free in it.
- **The threadpool rationale was wrong.** They had asserted a runtime cost without verifying it.

The most valuable thing in their reply was an observation about testability: they doubted they
would have caught the edge-trigger bug in an integration test either, because the failure needs a
*second wakeup source* to become visible, and that configuration does not exist in this crate
today. That is why M11 adds a multiplexed-wait test deliberately rather than for completeness.

They also flagged, unprompted, that their own notes aspire to less serialization than they
currently have, and that acting on that would reopen the single-waiter question. Recorded in
[D-21](../DESIGN-NOTES.md#d-21) so a later multi-waiter request is recognised as a design change
rather than a flag.

## Round 3 -- the durability problem

The consumer had designed a **synthetic ring**: a queue in front of the real ring, holding
operations back so that durability-marked writes and flushes could be ordered against each other in
host code. They believed the ring could not express durability.

**The premise was false, and this crate caused the misunderstanding.** The kernel exposes
`FILE_WRITE_FLAGS` on writes and `FILE_FLUSH_MODE` on flushes; this crate hardcoded both. A
consumer reading our API saw ordering but no way to express durability at all, and building a
synthetic ring is a reasonable response to that API. Recorded as
[D-25](../DESIGN-NOTES.md#d-25).

Three corrections followed, in order of increasing importance:

1. **The synthetic ring was the root of their concurrency problem, not a separate issue.** To
   enforce "flush after these writes" it must *observe* every preceding completion, which requires
   a thread waiting on completions, which is why they held a lock across a blocking wait, which is
   why submitters queued behind the waiter. A barrier flag does the same ordering inside the kernel
   with no host involvement.

2. **We conflated write-through with FUA, and they corrected us.** Write-through is a first-level
   cache directive; FUA is a device-level durability guarantee. The correction invalidated a
   recommendation we had just made -- that commit records use write-through to be durable without a
   full flush -- which would have been a data-loss bug. Retracted. See "Durability on the ring" in
   [DESIGN-NOTES.md](../DESIGN-NOTES.md).

3. **FUA is not an ordering primitive either.** They expected SCSI's FUA bit to form a barrier
   against reordering. It does not: ordering in SCSI comes from the task attribute (`ORDERED`), not
   from FUA, and NVMe has no inter-command ordering guarantee at all. If any part of a
   crash-consistency argument reads "FUA on this write orders it after that write", that is a bug
   rather than a tuning question.

The resolution is that FUA bundles two things, one of which it never delivered, and the ring
separates them: durability from the flush operation, ordering from the barrier flag, asynchrony
from both being ordinary SQEs. **Write plus drained flush is the FUA emulation path**, written
explicitly.

## Round 3 -- the architecture discussion

A side thread, prompted by asking whether kernels affine rings to threads the way userspace does.
They do not -- they affine to CPUs -- and per-thread is userspace's proxy for per-CPU because it has
no other durable ownership unit. Recorded as [D-27](../DESIGN-NOTES.md#d-27), together with the
observation that Model A and Model B are Windows' own two completion mechanisms (a special kernel
APC to the originating thread; a packet posted to a completion port) rather than anything this
crate invented.

## Round 4 -- measurement

Two spikes were run rather than reasoning further, after the write-through error made clear that
recalled semantics were not reliable here.

The drain spike needed **three iterations to become valid**, which is itself worth recording:

1. Buffered writes -- could not discriminate. Every completion arrived in submission order even
   with no ordering flag, because buffered writes land in the page cache and finish in issue order.
2. `NO_BUFFERING`, but *extending* writes -- still could not discriminate, because the filesystem
   serializes extending writes and writes past the valid-data length.
3. `NO_BUFFERING` over a **pre-written extent** -- 28 of 32 small writes overtook large ones with no
   flags at all, establishing the baseline that made every later result meaningful.

A control that shows the same result as the treatment means the harness is measuring nothing. Two
of the three iterations here would have produced confidently wrong conclusions.

The findings became [D-23](../DESIGN-NOTES.md#d-23) (an unflagged flush does not cover preceding
writes -- observed at 17 and 23 of 32 writes completing after it) and
[D-24](../DESIGN-NOTES.md#d-24) (the barrier is a full, ring-wide stall that spans submissions and
holds operations against unrelated files).

## Round 4 -- what belongs where

The closing question was whether this crate should provide the emulation a consumer needs on top of
these primitives. Answered as [D-26](../DESIGN-NOTES.md#d-26): Windows mechanism here, durability
policy with the consumer, and a worked example as the vehicle that carries the knowledge across
without this crate owning the policy. Queued as M13/M14.

---

## What to take from this exchange

**Three defects, all the same shape.** The completion event, the write flags, and the flush mode
were each reachable in the kernel and hidden by this crate, because the crate's own examples did not
need them. That is the PLATFORM INTEGRITY failure mode -- a platform narrowed to its visible
consumer -- and finding three instances in one review cycle means the mechanism that produced them
is still in place. Worth checking the remaining surface deliberately rather than waiting for a
fourth consumer to find it.

**Measurement beat recall three times.** The edge trigger, the unflagged flush, and the full-barrier
semantics were all contrary to a reasonable expectation, and one confidently-recalled answer
(write-through as FUA) was simply wrong. Every one of these is undocumented behaviour that a
consumer would otherwise encode by inference.

**The most useful thing the consumer did was structure their report as evidence.** Named source
facts plus a claim about what they jointly prevent, which made the diagnosis verifiable in minutes
instead of a negotiation about whether the problem was real. Two of their three arguments turned out
to be wrong; the proposal was right anyway.
