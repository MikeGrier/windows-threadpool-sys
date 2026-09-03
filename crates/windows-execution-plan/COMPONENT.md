# windows-execution-plan

**Planned, not built.** This directory currently holds a plan and no code. It becomes a crate
when [CHECKLIST.md](CHECKLIST.md) M2 begins; until then it exists so the work has an owner and a
place, rather than living as an assumption inside somebody else's milestone.

## What it is

A **synthesizer**. It takes two inputs and produces a third thing:

- **the observed machine** -- what Windows reports, plus whatever else is trivially available. This
  is `windows_topology_sys::Topology`, and it is **mockable**: a description of a machine nobody has
  is a first-class input, which is what makes this component testable without the hardware it plans
  for.
- **a description of the desired function** -- the scenario. What the caller intends to run, in
  enough detail that a measurement taken on its behalf means something.

From those it synthesizes **a concrete description of the arrangement to construct**: which
processors host domains, where each thread pins, which memory node each allocates from, what channel
connects each pair, and where each channel's buffer lives.

Two things follow that a "takes a topology, returns a plan" description would miss.

**It may ask.** Planning is a negotiation, not a pure function: the component may call back to its
caller through traits, for clarifying information the scenario did not settle. Which questions those
are is not yet known, and knowing them is what decides whether that is one trait or several.

**It may measure, with permission.** This is the component that probes, and it is the right one --
because a measured number is only meaningful alongside *what it measured*. The probe's existing
figures are nanoseconds for one ring-handoff pattern at one message size; a component that knows the
scenario can measure the right thing, where a `Topology::discover` that measured could not, having
no idea what the caller intends.

So there are three stages, each honest about its cost: **observe** (cheap, no choices), **synthesize**
(may measure, with permission), **execute** (no I/O, no probing).

The arrangement it plans for is the one
[CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) M33+ describes -- "one pinned thread, its
`IoRing`, its node-local registered pool, its shard" -- which is a Seastar-style shard-per-core
runtime.

## Two graphs, one word

Both inputs and the output are graphs of processors and their relations, so "topology" fits all of
them and distinguishes none. That ambiguity is live and unresolved: what the machine **is** and what
we intend to **build on it** are different enough that a reader seeing `Topology` twice will
eventually take one for the other. Naming is tracked as an open decision rather than settled by
whoever writes the first type.

## Why it is separate

Because two different kinds of statement were being made by one crate.

**`windows-topology-sys` states facts.** Which processors exist, what they share, at what
granularity, how that was established, and what was measured. It never says "use an SPSC ring
here", because that is not a fact about the machine.

**This crate applies policy.** One domain per core or per thread? Are efficiency cores peers or
excluded? SPSC everywhere, or SPSC within a cache domain and something else across one? Those are
choices, they depend on the workload, and reasonable clients will differ.

Keeping them in one crate has a specific failure mode, already observed: a policy answer gets
mistaken for a fact and consumers bind to it. `outermost_partitioning_cache` is that -- a single
policy choice ("give me one boundary to shard on") sitting in the facts crate, which three
consumers then re-derived differently. See
[CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md) SH-16.9.

## The seam, and how to tell if it is in the right place

**The planner must not re-derive anything.** If it has to work out for itself which cache level
partitions the machine, or reconstruct a mapping the topology already knows, the seam is wrong and
the missing query belongs in `windows-topology-sys`.

That test is the reason this crate is being planned *before* the topology model is finished rather
than after: its input requirements are the concrete statement of what the model has to answer, and
they feed the open design session directly.

## Why it is not the runtime either

The runtime (M33+, spanning `windows-ioring-sys`, `windows-thread-ambient-sys` and
`windows-namespace-request-sys`) *executes* a plan: it creates the threads, binds them, allocates
the pools, constructs the rings. This crate decides what that plan should be.

Separating them means a plan is a **value** -- inspectable, comparable, testable against a
synthetic topology for a machine nobody has, and reviewable by a human before anything is pinned
or allocated. A planner fused into the runtime can only be tested by running it on the machine it
plans for, which is exactly the class of test this repository has repeatedly found inadequate.

## Status and gating

Blocked on
[DESIGN-SESSION-2026-09-02-cache-locality-model.md](../../design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md).
The planner's central query -- "how close are these two processors?" -- has no answer in the
current topology model, and what shape it takes is the subject of that session.

The crate name is provisional. Changing it is cheap now and expensive once it is in
`release-please-config.json`, the publish workflow's tag patterns, and the manifest -- so it is
deliberately absent from all three until the name is ratified.
