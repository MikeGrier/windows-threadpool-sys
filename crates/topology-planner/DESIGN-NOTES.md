# Design notes: the topology planner

Current canonical decisions for this component. See [COMPONENT.md](COMPONENT.md) for what the
component is; see [CHECKLIST.md](CHECKLIST.md) for what is planned. Historical context and
exploratory analysis live in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

`EP-D-1` through `EP-D-3` are **queries** rather than choices: the planner's requirements, stated
precisely enough that the topology model could be designed against a real caller instead of against
a guess. They were written when the planner was to read `windows_topology_sys::MachineMemoryTopology`
directly; [EP-D-4](#ep-d-4) rebinds them to traits over an abstract model, which changes what
satisfies them and not what they require.

[EP-D-4](#ep-d-4) is the first genuine **choice** here, and it re-scopes the component: the planner
is `topology-planner`, it plans against an abstracted idealized machine, and adapters bracket it on
both sides. [EP-D-5](#ep-d-5) then settles the layout EP-D-4 left open, and the directory has been
renamed to match.

## Decision index

| ID | Decision |
|---|---|
| <a id="ep-d-1"></a>EP-D-1 | **The shard-set query**: what the planner must know to choose which processors host a domain, and what today's model cannot tell it. |
| <a id="ep-d-2"></a>EP-D-2 | **The proximity query**: how close two processors are, which selects the channel between their domains. Takes an **unordered** pair; the model has no answer today. |
| <a id="ep-d-3"></a>EP-D-3 | **The residency query**: where a domain's pool lives, and which side of a cross-domain pair should host a shared ring. **Ordered**, with directed cost entering through the abstract model/adapter path under [D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20). |
| <a id="ep-d-4"></a>EP-D-4 | **The four-part architecture, and the planner's name.** The engineer's position: the planner is **`topology-planner`** (no `windows-` prefix); it takes a **goal** description (shape deferred for litigation), queries an **abstracted idealized** model covering processors, memory, storage, interconnects, distances and bottlenecks, and emits a **JSON-serializable, platform-neutral** plan. Two kinds of **adapter** bracket it: one exposing the planner's traits over the Windows topology objects, one **realizing** a plan as buffers, rings and threads with the user's code inserted at the right steps. Settles `MMT-1.5` (the facts crate keeps its `-sys` name), the "two graphs, one word" ambiguity, and where distance lives -- the attributed interconnect shape D-9 sketched goes in the abstract model, so D-9's deferral in the facts crate stands unreopened. |
| <a id="ep-d-5"></a>EP-D-5 | **The component layout: `topology-model` is its own crate, and dependencies point one way.** The abstract model and the traits the planner queries live in `topology-model`, which the planner and both adapters depend on; non-planner components do not depend on `topology-planner`. Putting the traits in the planner would make a crate whose job is to *describe a machine* depend on one that applies *policy* -- the same defect as `outermost_partitioning_cache`, arriving as a dependency edge instead of an API. Two consequences derived from the same rule rather than decided separately: **the plan type also lives in `topology-model`** (otherwise the realizer depends on the planner), and the inward adapter and the realizer are **separate crates** (their dependency sets barely overlap, and fusing them would make reading a topology pull in the whole runtime). |
| <a id="ep-d-6"></a>EP-D-6 | **Runtime measurement is a planner-owned campaign over shared measurement mechanisms.** Runtime planning is the normal path, not a fallback: the planner decides what the scenario and current allocation require, sequences and interprets measurements, and stops when it has enough evidence. Neutral request/result contracts live in `topology-model`; platform components execute them; probe tools and the planner use the same underlying kernels. The client repository carries constraints and permissions rather than an exact allocation, and the concrete runtime plan retains the scenario-specific evidence for its choices without promoting it into an abstract machine fact. |

## EP-D-1: the shard-set query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.1.*

### What the planner is choosing

Which processors may host an execution domain, and how to group them so a policy can pick between
one domain per core and one per logical processor, and can decide whether efficiency cores are
peers, a second tier, or excluded.

This is the first step of the construction and it fixes the domain count, which everything
downstream is shaped by: the number of rings is quadratic in it, and each domain's memory pool is
sized against it.

### What it must know, and why

1. **Identity, as `(group, number)`.** Not a bare index. A processor number without its group names
   a different processor in every group and the wrong one in all but the first, and pinning is a
   `GROUP_AFFINITY` -- `SetThreadGroupAffinity`, not `SetThreadAffinityMask`, which cannot name
   another group at all. A planner that flattens this produces a plan that is silently wrong above
   64 processors.

2. **Whether the processor is online.** An offline slot exists and counts toward a group's maximum;
   planning a domain onto one is planning a thread that cannot run.

3. **Core membership and whether the core is SMT.** The choice between one domain per core and one
   per logical processor is the single largest policy lever, and it needs the sibling grouping, not
   just a count.

4. **Efficiency class.** On a hybrid part, putting latency-sensitive domains on efficiency cores is
   a defect the client will not see in a functional test, only in a percentile.

5. **Whether the processor is available to this process at all** -- parked by the scheduler, or
   outside the CPU-set allocation the process was given.

### What today's model answers

Points 1 through 3 cleanly. `ProcessorId` is `(group, number)` by construction and documents why
(D-7). `Processor::online` is exactly the distinction in point 2. `DomainKind::Core` carries
`simultaneous_multithreading` and the sibling set, so point 3 is a walk of `MachineMemoryTopology::cores()`.

Point 4 is answered, but **twice, in two shapes, and one of them is unsafe to use** -- see below.

Point 5 is **not answered at all**. `GetSystemCpuSetInformation` is consumed nowhere in the
workspace, so `Parked`, `Allocated` and `AllocatedToTargetProcess` are unavailable. A planner
cannot currently avoid pinning a domain to a parked processor, and the client cannot detect that it
happened. Tracked as `SH-16.10` in
[CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md).

### `Processor::capacity` must not be used for point 4

**Use `DomainKind::Core { efficiency_class, .. }`. Do not use `Processor::capacity`.**

`capacity` is computed as `online.then(|| find the owning Core domain).flatten().unwrap_or(0)`, so
the value `0` means any of three different things:

- the processor is offline;
- the processor is online but no `Core` domain names it, which the topology tolerates by design
  since firmware coverage is not guaranteed;
- the processor is online, has a core, and its efficiency class genuinely **is** `0`.

The third is not an edge case. It is **every processor on every non-hybrid machine**, so the
sentinel collides with the overwhelmingly common legitimate value.

For this planner the collision is worse than for most consumers, because Windows orders efficiency
class with `0` as the *least* performant. On a hybrid part an unknown processor is therefore
indistinguishable from an efficiency core, and a policy that excludes efficiency cores would
silently drop a processor that might be a performance core -- while a policy that tiers them would
place it in the wrong tier. Both failures are invisible in a functional test.

`Core { efficiency_class }` carries the firmware value with no sentinel, and absence is represented
by the processor being in no `Core` domain, which is a distinguishable state rather than a value.

**This is the same defect the locality-model session exists to fix, in a third place.** The others:
`ProcessorPlace::cache_domain: Option<u32>`, where `None` conflates "no level partitions this
machine" with "this processor was not named at the level that does" (`SH-16.5`); and
`MachineDescription::cpu_model`, where the same conflation was noticed and solved with a side
boolean. Recorded here so the sweep that fixes the model does not stop at the two already known.

### Partial core coverage is a real state, not a corruption

A processor in no `Core` domain is a firmware gap, not a contradiction, and the topology crate
tolerates it deliberately. The planner must therefore decide what to do with a processor it cannot
group -- it is a candidate host whose SMT relationships and class are unknown, which is exactly the
"unanswered query" case that [CHECKLIST.md](CHECKLIST.md) EP-1.4 owns. It is named here so that
item is not written as though the case were hypothetical.

### What this asks of the topology model

Nothing new in shape; three things in substance.

- Availability (parked, allocated) has to become expressible, since no policy can be correct
  without it.
- Efficiency class has to have exactly one representation, and it must distinguish "class zero"
  from "not known".
- Core membership has to admit that a processor may be in no core, without that being an error.

## EP-D-2: the proximity query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.2.*

### What the planner is choosing

For two domains, what connects them: a dedicated SPSC ring, a shared MPSC ring fanning several
producers into one consumer, or a routed hop through an intermediate domain. That choice is made
once per pair, and it is made from how close the two processors are.

This is the query the whole model question turns on. Everything else the planner asks is either
per-processor (EP-D-1) or per-memory-domain (EP-1.3); this is the only one that is *relational*,
and it is the one today's model cannot answer.

### It takes an unordered pair. The checklist item said ordered, and was wrong.

`windows-placement-probe` already settled this and stated the reasoning, which is worth quoting
because it is easy to get backwards:

> These names are deliberately symmetric, and that is not an oversight left over from before hops
> became directed. The *relationship* between two processors genuinely is symmetric -- two
> processors either are SMT siblings or are not, share a cache domain or do not -- so there is no
> honest `CrossNumaNodeForward` to name. Splitting the labels by direction would invent a
> distinction the topology does not have.
>
> The *workload* is what is asymmetric: the producer writes and the consumer reads, so swapping
> them swaps which side pays. Direction therefore lives where it is real, not in the label.

And on the measured side: "a hop is not symmetric even though the link is."

So the split is clean, and the planner needs both halves in different places:

- **Proximity is the link.** Symmetric, unordered pair, answered here.
- **Residency is the hop.** Asymmetric -- which side hosts the ring buffer, which the probe
  measures with a dedicated ring-placement column because it was found to matter. That belongs to
  EP-1.3, not here.

Putting direction in the proximity query would invent an asymmetry the topology does not have, and
would double the size of an answer that has no second half to fill.

### What the answer must contain

Not a boolean, and not a bare identifier. Three things:

1. **The tightest granularity the two share.** Comparable against other pairs' answers, because the
   policy's threshold ("SPSC within this, MPSC beyond it") is a comparison. The *identity* of the
   granularity matters less than its position.

2. **The membership of that granularity.** Selecting MPSC is not enough -- the planner must size the
   fan-in, which is "how many other domains sit at this same proximity". Without membership the
   planner would ask the proximity query O(n^2) times and reconstruct the grouping itself, which is
   the re-derivation the seam exists to prevent.

3. **Whether a finer granularity went unobserved.** This is the part that a naive design drops. If
   L3 was observed and L2 was not, "tightest shared is L3" is *not* the answer -- the answer is "at
   most L3, and finer was not looked at". A planner told the first would choose a slower channel
   than the machine can support and never learn why. Under the model's bar -- usable without further
   measurement -- it cannot go and check, so the distinction has to be in the answer.

### The query should be total, which needs a top element

Two processors in the same machine always share *something*: one address space, one scheduler, one
memory system, however far apart. If the granularity order has no top, the query returns "nothing
in common" for a cross-node pair and every caller writes the same empty-case branch.

Making "the machine" an explicit top granularity is honest -- it is a real, if loose, locality tier
-- and makes the query total. A bottom ("this processor alone") is the same argument at the other
end and makes `proximity(a, a)` answerable rather than a special case, though a planner has no
reason to ask it.

### A partial order means the answer may not be a single granularity

If the order is by observed set inclusion rather than by firmware numbering, two granularities can
be **incomparable** -- neither refines the other. The tightest shared granularity is then not
unique, and the honest answer is the set of *minimal* shared granularities, which is almost always
exactly one.

This is a cost, and it is worth naming rather than discovering later: every caller either handles a
multi-element answer or documents that it takes the first. But the alternative -- forcing a linear
order -- means silently discarding a real boundary on a machine whose levels do not nest, and this
repository has been bitten specifically by structure that was assumed rather than checked.

### What today's model answers: nothing

`MachineMemoryTopology::outermost_partitioning_cache` reports **one level for the whole machine**, and
`Slice::same_cache_domain` reduces that to a boolean at that one level. Neither is pairwise. There
is no query anywhere in `windows-topology-sys` that takes two processors.

So a planner today reconstructs proximity from the partition list -- which is exactly what
`SH-16.9` records three consumers already doing, in two mutually inconsistent ways. The absence of
this query is the cause of that defect, not a separate problem.

### What this asks of the model

- A granularity order derived from **observed set inclusion**, not firmware level numbers, so a
  measured-only tier and a machine with no L3 both have positions.
- Access to that order **as a collection**, with a pairwise helper derived from it, returning minimal
  shared granularities plus their membership. Stated here first as a pairwise query, which
  [windows-topology-sys](../windows-topology-sys/COMPLETED-CHECKLIST.md) `M4+.1` corrected: requiring the answer
  to carry the block containing both processors makes it a question about the partition, not the pair,
  and a pairwise-primary surface would force the planner into the O(n^2) reconstruction that `SH-16.9`
  records going wrong three times. The three *requirements* below are unchanged; only the shape is.
- Unobserved granularities represented, so an answer can be an upper bound and say so.
- A top element, so the query is total.

## EP-D-3: the residency query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.3.*

### The decision

The planner needs two distinct residency facts:

- **Processor to memory-domain placement** for node-local pool allocation, with unplaced processors
  represented explicitly.
- **Directed cross-domain residency cost** for choosing which side of a pair hosts shared buffers,
  with measurement context attached to measured values.

### Boundary alignment with D-20

[D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20) deleted `MachineMemoryTopology::distances` and
fixed the Win32 boundary for `windows-topology-sys`. Per [EP-D-6](#ep-d-6), directed residency cost
is scenario-specific planning evidence rather than a fact inserted into the abstract machine
description. The planner requests it through a neutral measurement contract, a platform component
executes the measurement, and the result carries the context that gives the number meaning.

### Current status

The processor-to-memory-domain half is available from topology memory-domain memberships and still
keeps the established asymmetry: unknown cache placement can degrade, unknown memory placement has
no honest default.

The directed-cost requirement is settled at the ownership boundary by [EP-D-6](#ep-d-6). Its neutral
request, result, and evidence vocabulary belongs in `topology-model`; its execution belongs in a
platform measurement component; and its interpretation belongs to the planner.

### Historical rationale

Detailed trigger analysis and prior framing are recorded in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-3-rationale-and-history).

## EP-D-4: the four-part architecture, and the planner's name

*The engineer's position, 2026-09-03. This is a **choice**, not one of M1's queries, and it
re-scopes the component that records it.*

### What was decided

**The planner is `topology-planner`** -- deliberately with no `windows-` prefix.

- **Input**: a description of the **goal** of the topology -- what the caller intends the
  arrangement to achieve. Its shape is **explicitly deferred for litigation**, which is a named
  deferral rather than an omission.
- **What it queries**: an **abstracted, idealized** description of the machine, covering
  **processors, memory, storage (NVMe), interconnects, distances, and bottlenecks**. Not
  Windows-shaped, and materially richer than what any one platform reports.
- **Output**: a data structure that **serializes to JSON** and is **still abstracted from Windows**.
- **Adapters, in two directions**:
  - *inward* -- exposing the traits the planner needs **over the topology objects already designed**,
    so `windows_topology_sys::MachineMemoryTopology` becomes one source feeding the abstract model;
  - *outward* -- **realizing** a planned topology in the current process as buffers, rings and
    threads, with the user's processing code inserted at the appropriate steps.

### What it settles

**The crate-naming question** (`MMT-1.5` in
[windows-topology-sys](../windows-topology-sys/COMPLETED-CHECKLIST.md)). The planner does not live in
`windows-topology-sys`, which therefore stays a pure Win32 wrapper and keeps its `-sys` name. The
decisive point is not preference but the adapter boundary: a crate on one side of an adapter is
exactly what `-sys` names, and [D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20) already scoped
that crate to "what the Win32 topology APIs report".

**"Two graphs, one word"** -- [COMPONENT.md](COMPONENT.md) flagged that both the input and the output
are graphs of processors and relations, so "topology" named all of them and distinguished none. Three
things are now distinct: the **machine memory topology** (Windows facts), the **abstract topology**
(idealized, multi-source, platform-neutral), and the **planned topology** (the output). The word is
shared deliberately; the qualifier carries the distinction.

**Where distance lives**, which three decisions had left in tension:

- [D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20) removed `distances` from the facts crate,
  because Win32 does not report it and that crate does not go below Win32.
- [EP-D-3](#ep-d-3) established that the planner needs a **directed** cost, which a SLIT-shaped
  scalar cannot express.
- `windows-topology-sys` D-9 deferred HMAT-style attributed relations until scalar distance
  "demonstrably mismodels a machine somebody is tuning for" -- a trigger this component *approaches*
  and, lacking multi-node hardware, has not met.

The abstract model resolves all three without disturbing any: **interconnects and bottlenecks** are
the attributed-edge shape D-9 sketched, and they live in the abstract model, so D-9's deferral in the
facts crate **stands unreopened** while the need it named is met elsewhere. The measurement condition
still applies before claiming asymmetry is real; it just no longer gates the schema.

**Storage becomes representable**, which `windows-topology-sys` D-9 also excluded -- on the grounds
that it "changes the crate's identity from processor topology to system topology". That exclusion was
about *that crate* and still holds. NVMe belongs to the abstract model, which was never scoped to a
processor topology.

Open follow-up history for this decision is recorded in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-4-open-follow-ups); current actionable work is tracked
in [CHECKLIST.md](CHECKLIST.md).

### What survives unchanged

[EP-D-1](#ep-d-1), [EP-D-2](#ep-d-2) and [EP-D-3](#ep-d-3) are **requirements**, and requirements
survive a change of binding. Each stated what the planner must know and why; what changes is that
they are now satisfied by traits over an abstract model rather than by methods on a Windows type.
They were written against a real caller, which is what makes them portable in this way.

## EP-D-5: the component layout, and which way dependencies point

*The engineer's choice, following [EP-D-4](#ep-d-4). Recorded separately because EP-D-4 explicitly
left it open.*

### The decision

**The abstract model and the traits the planner queries live in their own crate, `topology-model`,
which both the planner and the adapters depend on.**

| Component | Platform | Depends on |
|---|---|---|
| `topology-model` | neutral | nothing |
| `topology-planner` | neutral | `topology-model` |
| inward adapter | Windows | `topology-model`, `windows-topology-sys` |
| outward adapter (realizer) | Windows | `topology-model`, the runtime crates |

Everything depends on `topology-model`; **nothing depends on `topology-planner`** except a caller
that actually wants to plan.

### Why not put the traits in the planner

Because the arrow points the wrong way. An adapter whose job is to describe a machine would have to
depend on a planner in order to describe it, and anyone wanting to read a topology would pull in
planning policy they did not ask for. That is the same defect
[COMPONENT.md](COMPONENT.md) already records in a different place -- `outermost_partitioning_cache`,
a policy answer sitting where facts are stated -- arriving as a dependency edge rather than as an
API.

### The same rule decides where the plan type goes, one level down

This is a **derived** consequence rather than a separately-taken decision, and it is called out
because it is easy to miss: the realizer consumes a plan. If the plan type lived in
`topology-planner`, the realizer would depend on the planner -- policy dragged in by a component
whose only job is to execute.

So **the plan type lives in `topology-model` too**, alongside the machine vocabulary. The crate is
"the shared vocabulary", not merely "the machine description". This is consistent with
[COMPONENT.md](COMPONENT.md)'s existing argument that a plan is a **value** -- inspectable,
comparable, reviewable before anything is pinned or allocated. A value type belongs with the
vocabulary, not with the policy that produced it.

### Two Windows adapters, not one

Also derived. They are both Windows adapters and it is tempting to fuse them, but their dependency
sets barely overlap: the inward one needs `windows-topology-sys`, the realizer needs the runtime
(`windows-ioring-sys`, `windows-waitable-queues`, `windows-thread-ambient-sys`). Fusing them would
mean anyone reading a topology pulls in the whole runtime, which is the same "do not drag in what
the caller did not ask for" rule that decided the layout in the first place.

### What is still open

- **The adapters' names.** Deliberately not settled here; naming has been getting decided by
  whoever writes the first type, and this component has already been renamed once.
- **Whether `topology-model` is one crate or eventually two.** The machine description and the plan
  vocabulary are different enough that they might separate later. They are together now because
  splitting on speculation costs more than merging on evidence.

Measurement ownership is no longer open; [EP-D-6](#ep-d-6) assigns the campaign to the planner and
execution to shared platform measurement mechanisms.

## EP-D-6: runtime measurement ownership and its data boundary

*The engineer's decision, 2026-09-18. Recorded by [CHECKLIST.md](CHECKLIST.md) `EP-R1.1` and
`EP-1+.4`.*

### Runtime planning is the primary product

The planner is expected to do almost all matching at runtime, because the machine and NUMA
allocation available to a deployed process are not reliably known in advance. A checked-in exact
processor, queue, and buffer arrangement would be brittle across fleet evolution and allocation
changes. The client instead supplies a topology specification carrying intent, constraints,
permissions, and acceptable adaptation; the planner matches it to the current allocation and emits
one concrete plan for that run.

### The planner owns the campaign; providers own mechanisms

The planner decides what must be learned for the scenario and allocation, sequences those
measurements, interprets their results, and decides when it has enough evidence to produce or refuse
a plan. The caller authorizes the work through the topology specification, including constraints on
what may be measured and the resources it may consume.

The neutral measurement request, result, and evidence vocabulary lives in `topology-model`, so the
planner, probes, and platform implementations share one contract without giving the neutral planner
a Windows dependency. Platform measurement components execute the operations and return observations;
they do not choose queue policy.

### Probes and runtime planning share measurement kernels

Developer-facing probes and the autonomous planner use the same underlying measurement mechanisms.
The probes expose those mechanisms for early dependency evaluation, architecture work, deeper client
benchmarking, and evidence checked into this repository. The planner invokes them as part of its
normal runtime campaign. A separate runtime implementation would let the repository demonstrate one
behavior while deployed decisions depend on another.

### What persists

Scenario-specific measurements are not inserted into the abstract machine description. Their meaning
depends on direction, payload, queue form, concurrency, and current conditions, so treating them as
timeless machine facts would erase the context required to interpret them.

The concrete plan retains enough measurement evidence, context, assumptions, and constraint
resolution to explain why its allocation-specific choices were made. A client may capture that plan
for diagnostics, but the durable input in the client repository is normally the constraint
specification rather than the exact runtime result.

### Deferred boundary

How much higher-level work-item and buffer-flow machinery this project supplies remains open. In
particular, later design work must determine the interface between completed I/O, parsing stages,
serial and parallel work streams, worker placement, and intentional cross-domain migration, and
whether existing repository code or the Windows thread pool already supplies any part of it. That
question is tracked by [CHECKLIST.md](CHECKLIST.md) `EP-R1.7`; EP-D-6 does not answer it.
