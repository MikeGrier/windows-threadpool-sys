# topology-planner

**Production planner planned, not built.** This directory owns the design and plan.
The isolated [read/checksum experiment](experiments/read-checksum/COMPONENT.md)
supplies evidence for `EP-R1.7`; it is not the production planner or a settled runtime.
The production crate begins with [CHECKLIST.md](CHECKLIST.md) M2.

Named without a `windows-` prefix on purpose: it plans against an abstracted idealized machine and
emits a platform-neutral plan, so nothing in it is Windows-specific. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `EP-D-4` for the architecture, and `EP-D-5` for the layout.

## What it is

A **runtime planner**. It takes two semantic inputs, may gather allocation-specific evidence, and
produces a concrete result for the current run:

- **a topology specification** -- what the caller intends the arrangement to achieve, what
  constraints it must obey, and what runtime characterization it is permitted to perform. Its
  detailed shape remains open. The planning universe defaults to the supplied system and may be
  narrowed by a constraint; developer-defined partitions constrain candidate plans without becoming
  claims about physical proximity.
- **an abstracted idealized description of a machine** -- processors, memory, storage and network
  endpoints with their observed NUMA attachment and typed capabilities, interconnects, distances
  and bottlenecks. Not Windows-shaped, and richer than any single platform reports. It is
  **mockable by construction**: a description of a machine nobody has is an ordinary input, which
  is what makes this component testable without the hardware it plans for.

The planner normally runs a permissioned measurement campaign against the current allocation before
producing **a concrete plan**: which processors host domains, where each thread pins, which memory
node each allocates from, what channel connects each flow, where each channel's buffer lives, and
where cross-domain movement is intentional. The plan **serializes to JSON**, stays abstracted from
Windows, and retains the evidence and assumptions behind its allocation-specific choices.

**It may ask.** Planning is a negotiation, not a pure function: the component may call back to its
caller through traits for clarifying information the goal did not settle. Which questions those are
is not yet known, and knowing them is what decides whether that is one trait or several.

**It measures at runtime by default.** The planner owns which scenario-specific questions to ask and
how their answers shape the plan. Neutral measurement contracts live in `topology-model`; Windows
components execute them; and the probe tools use the same underlying measurement kernels. See
[EP-D-6](DESIGN-NOTES.md#ep-d-6).

## The five components, and which way the arrows point

| Component | Platform | Depends on |
|---|---|---|
| `topology-model` | neutral | nothing |
| `topology-planner` (this one) | neutral | `topology-model` |
| the inward adapter | Windows | `topology-model`, `windows-topology-sys`, Windows device, volume, and network APIs |
| the measurement foundation | Windows | `topology-model`, Windows APIs, measured queue and I/O primitives |
| the outward adapter (the realizer) | Windows | `topology-model`, the runtime crates |

`topology-model` holds the abstract machine description, **the traits the planner queries**, and
**the plan type**. Everything depends on it; components that only describe or realize topology
depend on `topology-model` and do not depend on this crate. Callers that need planning policy do
depend on this crate.

That is the whole point of the arrangement. If the traits lived here, an adapter whose only job is
to describe a machine would have to depend on a planner, and anyone wanting to read a topology would
pull in planning policy they did not ask for. The plan type is here for the same reason one level
down: the realizer *executes* a plan and has no business depending on the policy that chose it.

The measurement foundation is separate for the same dependency reason. The inward adapter reports
platform facts; the measurement foundation performs active scenario-specific work; and the realizer
constructs a completed plan. Probe tools and the planner use the same measurement kernels without
depending on each other. See [EP-D-7](DESIGN-NOTES.md#ep-d-7).

## Two kinds of adapter

**Inward** -- exposes the model's traits over platform observations.
`windows_topology_sys::MachineMemoryTopology` supplies processor, memory, and relation facts.
Separate Windows device, volume, and network surfaces may supply I/O endpoint identity, NUMA
attachment, and capabilities such as RSS receive steering. Interconnect costs and scenario-specific
measurements do not come from those sources. The inward adapter reconciles and translates observed
facts; it does not run the planner's measurement campaign. See
[EP-D-9](DESIGN-NOTES.md#ep-d-9).

**Outward (the realizer)** -- takes a plan and **realizes** it in the current process: buffers,
rings and threads, with the user's processing code inserted at the appropriate steps.

They are separate crates despite both being Windows adapters, because their dependency sets barely
overlap -- the inward one needs fact-discovery APIs and `windows-topology-sys`, while the realizer
needs the runtime. Fusing them would mean anyone reading a topology pulls in the whole runtime.

## Why the planner is separate from the facts

Because two different kinds of statement were being made by one crate.

**`windows-topology-sys` states facts.** Which processors exist, what they share, at what
granularity, how that was established, and what was measured. It never says "use an SPSC ring
here", because that is not a fact about the machine.

Per [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21), it is a policy-free, directionless,
memory-safe elevation of the processor and memory topology facts exposed by the Win32 APIs. A shape
that does not directly match this planner's goal is an adapter or neutral-model concern, not a reason
to reshape the facts crate. An actual defect in those facts or their memory-safe elevation is still
fixed at its source.

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

That test is why this component's requirements were written before the Windows topology reshape:
they gave the concluded locality-model session a real consumer to test against. The reshape has now
shipped. Those requirements remain evidence and adapter input, while the client-shaped
`topology-model` contract is this component family's own work.

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

**Deferred past PR #56, by the engineer's direction.** This component contributed only planning
documents to that effort and no code; PR #56 later closed unmerged. The topology reshape shipped by
another route without this component, because [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21)
establishes that `windows-topology-sys` publishes a refined view of what the platform publishes and
an adapter absorbs whatever this component needs beyond that. The two are no longer coupled.

The design session that previously blocked this component has concluded: its questions were answered
as `D-13` through `D-21` in
[windows-topology-sys/DESIGN-NOTES.md](../windows-topology-sys/DESIGN-NOTES.md), and the central
query -- "how close are these two processors?" -- is answered by the ordered relation set that `MMT`
M2 and M4 build. What remains here is this component's own work, not a wait on someone else's.

The name is settled; the crate does not exist yet. It is deliberately absent from
`release-please-config.json`, the publish workflow's tag patterns, and the workspace manifest until
there is code to publish.
