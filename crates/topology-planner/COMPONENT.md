# topology-planner

**Planned, not built.** This directory currently holds a plan and no code. It becomes a crate
when [CHECKLIST.md](CHECKLIST.md) M2 begins; until then it exists so the work has an owner and a
place, rather than living as an assumption inside somebody else's milestone.

Named without a `windows-` prefix on purpose: it plans against an abstracted idealized machine and
emits a platform-neutral plan, so nothing in it is Windows-specific. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `EP-D-4` for the architecture, and `EP-D-5` for the layout.

## What it is

A **planner**. It takes two inputs and produces a third thing:

- **a stated goal** -- what the caller intends the arrangement to achieve. Its shape is deliberately
  **deferred for litigation**; that is a named deferral, not an omission.
- **an abstracted idealized description of a machine** -- processors, memory, storage, interconnects,
  distances and bottlenecks. Not Windows-shaped, and richer than any single platform reports. It is
  **mockable by construction**: a description of a machine nobody has is an ordinary input, which is
  what makes this component testable without the hardware it plans for.

From those it produces **a plan**: which processors host domains, where each thread pins, which
memory node each allocates from, what channel connects each pair, and where each channel's buffer
lives. The plan **serializes to JSON** and stays abstracted from Windows.

**It may ask.** Planning is a negotiation, not a pure function: the component may call back to its
caller through traits for clarifying information the goal did not settle. Which questions those are
is not yet known, and knowing them is what decides whether that is one trait or several.

## The four components, and which way the arrows point

| Component | Platform | Depends on |
|---|---|---|
| `topology-model` | neutral | nothing |
| `topology-planner` (this one) | neutral | `topology-model` |
| the inward adapter | Windows | `topology-model`, `windows-topology-sys` |
| the outward adapter (the realizer) | Windows | `topology-model`, the runtime crates |

`topology-model` holds the abstract machine description, **the traits the planner queries**, and
**the plan type**. Everything depends on it; **nothing depends on this crate**.

That is the whole point of the arrangement. If the traits lived here, an adapter whose only job is
to describe a machine would have to depend on a planner, and anyone wanting to read a topology would
pull in planning policy they did not ask for. The plan type is here for the same reason one level
down: the realizer *executes* a plan and has no business depending on the policy that chose it.

## Two kinds of adapter

**Inward** -- exposes the model's traits over the topology objects already designed, so
`windows_topology_sys::MachineMemoryTopology` becomes one source feeding the abstract model. It is
one source among several: storage and interconnect facts do not come from there, and neither do
measured numbers.

**Outward (the realizer)** -- takes a plan and **realizes** it in the current process: buffers,
rings and threads, with the user's processing code inserted at the appropriate steps.

They are separate crates despite both being Windows adapters, because their dependency sets barely
overlap -- the inward one needs only `windows-topology-sys`, while the realizer needs the runtime.
Fusing them would mean anyone reading a topology pulls in the whole runtime.

## Why the planner is separate from the facts

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
partitions the machine, or reconstruct a mapping the model already knows, the seam is wrong and the
missing query belongs in `topology-model` -- or, if it is a Windows fact, in the inward adapter.

That test is the reason this component is being planned *before* the topology model is finished
rather than after: its input requirements are the concrete statement of what the model has to
answer, and they feed the open design session directly.

## Why it is not the runtime either

The runtime (M33+, spanning `windows-ioring-sys`, `windows-thread-ambient-sys` and
`windows-namespace-request-sys`) *executes* a plan: it creates the threads, binds them, allocates
the pools, constructs the rings. This crate decides what that plan should be, and the realizer
bridges the two.

Separating them means a plan is a **value** -- inspectable, comparable, testable against a
synthetic topology for a machine nobody has, and reviewable by a human before anything is pinned
or allocated. A planner fused into the runtime can only be tested by running it on the machine it
plans for, which is exactly the class of test this repository has repeatedly found inadequate.

The arrangement it plans for is the one
[CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) M33+ describes -- "one pinned thread, its
`IoRing`, its node-local registered pool, its shard" -- which is a Seastar-style shard-per-core
runtime.

## Status and gating

**Deferred past PR #56, by the engineer's direction.** This component contributes only planning
documents to that PR and no code. The topology reshape it fed requirements into is landing there
without it, because [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21) establishes that
`windows-topology-sys` publishes a refined view of what the platform publishes and an **adapter**
absorbs whatever this component needs beyond that -- so the reshape is self-justified and the two are
no longer coupled.

The design session that previously blocked this component has concluded: its questions were answered
as `D-13` through `D-21` in
[windows-topology-sys/DESIGN-NOTES.md](../windows-topology-sys/DESIGN-NOTES.md), and the central
query -- "how close are these two processors?" -- is answered by the ordered relation set that `MMT`
M2 and M4 build. What remains here is this component's own work, not a wait on someone else's.

The name is settled; the crate does not exist yet. It is deliberately absent from
`release-please-config.json`, the publish workflow's tag patterns, and the workspace manifest until
there is code to publish.
