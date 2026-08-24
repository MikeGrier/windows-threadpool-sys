# Design session 2026-08-22: Windows IoRing, and how it should relate to this repository

Decisions produced: D-1 through D-10 in [DESIGN-NOTES.md](../DESIGN-NOTES.md).

Participants: the engineer driving the repository, and an assistant. The session began as "how should or
could the Windows IoRing interact with the overlapped-io and threadpool components we have built here",
ran a throwaway spike against a real machine, and ended by settling the crate's architecture and
philosophy.

## Starting question

Whether IoRing is a third completion backend for `windows-overlapped-io-sys`, a separate crate, or
something that should be generalized into a shared core.

## What the spike found

A throwaway program (`.scratch/ioring-spike/`, git-ignored, disposable) probed the API directly rather
than relying on documentation. It is worth re-creating rather than trusting this summary if any of these
become load-bearing again.

Findings are listed under "What the spike established" in [DESIGN-NOTES.md](../DESIGN-NOTES.md). Two of
them changed the design materially:

1. **A file handle does not need `FILE_FLAG_OVERLAPPED`.** The assistant had assumed it would, which would
   have made `UnassociatedEndpoint` the natural input type and pulled this crate toward
   `windows-overlapped-io-sys`. It does not, so the two crates are less coupled than expected.
2. **`SetIoRingCompletionEvent` is behind a runtime feature flag** (`IORING_FEATURE_SET_COMPLETION_EVENT`),
   and there is also an `IORING_FEATURE_UM_EMULATION` flag meaning the ring may be emulated in user mode
   with no kernel benefit. Both are probe-at-runtime facts. On the machine tested, the event was available
   and emulation was off.

Incidental but useful: the IoRing bindings live in `windows-sys` under `Win32_Storage_FileSystem`, which
M13.1 had made unconditional in `windows-overlapped-io-sys` earlier the same day. No feature work was
needed to spike it.

## How the "arbitrary I/O" question resolved

The engineer asked to enable the ring for arbitrary I/O, and to be clear about why if a division was
warranted. The division turned out to be forced rather than chosen: the kernel op table is seven entries
and has never grown toward sockets or ioctl, unlike Linux's `io_uring`. This became D-2, and it reframed
the crate: `windows-ioring-sys` is a file data plane, not a general async backend, and its natural
vocabulary is files, batches, and registered buffers rather than endpoints and generic operations.

The engineer's constraint was that the design must extend when the op table does. That became D-7:
version negotiation rather than a hardcoded version, a cached per-op capability set, a `#[non_exhaustive]`
op enum so consumers cannot write a `match` that a new op breaks, and a narrow unsafe raw-SQE seam
mirroring the `device` family's existing unsafe `ioctl` precedent.

## How the completion shape resolved

The assistant offered two candidates: a per-operation closure stored in a slab, or a typed token the
caller reclaims. The engineer chose the second, on the grounds that the crate should strive for no more
allocations than necessary, and that the allocating convenience variant can be built atop the rawer typeful
one rather than the reverse.

This became D-4. The safety valve -- `mem::forget` on dropping a live token -- was the piece that made a
zero-allocation design sound rather than merely fast.

## The NUMA thread, which was the longest part of the session

The engineer's hope was that IoRing might feed an IOCP, because IOCP delivery carries implicit
associativity between the NUMA region of the device that satisfied the work and the thread that picks up
the completion, and indirecting through an event appeared to lose that.

Three corrections came out of this exchange, in order:

1. **The assistant had overstated ring sharding.** An earlier claim that per-node rings were "the natural
   design" imported an `io_uring` idiom that does not transfer. Sharding reduces submission-queue
   contention and gives control over one's own callback threads; it does not recover device-to-CPU
   locality.
2. **But the IOCP bridge cannot recover it either**, which settles the question against the original hope.
   The association is lost inside the kernel, before userspace runs: whatever thread the event wakes is
   chosen by the wait, so a subsequent `PostQueuedCompletionStatus` enters the port from an already
   arbitrary processor. This became D-9, and it is a stronger argument against the bridge than the
   two-kernel-transitions efficiency argument that preceded it.
3. **IOCP very likely does not do automatic NUMA-local dispatch either.** Recorded as an explicitly
   unverified belief in D-10, with the reasoning that the standard high-performance IOCP pattern is one
   port per node with manually affinitized threads -- which nobody would build if the kernel did it. Not
   resolvable on the machine at hand, which reported a single node (an 8-core slice of an EPYC 7763).

The engineer then pushed on the premise that "per NUMA node" is not well defined in the real world, which
was correct and produced the "Why the NUMA node is the wrong key" section: node count is an NPS/SNC
firmware setting rather than a hardware property, virtualized topology is frequently flattened or absent,
and the last-level cache domain is a better key than the node -- finer, meaningful across vendors, and
degrading sanely to one domain.

The three-way tension (serialization wants fine, registration wants coarse, dispatch wants whatever is
affinitized) was identified here, along with the observation that registration is the axis that punishes
over-sharding because it pins pages per ring.

## The realization that reorganized the plan

Clarifying that the question was about overall high-performance application architecture rather than about
who owns the rings produced the Model A / Model B framing, and with it the observation that **in Model B
the completion event is not needed at all** -- a pinned thread parks directly in
`SubmitIoRing(wait_n, timeout)`, and the fused submit-and-wait is the event loop.

That inverted the plan. The assistant had scoped the pinned-thread path twice, first as a fallback for a
missing capability and then as "the NUMA story"; both framings were wrong. It is the high-performance
architecture, and the thread-pool path is the convenient one. This became D-3, and the two paths are now
first-class siblings rather than primary and degraded.

It also made the engineer's earlier instinct precise. They had said they prefer to build threadless but
suspected a limit around NUMA association. The limit is exactly the Model A / Model B boundary: threadless
means a pool, a pool means a shared queue, and a shared queue means load balancing by design. That
property is unobtainable from a pool on IOCP and IoRing alike.

## Buffer placement

Raised late and probably underweighted throughout: the device DMAs into the registered buffer, so a buffer
on a remote node costs interconnect traversal on every byte of every operation, where callback placement is
a one-time cache-warmth question. `VirtualAllocExNuma` on the device's node is likely the highest-leverage
locality decision in the whole design, and is independent of the completion-routing discussion that
occupied most of the session.

## Philosophy, stated by the engineer and adopted as the crate's intent

> my goal here is not to solve all peoples' problems for them but to provide a memory safe toolkit for them
> to be able to use to solve their problems. The Win32 api set provides a lot of good primitives, let's
> raise them up with memory safe patterns with the minimum additional cpu and memory costs.
>
> We don't have to solve everything but if we can make it easy to construct rings, and give pointers on how
> to construct rings with the right sorts of affinities etc. this is just a good pattern for everyone.

This is why D-8 exists in the form it does, and why the architecture guidance is written into the design
notes for consumers rather than kept as maintainer-only reasoning: people reaching for these crates are
trying to maximize I/O throughput, and the trade-offs above are what they need to know.

## Deliberately left open

- The `RingFleet` / partitioning abstraction. Not in the initial plan; wants evidence first (D-8).
- Whether `IoBuf` is eventually shared with `windows-overlapped-io-sys` or stays duplicated (D-1).
- Whether IOCP performs NUMA-local dispatch (D-10) -- recorded as unverified, no work scheduled, because
  the design consequence does not depend on the answer.
