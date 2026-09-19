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

The NUMA testing and completion policy is [EP-D-11](#ep-d-11); the physical-hardware
limitation is shared across this component rather than repeated as an item gate.

| ID | Decision |
|---|---|
| <a id="ep-d-1"></a>EP-D-1 | **The shard-set query**: what the planner must know to choose which processors host a domain, and what today's model cannot tell it. |
| <a id="ep-d-2"></a>EP-D-2 | **The proximity query**: how close processors are within an input planning universe. Locality is a partial order by physical membership; the selected universe is its top, making the query total without making the order total. Developer-specified partitions constrain plans but do not become physical-locality facts. |
| <a id="ep-d-3"></a>EP-D-3 | **The residency query**: where a domain's pool lives, and which side of a cross-domain pair should host a shared ring. **Ordered**, with directed cost entering through the abstract model/adapter path under [D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20). |
| <a id="ep-d-4"></a>EP-D-4 | **The original four-part architecture, and the planner's name.** The planner is **`topology-planner`** (no `windows-` prefix); it takes a goal description, queries an abstracted idealized model, and emits a JSON-serializable platform-neutral plan. Its component count is superseded by [EP-D-7](#ep-d-7), which adds the active measurement foundation without changing the planner name or the two adapter boundaries. |
| <a id="ep-d-5"></a>EP-D-5 | **The shared-vocabulary layout and dependency direction.** The abstract model, planner query traits, and plan type live in `topology-model`, which the planner and platform components depend on; non-planner components do not depend on `topology-planner`. [EP-D-7](#ep-d-7) extends this layout with neutral measurement contracts and a separate platform measurement foundation while preserving the one-way dependency rule. |
| <a id="ep-d-6"></a>EP-D-6 | **Measurement ownership remains with the planner; routine campaign scope is superseded by [EP-D-10](#ep-d-10).** Runtime matching remains primary, but active measurement is optional and constrained by the startup budget. Neutral request/result contracts live in `topology-model`; platform components execute them; probes and the planner share mechanisms. Evidence stays allocation-specific rather than becoming a machine fact. |
| <a id="ep-d-7"></a>EP-D-7 | **Five components, with active measurement as a foundation rather than an adapter concern.** `topology-model`, `topology-planner`, the inward Windows adapter, a Windows measurement foundation, and the outward realizer have distinct dependency sets and responsibilities. Measurement contracts are neutral; measurement mechanisms are platform-specific; probes and runtime planning share those mechanisms; and exhaustive mocked platform-interaction testing is kept separate from real-hardware timing evidence. |
| <a id="ep-d-8"></a>EP-D-8 | **Names follow settled ownership and keep facts, intent, and results distinct.** `topology-model` and `topology-planner` are settled. Platform-specific crates use `windows-`; only direct low-level API wrappers use `-sys`; role names are preferred over generic `adapter`; and public nouns must distinguish physical facts, developer constraints, and allocation-specific plans. Names whose responsibilities depend on EP-R1.7 remain explicitly open rather than being chosen by the first implementation. |
| <a id="ep-d-9"></a>EP-D-9 | **Windows fact coverage includes processor, memory, relation, and I/O endpoint attachment observations, but not planning policy or measured cost.** The inward component scopes and translates those facts into the neutral model. Storage and network endpoints are first-class resources attached to observed NUMA domains; any measured cost remains contextual under [EP-D-10](#ep-d-10); logical pipeline partitions assign responsibilities without becoming hardware facts. |
| <a id="ep-d-10"></a>EP-D-10 | **Topology-first planning with a small bounded startup cost, not workload validation.** Discovery and application constraints drive matching and realization; zero active probes is supported. Optional probes resolve named ambiguities within the startup budget. Offline experimental sweeps are not a deployment prerequisite. |

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

### Coverage in the shipped Windows fact surface

`MachineMemoryTopology::shard_set()` now assembles the processor facts needed by points 1 through 4:
`(group, number)` identity, online state, optional core membership, SMT state, and efficiency class.
Every absence whose meaning matters is explicit rather than encoded by a sentinel.

The same view carries CPU-set observations for parked state and explicit allocation to the target
process. Those fields are facts reported by the platform, not a `usable()` policy judgment, and the
owning crate documents that the allocation flags carried no information on the Windows build it
measured. The inward component preserves the observations and their provenance. The planner decides
eligibility from the selected universe, caller constraints, and any permitted runtime evidence; it
must not reinterpret an uninformative flag as proof that a processor is unavailable.

Historical absence and sentinel findings that drove this surface are retained in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-1-shipped-coverage-history).

### Partial core coverage is a real state, not a corruption

A processor in no `Core` domain is a firmware gap, not a contradiction, and the topology crate
tolerates it deliberately. The planner must therefore decide what to do with a processor it cannot
group -- it is a candidate host whose SMT relationships and class are unknown, which is exactly the
"unanswered query" case that [CHECKLIST.md](CHECKLIST.md) EP-1.4 owns. It is named here so that
item is not written as though the case were hypothetical.

### What this asks of the neutral topology model

- Availability observations and their provenance remain expressible without becoming a `usable()`
  judgment.
- Efficiency class has exactly one representation and distinguishes "class zero" from "not known".
- Core membership admits that a processor may be in no core without treating that state as an
  error.
- Planner eligibility is a separate result derived from facts, constraints, and permitted runtime
  evidence.

## EP-D-2: the proximity query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.2.*

### What the planner is choosing

For two domains, what connects them: a dedicated SPSC ring, a shared MPSC ring fanning several
producers into one consumer, or a routed hop through an intermediate domain. That choice is made
once per pair, and it is made from how close the two processors are.

This is the query the original processor-locality model question turned on. It is the relational
half of the processor and memory requirements; [EP-D-9](#ep-d-9) extends the abstract machine with
storage attachment and measured endpoint flows without changing this physical-proximity contract.

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

### The query is total over an input planning universe

The universe is an input. It defaults to every processor represented by the supplied real or mocked
system, and the topology specification may select that full set or a subset for one planning run.
The selected universe is the top element for proximity queries in that scope.

Every non-empty set of processors known to the selected universe therefore shares at least the top,
so the query always has an answer even when no finer physical relation contains them. A processor
outside the selected universe is an invalid query input rather than a fabricated top-level match.
An empty input is invalid because every relation vacuously contains it and no useful proximity
question was asked. Duplicate processor IDs are normalized because proximity is over a set.

The top makes the **query** total. It does not make the relation order total.

### A partial order means the answer may not be a single granularity

If the order is by observed set inclusion rather than by firmware numbering, two granularities can
be **incomparable** -- neither refines the other. The tightest shared granularity is then not
unique, and the honest answer is the set of *minimal* shared granularities, which is almost always
exactly one.

This is a cost, and it is worth naming rather than discovering later: every caller either handles a
multi-element answer or explicitly refuses it. The model never silently takes the first. The
alternative -- forcing a linear order -- means silently discarding a real boundary on a machine
whose levels do not nest, and this repository has been bitten specifically by structure that was
assumed rather than checked.

### Specified partitions constrain planning without becoming locality facts

The topology specification may define its own partitions inside the selected universe. These are
logical constraints or candidate domain boundaries: workload ownership, serialization boundaries,
load-balancing groups, or other developer intent. They may overlap, nest, or cut across physical
cache and memory strata.

Specified partitions participate in determining which plans are admissible, but a proximity query
does not return them as evidence that processors physically share something. The planner combines
the logical and physical dimensions, for example by intersecting a required workload partition with
observed NUMA and cache relations. A synthetic claim about hardware belongs in the supplied abstract
machine or its mocked source, not in a planning constraint.

When a specified partition cheaply cuts across a physical stratum, the planner should report the
observed relationship and likely consequence. It must not call the constraint inappropriate without
knowing its intent: an SPSC edge crossing a NUMA boundary may be a mistake or a deliberate transfer.
The topology-specification contract must therefore carry enough intent to distinguish those cases;
that diagnostic contract is scheduled by [CHECKLIST.md](CHECKLIST.md) `EP-R1.7`.

### Coverage in the shipped Windows fact surface

`MachineMemoryTopology` now exposes the observed relation collection ordered by processor-set
inclusion and derives `proximity()` from it. The answer carries all minimal shared granularities,
their membership through the relation values, whether a finer reported kind failed to cover an
input processor, and processors the platform representation cannot express. The machine itself is
the top when no finer relation covers the query.

The neutral contract is deliberately stricter and differently scoped. Its top is the selected
planning universe rather than necessarily the whole observed machine; processors outside that
universe and empty queries are invalid; duplicates are normalized; and every caller must handle or
refuse multiple incomparable minima explicitly. The inward component therefore translates and
scopes the Windows relation facts rather than exposing the convenience query as the planner's
contract. This is an adapter responsibility, not a defect requiring the policy-free facts crate to
become client-shaped.

Historical text claiming that the Windows model had no proximity answer is retained in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-2-shipped-coverage-history).

## EP-D-3: the residency query
**Startup measurement requirements are constrained by [EP-D-10](#ep-d-10); residency evidence ownership remains current.**

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
description. If a permitted probe fits [EP-D-10](#ep-d-10)'s budget, the planner requests it
through a neutral measurement contract, a platform component executes it, and the result carries
its context. Unknown directed cost does not by itself require a startup workload experiment.

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

**Superseded in component count by [EP-D-7](#ep-d-7); the planner name and adapter boundaries remain current.**

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
  - *inward* -- exposing the traits the planner needs over platform observations, with
    `windows_topology_sys::MachineMemoryTopology` supplying processor and memory facts and separate
    Windows surfaces supplying facts such as storage attachment;
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

**I/O endpoints become representable**, which `windows-topology-sys` D-9 also excluded for storage
-- on the grounds that it "changes the crate's identity from processor topology to system
topology". That exclusion was about *that crate* and still holds. NVMe and network endpoints belong
to the abstract model, which was never scoped to a processor topology.

Open follow-up history for this decision is recorded in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-4-open-follow-ups); current actionable work is tracked
in [CHECKLIST.md](CHECKLIST.md).

### What survives unchanged

[EP-D-1](#ep-d-1), [EP-D-2](#ep-d-2) and [EP-D-3](#ep-d-3) are **requirements**, and requirements
survive a change of binding. Each stated what the planner must know and why; what changes is that
they are now satisfied by traits over an abstract model rather than by methods on a Windows type.
They were written against a real caller, which is what makes them portable in this way.

## EP-D-5: the component layout, and which way dependencies point

**Superseded in component count by [EP-D-7](#ep-d-7); the shared-vocabulary ownership and dependency direction remain current.**

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

- **The platform component and public type names.** Deliberately not settled here; their ownership
  boundaries must be defined before their names. [EP-D-8](#ep-d-8) records the governing principles,
  and [CHECKLIST.md](CHECKLIST.md) `EP-1+.5` owns the decision after `EP-R1.7`.
- **Whether `topology-model` is one crate or eventually two.** The machine description and the plan
  vocabulary are different enough that they might separate later. They are together now because
  splitting on speculation costs more than merging on evidence.

Measurement ownership is no longer open; [EP-D-6](#ep-d-6) assigns the campaign to the planner and
execution to shared platform measurement mechanisms.

## EP-D-6: runtime measurement ownership and its data boundary
**Routine measurement-campaign scope superseded by [EP-D-10](#ep-d-10); ownership, shared mechanisms and evidence boundaries remain current.**

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

The planner identifies unanswered questions for the scenario and allocation and interprets any
permitted measurements. [EP-D-10](#ep-d-10) governs whether it may measure at all and bounds that
work; permission is not an instruction to benchmark candidates. Discovery and application
constraints can produce a plan without active measurement. Missing information remains explicit
rather than extending startup until a workload comparison converges.

The neutral measurement request, result, and evidence vocabulary lives in `topology-model`, so the
planner, probes, and platform implementations share one contract without giving the neutral planner
a Windows dependency. Platform measurement components execute the operations and return observations;
they do not choose queue policy.

### Probes and runtime planning share measurement kernels

Developer-facing probes and the autonomous planner use the same underlying measurement mechanisms.
The probes expose those mechanisms for early dependency evaluation, architecture work, deeper client
benchmarking, and evidence checked into this repository. The planner may invoke a bounded subset
under [EP-D-10](#ep-d-10); sharing mechanisms does not require sharing an experiment's duration or
parameter matrix. A separate runtime implementation would let the repository demonstrate one
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

## EP-D-7: five components and the measurement foundation

*The engineer's decision, 2026-09-18. Recorded by [CHECKLIST.md](CHECKLIST.md) `EP-R1.2`.*

### The five responsibilities

| Component role | Platform | Owns | Depends on |
|---|---|---|---|
| `topology-model` | neutral | Abstract machine, topology specification, concrete plan, planner query traits, measurement request/result/evidence vocabulary | nothing |
| `topology-planner` | neutral | Runtime matching, bounded optional probe selection, policy, interpretation, constraint resolution, concrete-plan construction under [EP-D-10](#ep-d-10) | `topology-model` |
| inward topology adapter | Windows | Discovery and translation of platform-published processor, memory, relation, and I/O endpoint attachment facts into the abstract machine | `topology-model`, `windows-topology-sys`, Windows device, volume, and network APIs |
| measurement foundation | Windows | Active measurement kernels and the live platform backend | `topology-model`, Windows APIs, measured queue and I/O primitives |
| outward realizer | Windows | Construction of threads, buffers, rings, affinities, and other runtime objects from a concrete plan | `topology-model`, runtime crates |

The measurement foundation is not a third adapter. The inward adapter translates already-published
facts and stays cheap and scenario-independent. The realizer consumes a completed plan. Active
measurement is permissioned, scenario-shaped work between those two stages and has a dependency set
of its own.

### The execution flow

The dependency and data flow is:

1. The inward adapter produces an abstract machine from the current Windows topology facts.
2. The planner combines that machine with the client's topology specification.
3. The planner issues neutral measurement requests when the specification permits and the current
   evidence is insufficient.
4. The measurement foundation executes those requests and returns contextual evidence.
5. The planner resolves constraints and produces a concrete allocation-specific plan.
6. The realizer constructs the runtime objects named by that plan.

Neither the measurement foundation nor the realizer depends on `topology-planner`; both consume
shared vocabulary from `topology-model`. A probe depends on the measurement foundation and presents
its mechanisms to a developer, while the planner reaches the same mechanisms through the neutral
measurement contracts.

### Existing probe code is source material, not the production dependency

The current probes already establish useful seams. Placement classification, representative-pair
selection, and NUMA-hop selection are pure functions in
[core_affinity.rs](../windows-placement-probe/src/core_affinity.rs), while the live measurement is
assembled separately. [windows-platform-probes/Cargo.toml](../windows-platform-probes/Cargo.toml)
already depends on `windows-placement-probe` rather than carrying a second placement measurement.

The existing ownership is nevertheless deliberately experimental:
[peer_index_cache.rs](../windows-placement-probe/src/peer_index_cache.rs) says production code must
not call it, and [windows-placement-probe/Cargo.toml](../windows-placement-probe/Cargo.toml) says its
measurement code is not a compatibility surface. Production planning therefore does not depend on a
probe crate. Reusable kernels move into the measurement foundation, and probes become front ends
over that owner.

### Mocked interaction testing and real evidence are different claims

The measurement foundation has an injected platform-operations boundary covering affinity,
allocation, achieved-placement inspection, clocks, threads, queue execution, I/O operations, and
cleanup. Tests can then generate degenerate topology, API error, partial-success, impossible-result,
and malformed-response cases in volume before hardware is involved.

Those tests prove orchestration, validation, refusal, and cleanup. They do not manufacture valid
performance evidence. A timing or coherence result is trusted as hardware evidence only when the
live Windows backend measured it on the machine the result describes.

### Planning consequence

The architecture and dependency order are settled here, but final component-local checklists require
the remaining component names and the contracts that define their first implementable items.
[CHECKLIST.md](CHECKLIST.md) `EP-R1.8` materializes those plans after `EP-R1.5` and `EP-R1.7`;
this dependency is recorded rather than hidden behind provisional crate names.

## EP-D-8: naming follows ownership

*The engineer's decision, 2026-09-18. Recorded by [CHECKLIST.md](CHECKLIST.md) `EP-R1.5` and
`EP-1+.3`.*

### What is settled

The neutral shared-vocabulary crate is `topology-model`, and the neutral policy and campaign crate
is `topology-planner`. The conceptual vocabulary also distinguishes:

- Windows processor and memory topology facts;
- the neutral abstract machine;
- the checked-in topology specification carrying developer intent and constraints;
- the allocation-specific concrete runtime plan.

These distinctions are settled even though the eventual Rust type names are not.

### What remains open

The final crate names for the inward Windows fact source, Windows measurement foundation, and
outward realization/runtime component remain open. So do the exact public names for the abstract
machine, topology specification, measurement requests and evidence, diagnostics, and concrete plan.

Those names depend on responsibilities still being defined by [CHECKLIST.md](CHECKLIST.md)
`EP-R1.7`. The outward component is the clearest example: whether it only instantiates a plan or also
owns higher-level work-item and buffer-flow machinery changes the noun that honestly describes it.
The remaining names are therefore owned by `EP-1+.5`, after `EP-R1.7` and before `EP-R1.8`
materializes component-local plans.

### Naming rules

- A platform-specific crate uses the `windows-` prefix.
- The `-sys` suffix is reserved for direct low-level API wrappers such as
  `windows-topology-sys`; safe policy, measurement, translation, and realization layers do not use
  it.
- Prefer the owned role over the generic word `adapter` when the role is settled. Direction-only
  labels remain useful in architecture discussion but are not automatically crate names.
- Public nouns distinguish physical facts, developer intent, and allocation-specific results rather
  than calling all three a topology.
- A name is not chosen merely because implementation needs an identifier. If the responsibility is
  still open, the naming work stays open with it.

## EP-D-9: shipped Windows facts and first-class I/O endpoint attachment

*The engineer's decision, 2026-09-18. Recorded by [CHECKLIST.md](CHECKLIST.md) `EP-R1.6` and
`EP-1.5`.*

### The fact-source boundary

The Windows fact source provides observations, not eligibility or placement policy:

| Planner need | Windows fact coverage | Responsibility above the fact source |
|---|---|---|
| Candidate processors | Identity, online state, core and SMT membership, efficiency class, memory domain, and CPU-set observations | The inward component maps them into the selected universe; the planner decides eligibility |
| Physical proximity | Ordered relations, incomparable minima, membership, incomplete-coverage evidence, and a whole-machine top | The inward component scopes them to the selected universe; the neutral model owns the planner-facing query contract |
| Memory placement | Processor-to-memory-domain membership and explicit unplaced processors | The planner decides whether an unplaced processor is admissible |
| I/O endpoint attachment | A volume or device may report a NUMA node or proximity domain; a network adapter may also report NUMA affinity and receive-steering configuration | The inward component reconciles workload-visible storage and network endpoints with those observations and preserves unknown or ambiguous attachment |
| Directed processor, memory, storage, and network cost | No timeless fact is claimed | Preserve unknown cost; any runtime probe is optional and bounded by [EP-D-10](#ep-d-10) |
| Developer partitions and intent | Not a platform fact | The topology specification constrains admissible plans |

`windows-topology-sys` remains the policy-free processor and memory fact layer. The inward component
may use additional Windows device, volume, and network discovery surfaces for I/O endpoints; that
does not broaden `windows-topology-sys` into a system-topology or planning crate.

The documented Windows inputs include
[`FSCTL_QUERY_VOLUME_NUMA_INFO`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/fsctl-query-volume-numa-info)
for the current NUMA node of a volume and
[`DEVPKEY_Numa_Proximity_Domain`](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/devpkey-numa-proximity-domain)
for a device instance's firmware proximity domain. The latter can be mapped to a Windows node with
[`GetNumaProximityNodeEx`](https://learn.microsoft.com/en-us/windows/win32/api/systemtopologyapi/nf-systemtopologyapi-getnumaproximitynodeex).
For network adapters,
[`Get-NetAdapterRss`](https://learn.microsoft.com/en-us/powershell/module/netadapter/get-netadapterrss)
exposes receive-side scaling configuration, while the standardized
[`*NumaNodeId`](https://learn.microsoft.com/en-us/windows-hardware/drivers/network/standardized-inf-keywords-for-rss)
describes the preferred node for adapter memory allocation and RSS processor preference.

### I/O endpoints are topology resources

An I/O endpoint used by a workload is represented in the neutral abstract machine with stable
identity for the planning run and an observed attachment to a NUMA or memory domain when Windows can
report one. Absence and ambiguity remain explicit.

The common endpoint vocabulary covers identity, attachment, direction, queue capabilities, buffer
domains, and observed steering. Typed capability records preserve distinctions the common shape
must not erase. A storage controller, namespace, volume, file-system path, virtual disk, and
composite volume are not assumed to be interchangeable identities. A network interface additionally
has receive/transmit direction, RSS processor sets and indirection, interrupt or completion
behavior, and offload capabilities. The inward component records only reconciliations and
capabilities it can establish.

The attachment is comparatively stable physical evidence and belongs in discovery for each runtime
planning campaign. It is not durable client configuration: device paths, volume composition,
network configuration, virtualization, hot-plug state, and the process allocation may differ on the
next run.

### Logical pipeline partitions describe work, not hardware

The topology specification may partition a pipeline across I/O endpoints and logical workload
stages. For example, one NVMe endpoint and domain may own reading and parsing while another owns
collation, formatting, and output. A network pipeline may place receive processing and parsing near
one NIC, transform elsewhere, and place transmit processing near another NIC. A mixed pipeline may
flow from NIC receive through processing to NVMe write, or from NVMe read through formatting to NIC
transmit. These partitions express ownership, serialization, throughput, and transfer intent. They
do not become evidence that two resources are physically close.

The planner combines that intent with discovered attachment and any available contextual cost
evidence, without requiring a cost measurement before matching. It may
place completion handling, buffers, parsing, workers, and output near their endpoints and make a
cross-domain handoff explicit where the pipeline requires one. The higher-level work-item and
buffer-flow contract that makes those stages executable remains scheduled by
[CHECKLIST.md](CHECKLIST.md) `EP-R1.7`.

### Discovery does not establish measured cost

A reported home NUMA node establishes attachment, not the cost of every route to the endpoint.
Queue form, queue depth, transfer direction, buffer residency, filesystem and volume layers, RSS
indirection, flow distribution, interrupt and completion steering, offloads, current load, and
completion processing can change the observed result. This limits the performance claims discovery
supports; it does not require startup to measure endpoint, queue, buffer and worker combinations.
[EP-D-10](#ep-d-10) governs optional runtime probes. Offline experiments may characterize these
combinations, with contextual evidence kept distinct from discovered attachment.

## EP-D-10: topology-first planning and bounded startup cost

**Offline exception:** the engineer has authorized [CHECKLIST.md](CHECKLIST.md) -> `EP-X2.1`
with its generated-buffer prerequisite and local captures while `EP-R1.7.1` remains open.
The subsequent `EP-X2.2` request/reply experiment is also explicitly authorized offline,
including its deterministic steady/burst traces, under its
[DESIGN-NOTES.md](experiments/request-reply/DESIGN-NOTES.md). Other work stays paused.
The engineer subsequently authorized `EP-X2.3` as a separate stateful comparison in
that component under `RR-D5`, preserving the stateless paths and all startup constraints.
Physical NUMA fidelity is tracked once under [EP-D-11](#ep-d-11), not as a completion
gate on that item. This does not change the startup constraint.

The planner matches the application design and its constraints to the architecture available to
the process. Processor and memory domains, endpoint attachment, ownership, ordering and permitted
parallelism drive the arrangement. Runtime planning remains the primary path; runtime workload
validation does not become its prerequisite.

Discovery, matching and realization together must have an extremely small, explicitly bounded
pre-execution cost. This is not a constant-time complexity claim: discovery depends on the machine
description, matching on the application graph, and setup on the resources being constructed.
It must not scale with running representative workloads, validating mixes of sizes, waiting for
stable throughput/latency distributions, or sweeping candidate configurations. Moving such work
from the planner into realization does not satisfy this constraint.

A plan with no active probe is a supported outcome, not merely an unvalidated fallback. Any probe
must answer a named ambiguity that could affect the arrangement, be permitted, and fit within
the small startup budget. If that cannot be done, preserve the uncertainty and apply the explicit
missing-evidence policy; do not expand the budget or start workload validation. A required fact
for correctness or authorization is not replaced by a guessed value. An unmet demand for measured
performance cannot be silently promoted to a guarantee.

Endpoint attachment and application-owned constraints anchor placement. Buffer residency, worker
placement, partitioning and transfers are planned around them. Unknown attachment remains unknown;
reported proximity is not a prohibition on a deliberate remote transfer. Storage attachment and
network receive steering remain distinct facts under [EP-D-9](#ep-d-9).

Application-supplied granularity and service information are inputs, not obligations to measure
every size mix. Offline research may test which dimensions change architectural choices; detailed
tuning is separate, explicitly requested work, not an implicit phase before the application starts.
Existing experimental paths and captures remain intact. The five-component architecture and shared
measurement mechanisms remain separate; the startup restriction does not remove their capabilities.

No numerical time/probe cap has been agreed. Specify and test that cap, including zero-probe
planning, setup accounting, exhaustion and unknown-evidence outcomes, under
[CHECKLIST.md](CHECKLIST.md) -> `EP-R1.7.1` before further MX execution. That item also reconciles
the research queue with this boundary; no remaining experiment is silently cancelled or completed.
Rationale is recorded in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-10-startup-cost-clarification).

## <a id="ep-d-11"></a>EP-D-11: one consistent faux-NUMA environment and one fidelity limitation

**Physical NUMA fidelity is unverified: multi-node hardware is unavailable. This is
a shared, non-blocking qualification, not a repeated work-item or milestone gate.**

Use one explicit faux-NUMA environment throughout a test. Inject its observations at
the data-gathering boundary, then have discovery consumers, placement selection,
allocation/binding operations and residency queries use the same logical identities
and state. Fake realization records requested operations and produces controlled
success, refusal, unknown-residency and mismatch outcomes from that environment.
It must not independently invent a topology or silently consult the host's real
node membership after synthetic discovery. Synthetic identities never reach live
Windows affinity or NUMA allocation calls.

Test ordinary and adversarial layouts deterministically: multiple and sparse node
identities, processor groups, shared/separate/ambiguous caches, known and missing
endpoint attachment where the owning model supports it, both transfer directions,
resource bounds and cleanup after failure. Assert resulting decisions and requests,
not only that synthetic input can be deserialized or passed to a selector. A supplied
distance value can test how code uses that value; it cannot establish a physical
memory-access cost. Do not add artificial sleeps or timing penalties and call them
NUMA fidelity.

Faux-NUMA execution validates behavior under the injected model. It does not mimic
memory-distance effects, bandwidth, contention, cache coherence costs, device DMA
locality or the OS's physical placement behavior. Outputs identify the synthetic
environment; do not report fake residency or timings as hardware observations.
Live single-node integration tests remain separate and retain their actual scope.

An item or milestone can complete when its implementation and specified behavioral
tests pass under this policy and any non-NUMA obligations are met. Missing physical
hardware alone neither keeps it open nor creates a duplicate hardware follow-up.
Conversely, an unimplemented fake boundary or an uncovered behavior is real pending
software work; this decision does not mark it tested. Performance baselines, size-mix
validation and universal tuning thresholds are not goals of this acceptance policy.
An explicitly requested future performance claim must name its evidence separately.

[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ep-r172) records the shared faux environment
and adoption by the current experiment's actual consumers. Future planner/realizer
adoption remains queued in [CHECKLIST.md](CHECKLIST.md) -> `EP-R1.8`. Existing synthetic
selector tests are supplemented by the gathering-to-scheduler/fake-realization tests;
this does not claim tests of production components not yet implemented.
`EP-HW.1` is the single later physical-NUMA follow-up, triggered by hardware access.
It checks the real gathering/realization boundaries against behavior tested with
faux NUMA and records discrepancies; it is not a prerequisite for other milestones.
Existing captures and paths remain retained, with no hardware-timing obligation
silently converted into a claimed measurement.

This supersedes EP-X2.1's previous mandatory cross-NUMA-timing closure requirement,
not the limitations of its recorded observations. Rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-11-shared-numa-fidelity).
