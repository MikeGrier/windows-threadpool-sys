# Design session 2026-08-22: topology description, and the Linux cross-check

Decisions produced: D-1 through D-10 in [DESIGN-NOTES.md](../DESIGN-NOTES.md).

Continues the same day's [IoRing architecture
session](../../windows-ioring-sys/design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md), which
ended with locality left as the consumer's decision (that crate's D-8). This session asked what the
consumer would need in order to make that decision well.

## The motivating position

The engineer's stated view, which is the premise the rest of this rests on:

> I'm kind of obsessed with enabling numa; I believe that as the microarchitectures mature further we will
> see more federation of effectively partitioned memory and IO spaces which are usable in a fungible manner
> but which deliver order of magnitude better performance when aligned.

With the constraint, consistent with the rest of the repository, that no expensive built-in mechanism
should be built to serve it. The wanted outcome was a description that could be *discovered* from the
running system or *fed in* from elsewhere, with a separate policy turning it into a ring configuration --
explicitly not a description that is merely a list of rings to create.

That separation is what makes a synthetic description useful: it lets a machine you do not have induce a
configuration you can inspect.

## Whether the `windows` crate already solves it

Checked rather than assumed, because the answer decided whether a crate was warranted at all. It does not.
`windows` 0.61.1 gives typed FFI -- `pub unsafe fn` returning `windows_core::Result<()>` but taking raw
pointers -- and leaves every genuine hazard with the caller. The specifics are recorded in D-1; the one
worth repeating is that `PROCESSOR_RELATIONSHIP.GroupMask` is declared as `[GROUP_AFFINITY; 1]` while
actually being `GroupCount` elements long, so correct use of the API requires exactly the read that Rust
defines as undefined behavior.

That is a textbook instance of the gap this repository exists to close, so the crate is justified on its
own terms rather than only as a dependency of the ring sample.

## Thin wrapper or renderer

Both shapes were considered. The argument that settled it was the engineer's own premise: if
microarchitectures are going to keep federating, then *interpretation* is the volatile part and
*enumeration* is the durable part. Putting the durable thing in the crate and the volatile thing above it
is the layering this repository already follows.

The refinement was that a strictly faithful wrapper handing back raw masks would be safe FFI rather than a
safe API, because the recurring real-world bugs are group-related: assuming one processor group, assuming
64 or fewer processors, flattening a `(group, number)` pair to an index. So `ProcessorSet` was admitted as
the single abstraction above faithful records (D-3), and nothing else.

## The Linux cross-check

Proposed by the engineer as a cross-check, and explicitly with an eye to future expansion directions
rather than only to validating the draft. It served both purposes and was the most productive part of the
session.

**Three things in the draft were genuinely violated.**

1. **Memory-only nodes.** Linux exposes `has_cpu` / `has_memory` per node because CXL expanders, PMEM in
   system-RAM mode, HBM tiers, and coherent GPU memory all appear as nodes with memory and no CPUs. The
   draft defined a node *by* its processor list, making the CXL case degenerate rather than first-class --
   backwards for precisely the hardware direction that motivated the session. Became D-5. This is the
   finding that justified running the comparison: Windows' model is impoverished here, so looking only at
   Windows would have missed it.
2. **Fixed domain kinds.** Linux models `die` and `cluster`, and `book` / `drawer` on s390x. Cache domains
   approximate dies but do not equal them -- Zen 2 had two L3 domains per die. Enumerating every level any
   architecture will ever have is unwinnable, so domains became open-kinded. Became D-4.
3. **No online/offline state.** Linux distinguishes possible, present, and online with hotplug; Windows
   distinguishes active from maximum processors per group. Missing on both sides of the draft.

**Three decisions held up unchanged**, which was the reassuring half: processor identity as
`(group, number)` (lossless when sourced from a system without groups), reference-don't-nest (Linux's
levels do not form a strict hierarchy either -- clusters cut across cache domains on some ARM parts), and
treating distances as optional (vindicated, since Linux *has* SLIT where Windows does not, so the field
earns its keep in exactly the fed-in-description case).

A further consequence surfaced: a Linux-sourced description can have one group with more than 64
processors, which is unrepresentable as a Windows affinity mask. Rather than constraining the schema, the
constraint belongs in the planner. Became D-10.

## The expansion directions, and the decision to decline them

Having found what is already real, the discussion turned to what is visibly coming. The unifying
observation was that every direction pushes the same way: what generalizes is **a graph of attributed
relationships between initiators and targets**, not a list of processor groupings.

Four candidates were laid out with the two that would cost a breaking change if deferred called out as
such: HMAT-style attributed relations (read/write latency and bandwidth per initiator-target pair,
superseding SLIT's single scalar), devices as first-class initiators, queue and interrupt affinity, and
the various non-locality partitioning domains (power, cache partitioning, memory encryption).

**The engineer declined all of them for now**, and the schema stayed at the scoped shape from the
cross-check. Recorded in full, with revisit triggers, under D-9.

Two things make that a sound decision rather than a postponement of pain:

- The **breaking-change objection dissolves** against D-8. The JSON schema is explicitly not semver-covered,
  following `windows-file-watcher`'s D-71 precedent for its scenario schema, so a v2 is permitted when
  HMAT-class hardware actually matters to a consumer. The cost of not pre-building is therefore bounded,
  which is the fact that turns "we should build the edge list now" into a genuine choice rather than a
  forced move.
- Devices-as-initiators is not merely more fields -- it changes the crate from *processor topology* to
  *system topology*, a materially larger surface than the name promises. Declining it keeps the crate
  honest about what it is.

The engineer's framing was that what a design chooses not to do is as important as what it does, which is
why D-9 exists as a decision with reasons and revisit triggers rather than as an absence.

## Deliberately left open

- Whether the sample's policy set should eventually be data-driven rather than named code. Named code for
  now.
- Whether `windows-topology-sys` remains the right name if devices are ever admitted (D-9 suggests not).
