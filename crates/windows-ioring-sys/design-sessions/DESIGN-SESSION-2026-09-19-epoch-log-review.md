# Design session 2026-09-19: review of the epoch-log sample and its durability surface

A read-only review of [examples/epoch_log](../examples/epoch_log) and the parts of the crate it
composes, prompted by two questions from the engineer: whether the sample is correct and efficient,
and whether the repository's accumulated learnings about ring structuring and storage affinity
suggest reworking it.

**Nothing was built, run, or measured during this session.** Every finding below is from reading the
source and the recorded decisions. Where a finding rests on reasoning rather than on a measurement,
it says so. No claim here is a compile claim.

## What resulted

New work items [M21](../CHECKLIST.md), [M22](../CHECKLIST.md) and [M23](../CHECKLIST.md) in
[CHECKLIST.md](../CHECKLIST.md), plus an addendum to the already-queued
[M20.6](../CHECKLIST.md). No decision in [DESIGN-NOTES.md](../DESIGN-NOTES.md) was changed by this
session; two of the findings are about decisions whose corrections are queued and not yet landed
(see "Already queued, still undone" below).

## Scope and method

Read in full or in relevant part:

- every module of [examples/epoch_log](../examples/epoch_log);
- [src/batch.rs](../src/batch.rs)'s submission and flush surface, [src/ring.rs](../src/ring.rs)'s
  pop and test helpers, [src/lib.rs](../src/lib.rs)'s durability and topology guidance;
- [DESIGN-NOTES.md](../DESIGN-NOTES.md) decisions D-3, D-5, D-8, D-19, D-21, D-23, D-24, D-47, the
  "Why the NUMA node is the wrong key" and "What is not reachable" sections;
- [CHECKLIST.md](../CHECKLIST.md) M20, and the session it was queued from,
  [DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](../../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md);
- [design-sessions/spikes](spikes) -- the two unrun instruments and their README;
- [examples/ring_copy](../examples/ring_copy) for comparison, since it is the crate's other sample
  and the one that does make a locality decision.

## Findings: correctness

### C-1. A withdrawn D-24 claim survives at one site

[examples/epoch_log/commit.rs](../examples/epoch_log/commit.rs) line 156 justifies its epoch-order
`debug_assert` with "D-24 holds an operation pushed after a drained one until it completes". That is
the half of [D-24](../DESIGN-NOTES.md#d-24) that [D-47](../DESIGN-NOTES.md#d-47) withdrew, and the
same file's own module header (line 24) already carries the correction.

A blast-radius sweep of the hold-back phrasing across the crate found 17 matches in 10 files; every
other site is corrected. This is the last one.

The assertion it guards is still sound, by a different route: commit *N+1* carries the drain flag
itself, and D-47's *surviving* half ("not once did an operation queued before a drained flush
complete after it") is what orders it behind commit *N*. So the conclusion holds and the cited
reason does not -- a correction that did not propagate, rather than a wrong conclusion.

### C-2. A bare `loop { try_pop }` in the flagship example

[examples/epoch_log/append.rs](../examples/epoch_log/append.rs) line 89 spins unbounded after
`submit_and_wait(1, 30_000)`. [`Batch::submit_and_wait`](../src/batch.rs) documents that returning
does not mean a completion is poppable, because the timeout can expire first, and
[`pop_within`](../src/ring.rs) states the consequence outright: "A bare `loop` around `try_pop` is
worse, because it converts that flake into a hang."

The same step is written three ways in this crate:

| Site | Shape |
|---|---|
| [examples/ring_copy/engine.rs](../examples/ring_copy/engine.rs) line 147 | absence is an error (`TimedOut`) |
| [examples/epoch_log/strategy.rs](../examples/epoch_log/strategy.rs) `Lane::new` | absence is an error |
| [src/ring.rs](../src/ring.rs) `pop_within` | bounded wait, panics on deadline |
| [examples/epoch_log/append.rs](../examples/epoch_log/append.rs) line 89 | unbounded hot spin |
| [tests/fault_injection.rs](../tests/fault_injection.rs) line 50 | unbounded hot spin |

`pop_within` is `#[cfg(test)] pub(crate)`, so neither an example nor an integration test can reach
it -- examples and `tests/` are separate crates. That is why the duplication exists, and it means
the fix is an API question rather than a copy-paste: publish a bounded pop, or keep re-deriving it.

The rule is written down in three places and enforced nowhere, which is the detection-ladder point:
prose is not a rung.

### C-3. The commit trigger keys off the counter, not off the append

[examples/epoch_log/main.rs](../examples/epoch_log/main.rs) line 279 tests
`appended % EPOCH_SIZE == 0` on every pass of the append loop, including a pass where `append`
returned `WouldBlock` and `appended` did not move. On such a pass it commits again: a second
covering flush closing an epoch with nothing in it, and an epoch number consumed for no records.

Not reachable at the sample's current constants -- `SLOTS` is 8, `EPOCH_SIZE` is 6, and the commit
wait drains the arena, so the arena cannot be full at a boundary. It is armed by anyone who copies
the sample and raises `EPOCH_SIZE`, which is what the sample exists to be.

> **Corrected 2026-09-21 while implementing `M21.3`: the second paragraph is wrong.** The retry is
> not reachable at *any* constants, because the predicate is true at exactly two moments -- before
> the first append, and immediately after a commit -- and the arena is empty at both, the commit
> having waited for a covering flush that retires every outstanding write. Measured rather than
> re-reasoned: the retry path was instrumented to report when the old shape would have committed,
> and it fired **zero** times at `EPOCH_SIZE` of 6, 8, 12, 16 and 24, including the values past
> `SLOTS` this finding predicted would arm it.
>
> What survives is the coupling complaint in the heading, and it is worth the change on its own: the
> trigger was safe because of an invariant three blocks away that nothing stated, rather than
> because of where it was written. The lesson for this review is narrower and sharper -- "unreachable
> today, armed tomorrow" is a claim about a program's reachable states, and reading the code is not
> how to settle one.

### C-4. `durable_through` across a failed commit is under-specified

[examples/epoch_log/commit.rs](../examples/epoch_log/commit.rs) says "A failed commit advances
nothing", which reads as though a failed commit of epoch *N* leaves *N* non-durable permanently.

It does not. Epoch *N*'s writes precede commit *N+1*'s covering flush, so a later successful commit
makes *N* genuinely durable, and the monotonic reading of `durable_through` stays true. That is the
correct behaviour; the reasoning appears nowhere, so a reader auditing monotonicity after a failure
has to re-derive it. This is a specification gap, not a defect.

### C-5. Two wait loops hang where their sibling fails

[examples/epoch_log/strategy.rs](../examples/epoch_log/strategy.rs) lines 372 and 384 discard the
`submit_and_wait` timeout and loop forever.
[`EventLoop::pump`](../examples/epoch_log/event_loop.rs) raises `TimedOut` on the same condition and
documents why: "so a stuck loop fails instead of spinning". One program, opposite policies.

## Findings: efficiency

### E-1. One `SubmitIoRing` per record

Both [`Appender::append`](../examples/epoch_log/append.rs) and
[`Lane::append`](../examples/epoch_log/strategy.rs) construct a `Batch`, push one write, and submit
it. `Batch` exists to amortise submission across many SQEs; the sample that teaches `Batch` submits
one entry at a time.

This is not only a throughput observation. It puts a fixed per-record submission cost into all three
strategies in [strategy.rs](../examples/epoch_log/strategy.rs), which is a shared term in the
comparison whose headline result is that the three are indistinguishable. Whether batching moves
that spread is unmeasured; it is the cheapest experiment available, and it bears on
[M20.6](../CHECKLIST.md).

### E-2. Two implementations of the free-slot pool, in one program

[`Appender::free_slot`](../examples/epoch_log/append.rs) line 132 scans the arena calling
`outstanding()` per slot; `Lane` keeps a `Vec<u32>` free list. Both are correct and the difference
does not matter at eight slots. The duplication is what matters, because the two can drift.

### E-3. The arena has no placement story

[examples/epoch_log/append.rs](../examples/epoch_log/append.rs) line 84 allocates the registered
arena as `vec![0_u8; SLOT_LEN]` -- heap, no alignment, no node. The crate's own front page
([src/lib.rs](../src/lib.rs)) says buffer placement "is very likely the highest-leverage locality
decision available" and names `VirtualAllocExNuma`, and
[examples/ring_copy/buffer.rs](../examples/ring_copy/buffer.rs) already implements exactly that.

The durability sample has no locality story at all: no pinning, no node-local arena, one ring. That
may be the right call for a sample about durability -- but it is currently a silence rather than a
stated choice, while the crate's headline guidance says the opposite.

## Findings: ring structuring and storage affinity

### S-1. D-47 left one cost standing that the sample never names

[D-47](../DESIGN-NOTES.md#d-47) withdrew the hold-back claim and explicitly kept the other half: the
barrier "does still reach every outstanding operation on the ring rather than only the current
submission batch".

So commit latency is a function of whatever else shares the ring. The ring is therefore part of the
log's durability unit, and "one ring per log" is a precondition rather than a sample convenience.

**Refined when `M23.1` implemented this (2026-09-23); the finding is left as recorded, per Tier 3.**
The barrier is a *ring* flag and the flush names a *file*, so the two bound different things: the
barrier bounds what a commit waits for, the flush bounds what it makes durable. Completion is not
durability, so a shared ring threatens the **cost model** rather than the guarantee. See
[contract.rs](../examples/epoch_log/contract.rs) -> "The ring bounds the wait; the device bounds the
durability", which is authoritative over this paragraph.
[contract.rs](../examples/epoch_log/contract.rs) -- which is where this sample puts its
preconditions, and which was deliberately written before the code -- does not say so.

### S-2. The re-founding of `AlternatingRings` that M20.6 is asking for

[M20.6](../CHECKLIST.md) asks whether alternating rings still earns its cost now that its stated
benefit (keeping appends off a stalled ring) is withdrawn, and offers "epoch *N+1*'s appends are
provably outside epoch *N*" as the remaining benefit, characterising that as a correctness property
rather than a throughput one.

Read against S-1 it is also a throughput property, sited differently. What alternating rings buys is
a **bound on what a commit's barrier can be dragged into**: under a shared ring, commit latency is
unbounded in unrelated traffic on that ring; under alternating rings it is bounded by the epoch.
That is a stronger answer than the one M20.6 currently records, and it is measurable with the
harness that already exists.

Unmeasured. It follows from the flush's recorded scope plus D-47's surviving half.

### S-3. The storage-affinity question, and the adjacent one that is answerable

The engineer's framing -- Windows does not really present storage NUMA affinity for NVMe-attached
storage, but that is no reason not to anticipate it -- matches what the repository has already
established, and [M20.4](../CHECKLIST.md) holds the mechanism research: `FSCTL_QUERY_VOLUME_NUMA_INFO`
takes a file or directory handle directly with no device-instance walk; `GetNumaNodeNumberFromHandle`
bottoms out in `NtQueryInformationFile` with `FileNumaNodeInformation` (class 53), which PHNT and the
WDK mark reserved for system use, so this crate must not build on it; and no published measurement of
either succeeding on an ordinary NTFS data file could be found. The conclusion survives for a better
reason than the one originally recorded: the documented meaning is the node the *volume* resides on,
not where the file's extents live, so it cannot answer "which ring should this file's I/O go to" even
when it succeeds.

Two ways to anticipate it without claiming it, both of which keep [D-8](../DESIGN-NOTES.md#d-8)
intact by leaving the policy with the consumer:

1. **Declare rather than discover.** Let a consumer *state* the storage node for a domain and have
   the arena allocate there via `VirtualAllocExNuma`. The unanswerable question becomes a declared
   input; [file-handle-numa-spike.rs](spikes/file-handle-numa-spike.rs) fills it in automatically if
   hardware ever answers.
2. **Shard by backing device rather than by node.** `IOCTL_STORAGE_GET_DEVICE_NUMBER` and
   `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` -- both already named in the spike -- answer *which
   physical device backs this handle*, and that question is reachable today on ordinary hardware. It
   is also the question that governs the cost this sample is built around: a device cache flush is
   per-device, so two logs on one device contend at every commit, and a ring spanning two devices
   takes the slower device's flush on every covering flush.

The second is the substantive suggestion of this session: the placement question people reach for
(which node) is unanswerable, while the adjacent question that actually sets commit cost (which
device) is not. Unmeasured; it follows from the flush's scope, and the instruments to settle it
exist.

## Already queued, still undone

Two corrections were queued before this session and have not landed. Recorded here so this session
is not read as discovering them:

- [M20.4](../CHECKLIST.md) -- "What is not reachable" in [DESIGN-NOTES.md](../DESIGN-NOTES.md) still
  carries the superseded mechanism (walking volume to disk to device instance and reading
  `DEVPKEY_Device_Numa_Node`). The corrected mechanism is written in the checklist item and not in
  the decision.
  **Landed the same day**, in this session's follow-up work -- see
  [COMPLETED-CHECKLIST.md](../COMPLETED-CHECKLIST.md#m204), which also records two things the item's
  own text had stale.
- [M20.6](../CHECKLIST.md) -- the `AlternatingRings` re-evaluation. S-2 above is an addendum to it,
  not a replacement.
