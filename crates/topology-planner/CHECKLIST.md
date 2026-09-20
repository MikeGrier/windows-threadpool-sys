# Checklist: the topology planner

Plans an arrangement of execution domains from a stated **goal** plus an **abstracted idealized**
description of a machine. See [COMPONENT.md](COMPONENT.md) for what this component is and why it is
separate from both the topology crate and the runtime, and
[EP-D-4](DESIGN-NOTES.md#ep-d-4) for the architecture it now sits in.

**The component has been re-scoped**, per [EP-D-4](DESIGN-NOTES.md#ep-d-4) through
[EP-D-7](DESIGN-NOTES.md#ep-d-7). It is named `topology-planner` and the directory now matches; it
queries an abstract model covering processors, memory, storage, interconnects, distances and
bottlenecks rather than `MachineMemoryTopology` directly; and **adapters** bracket it -- one exposing
the model's traits over the Windows topology objects, one realizing a plan as buffers, rings and
threads. The model, its traits, and the plan type live in a separate `topology-model` crate that
everything depends on and that depends on nothing. A fifth component owns active Windows measurement
kernels shared by the probes and runtime planner.
**M2+ and M3+ are written against the superseded shape and are not yet re-cut**; MR3 owns
that re-cut.

## Where this stands

**The production planner is not implemented.** MR2 is the active milestone: it settles the planner
contract, and MX1 through MX3 are the experiments that inform it. The isolated read/checksum
baseline is recorded in its
[COMPLETED-CHECKLIST.md](experiments/read-checksum/COMPLETED-CHECKLIST.md#rc-1).
The two crates under `experiments/` are the only code this component owns, and they are workspace
members.

**No longer coupled to `windows-topology-sys`.** PR #56 closed unmerged and that crate's reshape
shipped separately; per [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21) it publishes a refined
view of what the platform publishes, and an **adapter** absorbs whatever this component needs beyond
it.

The design session that gated M2+ onward **has concluded** -- its questions were answered as `D-13`
through `D-21`, and the `MMT-*` plan is what it produced. M2+ is now gated on this component's own
prerequisites rather than on someone else's decision.

| Milestone | State | What it is waiting on |
|---|---|---|
| MR2 the planner contract | active | `EP-R1.7.1` first; it gates the rest of MR2 and all MX execution |
| MX1 read/checksum comparisons | paused | `EP-X1.2`'s result discussion, held under `EP-R1.7.1` |
| MX2 locality and ownership | complete | nothing; its stubs migrate when the group is archived |
| MX3 real endpoints and composition | not started | MX1/MX2 evidence, an implementation component (`EP-X3.1`), and the permissions each item names |
| MR3 component-local plans | parked | MR2 |
| M2+ the plan as a value | parked, **and needs re-cutting** | MR3 |
| M3+ the policies | parked, **and needs re-cutting** | M2+ |
| MH1 physical NUMA fidelity | parked, ungated | `EP-HW.1` triggers on hardware access; it gates no other item or milestone |
| M-inf horizon | parked, ungated | deliberately not scheduled |

## MR2: settle the planner contract

Item IDs keep the `EP-R1.7.x` sub-numbering this milestone was decomposed from, because `EP-R1.7.2`
is already archived under that ID. The milestone also owns what the former `M1+` items and `EP-1.4`
described.

**Renumbered:** `EP-1+.1` -> `EP-R1.7.3`, `EP-1+.2` -> `EP-R1.7.4`, `EP-1.4` -> `EP-R1.7.5`,
`EP-1+.5` -> `EP-R1.7.11`. A document citing one of those, or citing `EP-R1.7` as a single item,
means this milestone. `EP-1+.3` and `EP-1+.4` were already complete and keep their archived IDs.

[EP-D-10](DESIGN-NOTES.md#ep-d-10) bounds every decision here: topology-first matching inside a small
startup budget, zero active probes supported, and no workload validation as a deployment
prerequisite. Use [DESIGN-PROPOSAL-EP-R1.7.md](DESIGN-PROPOSAL-EP-R1.7.md) as a working basis rather
than a frozen contract, and the primary-source survey in
[DESIGN-RESEARCH-WORKLOAD-PATTERNS.md](DESIGN-RESEARCH-WORKLOAD-PATTERNS.md) as the starting point.
The full contract need not be settled before experiments inform it.

MX1 through MX3 are this milestone's evidence, not a separate program. Each item below names the
experiments it needs; an item naming none is settled on design grounds. Resolve and commit one item
before beginning the next.

- [ ] **EP-R1.7.1** -- **Reconcile the topology-first contract and experiment sequence with the
  bounded startup cost in [EP-D-10](DESIGN-NOTES.md#ep-d-10).** Specify an explicit small
  budget across discovery, matching, optional probes and realization; record its units, limits,
  permitted operations and exhaustion behavior without requiring representative workload runs
  or size-mix validation. Define zero-probe acceptance and handling of unknown cost versus missing
  correctness facts. Separate offline architectural research from explicitly requested tuning,
  revisit the five-workload progression and every remaining MX dependency, and preserve all
  unperformed work with its agreed scope and next action. Reconcile the working proposal's
  candidate-selection, measurement-grant and ready-plan rules; queue executable acceptance for
  zero probes, budget exhaustion and no hidden workload validation during setup. Settle EP-X1.2's
  disposition and the next experiment with the engineer before resuming MX execution.
  **This item gates every other item in this milestone and all MX execution.**

- [x] **EP-R1.7.2** -- Implement and adopt a consistent faux-NUMA test environment. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-r172)

- [ ] **EP-R1.7.3** -- **Describe the scenario input.** The planner takes *two* inputs and only the
  machine description is specified anywhere. The scenario says what the caller intends to run, and it
  is what makes a measurement meaningful: [EP-D-3](DESIGN-NOTES.md#ep-d-3) established that a measured
  number means nothing without knowing what it measured, so at minimum the scenario must distinguish
  small-message handoff from large-buffer streaming. State what the caller must supply, what is
  optional, what the planner may infer, and what it must refuse to guess. Its absence is why "what is
  most useful for consumers" stayed hard to answer in the abstract.
  **Evidence:** MX1's workload dimensions and MX2's ownership shapes.

- [ ] **EP-R1.7.4** -- **Decide what the caller-callback traits ask.** Planning is a negotiation: the
  component may call back for clarification the scenario did not settle. Enumerating those questions
  is what decides whether this is one trait or several, and it cannot be done before `EP-R1.7.3` says
  what the scenario already answers. Define callback semantics, when they may be invoked, and what a
  refusal, a failure or a timeout means for the resulting plan.

- [ ] **EP-R1.7.5** -- **Decide what the planner does when required evidence is unavailable.** Runtime
  characterization may answer a fact the initial machine observation did not, but measurement can be
  disallowed, fail, exceed its budget, or remain inconclusive. Decide when the planner adapts to a
  documented weaker policy, emits a plan carrying an explicit assumption or unresolved constraint, or
  refuses to plan, and define measurement permission and failure behavior alongside that rule.
  [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21) requires `windows-topology-sys` only to state
  policy-free platform facts and distinguish absence; it does not decide consumer behavior and will
  not be reshaped to match the planner's goal. Keep [EP-D-3](DESIGN-NOTES.md#ep-d-3)'s asymmetry: an
  unknown cache domain costs an optimisation, an unknown memory domain has no honest fallback.

- [ ] **EP-R1.7.6** -- **State the plan value's invariants and errors, its deterministic policy and
  tie-breaking, and its JSON compatibility rule.** The same inputs must produce the same plan, so
  every policy choice needs a stated tie-break rather than an incidental one. Say which JSON changes
  are compatible and which are not, and whether the plan's schema versions separately from the crates
  that carry it.

- [ ] **EP-R1.7.7** -- **Define the common I/O endpoint contract and the typed storage and network
  capabilities over it**, including network receive steering and RSS constraints, and what an endpoint
  must state about the NUMA domain it is attached to.
  **Evidence:** `EP-X3.1` for the network side, `EP-X1.5` for the storage side.

- [ ] **EP-R1.7.8** -- **Decide storage, network and interconnect policy, and how a routed hop is
  represented** both in the plan and in the model the plan is read from.
  **Evidence:** `EP-X2.1`'s directed-transfer results and `EP-X3.2`'s composition.

- [ ] **EP-R1.7.9** -- **Draw the boundary on work-item and buffer-flow machinery, and define
  intent-aware diagnostics.** Decide how much higher-level machinery this project supplies between
  completed I/O, parsing, serial or parallel processing, workers, and cross-domain migration,
  including whether existing repository code or the Windows thread pool already owns any part. Define
  diagnostics for constraints that cross physical cache or memory strata so the planner reports the
  observed consequence without treating a deliberate transfer as inherently wrong.
  **Evidence:** MX2's ownership and ordering results.

- [ ] **EP-R1.7.10** -- **Build the acceptance matrix.** Include at least ten normal cases plus every
  identified edge case, expressed as synthetic cases the planner can actually be run against. Trace
  each accepted and rejected case to an experiment or to an explicit untested design obligation, and
  replace every "deferred for litigation" with a linked decision or a concrete blocker and graduation
  trigger.
  **Evidence:** `EP-X3.3`, which synthesizes the experimental constraints this matrix encodes.

- [ ] **EP-R1.7.11** -- **Name the remaining platform components and public contract types, once the
  items above have settled their responsibilities.** Apply [EP-D-8](DESIGN-NOTES.md#ep-d-8): platform
  crates use `windows-`, only direct low-level wrappers use `-sys`, role names are preferred over
  generic `adapter`, and physical facts, developer intent, and allocation-specific results use
  distinct nouns. Complete this before MR3 creates the component-local plans.

## MX1 through MX3: the experiment program for MR2

**`EP-X2.5` is complete, closing MX2; all experiment execution remains paused pending
`EP-R1.7.1`.** Existing captures and completed work are unchanged, and paused items retain the work
they state.

**These three milestones group experiments by workload, not by execution order.** Execution has
already interleaved them -- `EP-X2.1` through `EP-X2.3` ran while `EP-X1.2` was still open -- so an
item's position in this list is not a dependency. Each open item states its own prerequisites, and
the next experiment is chosen at the preceding result discussion under `EP-R1.7.1` rather than by
reading down the page. If evidence changes a dependency or the next useful question, revise this
checklist before continuing.

[EP-D-10](DESIGN-NOTES.md#ep-d-10) remains the startup constraint. The shared NUMA validation policy
is [EP-D-11](DESIGN-NOTES.md#ep-d-11); physical hardware is not an item or milestone gate, and
physical follow-up belongs only to `EP-HW.1`. Faux NUMA establishes behavior, not hardware timings.

Every MX item is owned in this file. Implementation lands in `experiments/read-checksum` or
`experiments/request-reply`, which are separate source-components with their own design notes and
captures, but the checklist item never moves there and those components keep no checklist of their
own -- their plan indexes link back here. Use the
[research](DESIGN-RESEARCH-WORKLOAD-PATTERNS.md) and
[working proposal](DESIGN-PROPOSAL-EP-R1.7.md) as hypotheses, not a fixed archetype catalog.

For each item, specify the workload constraints, candidate arrangements, bounded parameter
matrix, permissions, resource ceilings, stopping conditions and comparison criteria before
implementation. Hold semantics and aggregate resource budgets equal within each paired
comparison; label intentionally unequal controls. Use deterministic reference outcomes and
exercise invalid input, pressure, cancellation and rundown as applicable. Record actual
placement and distinguish synthetic, cached-file and physical-device evidence.

Retain raw observations with source/configuration provenance, balanced run order and
same-code controls. Record inconclusive results without promoting them to a ranking.
Discuss the result and explicitly retain, revise, merge or delete each speculative path;
update the working contract and affected future items, then commit that item before starting
another. Do not wait for the final synthesis to record what an experiment teaches.

If hardware, permission or a missing primitive blocks work, name the blocker and discuss the
workaround or reordering; leave unperformed work unchecked.

## MX1: controlled read/checksum comparisons

- [x] **EP-X1.1** -- Separate repeatability, stage separation and processing parallelism. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-x11)

- [ ] **EP-X1.2** -- **Isolate block size, compute work, read depth, queue capacity and batching.**
  **Result discussion pending:** review the implemented protocol and observations in the
  [capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-2/README.md),
  reconcile scope under EP-R1.7.1, agree the path disposition and next experiment, then close this item.
  The protocol is [DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md#rc-d9-ep-x12-parameter-isolation).
  **Question:** how does each dimension change the comparison, rather than several changing
  together? **Controls:** use `EP-X1.1`'s arrangements and a bounded one-factor-at-a-time
  matrix, adding explicitly selected interactions; match aggregate budgets between candidates
  at each matrix point. Include partial batches and short final blocks.
  Retain all arrangements and both role orientations, balance treatment positions, and bracket
  each sweep with same-code controls per
  [DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md) -> `RC-D8`.
  Compare per-orientation spread before ranking; do not select a sole baseline from EP-X1.1.
  **Evidence:** correctness, useful throughput, stage/queue latency, CPU use, payload
  occupancy and pressure, with the changed dimension recorded. **Review:** retain conditional
  observations and choose subsequent cases without inventing a universal queue or batch size.
  **Prerequisite:** none outstanding -- the sweep is implemented and captured, and only the result
  discussion remains.
  > **-> CROSS-COMPONENT HANDOFF:** implemented in `experiments/read-checksum`, which keeps the
  > captures and `RC-D9`; the item stays owned here. Return to `MR2` -> `EP-R1.7.1`, which holds the
  > result discussion and chooses what runs next.

- [ ] **EP-X1.3** -- **Compare polling, waiting and bounded hybrid idle policies.**
  Select its fixed configurations at the EP-X1.2 result discussion using the
  [proposed cases](experiments/read-checksum/captures/2026-09-19-ep-x1-2/README.md#proposed-disposition-and-next-cases).
  **Question:** how much of a candidate's CPU use and response time comes from its idle and
  backpressure policy? **Controls:** hold the workload, placement, topology and resource
  bounds from selected `EP-X1.2` cases fixed; apply equivalent policy choices to comparable
  roles, including empty input, full queues and an intermittently stalled processor.
  **Evidence:** CPU time, wake/queue latency, progress and shutdown under pressure; verify
  the real notification path cannot lose work or a wakeup. **Review:** separate policy
  effects from arrangement effects and retain the measured policy choices explicitly.
  **Prerequisite:** `EP-X1.2`'s result discussion, which selects this item's fixed configurations.
  > **-> CROSS-COMPONENT HANDOFF:** implemented in `experiments/read-checksum`; the item stays owned
  > here. Return to `MR2` -> `EP-R1.7.1` for the result discussion.

- [ ] **EP-X1.4** -- **Measure independently paced arrivals and overload behavior.**
  Reuse the deterministic steady/burst trace and outcome-accounting contract from
  [DESIGN-NOTES.md](experiments/request-reply/DESIGN-NOTES.md) -> `RR-D2` where applicable;
  EP-X2.2's completion does not supply this read/checksum workload's overload comparison.
  **Question:** what happens below, near and beyond the service rate, including bursts?
  **Controls:** use deterministic scheduled-arrival traces and selected `EP-X1.2`/`EP-X1.3`
  configurations; keep resource limits and the admission/rejection contract equal.
  **Evidence:** offered, admitted, completed, rejected and cancelled identities; latency
  from scheduled arrival, including generator lateness and pre-admission waiting; queue
  pressure and recovery after a burst. **Review:** distinguish throughput from responsiveness,
  account for unfinished work, and refine the workload's admission/deadline constraints.
  **Prerequisite:** the `EP-X1.2` and `EP-X1.3` configurations this item holds fixed.
  > **-> CROSS-COMPONENT HANDOFF:** implemented in `experiments/read-checksum`, reusing
  > `experiments/request-reply`'s `RR-D2` accounting contract; the item stays owned here. Return to
  > `MR2` -> `EP-R1.7.1` for the result discussion.

- [ ] **EP-X1.5** -- **Separate generated-buffer, buffered-file and device-path evidence.**
  The generated-buffer companion this needs was implemented under `EP-X2.1` and is complete, so that
  prerequisite is already satisfied; this item still owns the buffered working-set and authorized
  device-path comparisons and their evidence review.
  **Question:** which differences remain when file/cache service is changed or removed?
  **Controls:** retain equal transforms, logical inputs and per-comparison budgets; compare
  a labelled generated-buffer companion, buffered working-set sweeps and an explicitly
  authorized device-path experiment with its alignment/cache conditions established.
  **Evidence:** the actual I/O mode, working set, completed bytes, CPU/latency distributions
  and observed cache/device conditions; do not infer physical service from a mode flag alone.
  **Review:** identify which claims each evidence class supports and what remains unmeasured.
  **Prerequisite:** device-path access must be explicitly authorized before that comparison runs;
  the generated-buffer and buffered working-set halves are not blocked on it.
  > **-> CROSS-COMPONENT HANDOFF:** implemented in `experiments/read-checksum`; the item stays owned
  > here. Return to `MR2` -> `EP-R1.7.1` for the result discussion, and to `EP-R1.7.7`, which this
  > item's storage evidence feeds.

## MX2: locality and ownership patterns

- [x] **EP-X2.1** -- Validate placement and directed buffer-transfer behavior. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-x21)

- [x] **EP-X2.2** -- Compare shared completion service with assigned request lanes. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-x22)

- [x] **EP-X2.3** -- Compare shared state with key-owned processing. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-x23)

- [x] **EP-X2.4** -- Compare serial and staged ordered ingestion. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-x24)

- [x] **EP-X2.5** -- Exercise scatter/gather and broadcast/branch/join constraints. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-x25)

## MX3: real endpoints, composition and contract synthesis

- [ ] **EP-X3.1** -- **Compare network receive and application-processing arrangements.**
  **Question:** how do completion-side processing, staged processing and per-flow lanes
  behave with real stream input? **Controls:** use declared framing and per-flow order,
  matched bytes/transforms/resource bounds and reproducible steady/burst traffic; record
  receive steering separately from application bindings and payload residency.
  **Evidence:** frame/correlation correctness, fragmentation, disconnect/cancellation,
  backpressure and latency/CPU observations. Label loopback separately from physical-NIC
  captures; establish RSS/adapter observations and endpoint permissions before physical runs.
  **Review:** identify which network-specific capabilities belong beside the common I/O
  endpoint contract, without assuming device receive processing determines application ownership.
  **Prerequisite:** endpoint permissions, and RSS/adapter observations established before any
  physical-NIC run; loopback needs neither.
  > **-> CROSS-COMPONENT HANDOFF:** **MX3 has no implementation home yet.** This needs a new
  > experiment source-component beside `read-checksum` and `request-reply`. Name it and record the
  > reciprocal handoff at the result discussion that selects this item, before any code is written.
  > Return to `MR2` -> `EP-R1.7.7`, which this item's network evidence feeds.

- [ ] **EP-X3.2** -- **Compose a multi-endpoint transform/collate/output workload.**
  **Question:** what changes when source, processing, aggregation and destination placement
  interact in the motivating complex example? **Controls:** build from MX2's validated
  dependencies and `EP-X3.1`'s endpoint distinctions; start with a reduced composition and
  extend to separately observed storage/network endpoints. Compare shared, endpoint-owned
  and intentionally transferring arrangements with identical logical results, ordering,
  output-effect contracts and aggregate budgets.
  **Evidence:** end-to-end identities/effects, achieved endpoint/worker/buffer placement,
  per-stage pressure, directed transfers and recovery from one slow or failed endpoint.
  **Review:** record which constraints survive composition; do not claim independent devices
  or cross-NUMA behavior from multiple logical handles to one underlying resource.
  **Prerequisite:** `EP-X3.1`'s endpoint distinctions and MX2's validated dependencies.
  > **-> CROSS-COMPONENT HANDOFF:** hosted by the component `EP-X3.1` establishes. Return to `MR2`
  > -> `EP-R1.7.8`, which this item's composition evidence feeds.

- [ ] **EP-X3.3** -- **Synthesize the experimental constraints into the working planner contract.**
  **Question:** what must a workload description and candidate plan express to reproduce
  the tested legal arrangements? **Controls:** replay representative recorded descriptors
  against deterministic candidate construction and a shared constraint validator; include
  accepted and rejected arrangements, unavailable evidence and bounded search exhaustion.
  **Evidence:** trace each proposed constraint and acceptance case to an experiment or an
  explicit untested design obligation; keep measured costs allocation-specific.
  **Review:** reconcile the [working proposal](DESIGN-PROPOSAL-EP-R1.7.md), canonical decisions
  and acceptance matrix, and record retain/revise/merge/delete dispositions without freezing
  the catalog merely because this sequence ended.
  **Prerequisite:** the MX1 and MX2 evidence each contract item names; MX3's own two experiments
  where composition constraints are in scope.
  > **-> CROSS-COMPONENT HANDOFF:** return to `MR2` -> `EP-R1.7.10`, which this item's synthesis
  > becomes. MR2's remaining contract items close before MR3 begins.

## MR3: materialize the component-local plans

Parked on MR2. Nothing here starts because one experiment or one contract item has finished.

- [ ] **EP-R1.8** -- **Materialize executable component-local plans once MR2's names and contracts
  are settled.** Create dependency-ordered checklists and reciprocal cross-component handoffs for
  `topology-model`, `topology-planner`, the inward adapter, the measurement foundation, and the
  realizer. Move the model, specification, query-trait, measurement-vocabulary, and plan-value work
  to their owning component. Each plan must identify its first implementable item without a
  provisional crate name or an unstated contract prerequisite.
  Carry `EP-R1.7.2`'s consistent gathering/fake-realization acceptance into each owning
  component, including zero leakage of synthetic identities into live placement calls.

## M2+: the plan as a value

**Written against the superseded pre-[EP-D-4](DESIGN-NOTES.md#ep-d-4) shape; MR3 re-cuts these items.**

Parked, not pending. The Windows topology reshape has already shipped. This work waits on MR2, then
MR3's component-local plans, and implementation of the resulting shared `topology-model`
contracts. Shape recorded so it is not lost, per the `M{n}+` convention.

- [ ] **M2+.1** -- The plan type: domains, each with its processor, its memory domain and its
  channels; inspectable and comparable, constructible against a synthetic topology so a machine
  nobody has can be planned for and reviewed.

- [ ] **M2+.2** -- Rendering a plan for a human to read before anything is pinned or allocated,
  including which queries were unanswered and what was assumed in their place.

- [ ] **M2+.3** -- Validation against synthetic topologies drawn from the shapes this repository has
  actually met: the ARM64 host with no L3, the x64 host whose outermost partitioning cache is L2
  shared by SMT siblings, a hybrid part with efficiency classes, and a machine with more than 64
  processors so the group boundary is exercised rather than assumed.

## M3+: the policies

**Written against the superseded pre-[EP-D-4](DESIGN-NOTES.md#ep-d-4) shape; MR3 re-cuts these items.**

Parked. These are the choices the component exists to make, and each is a decision item rather than
an implementation one.

- [ ] **M3+.1** -- Domain-per-core versus domain-per-thread, and whether efficiency cores are peers,
  excluded, or a second tier.

- [ ] **M3+.2** -- The channel policy: what proximity justifies SPSC, what falls back to MPSC, and
  whether any pair is deliberately not connected directly at all.

- [ ] **M3+.3** -- Buffer residency for a channel spanning two memory domains, which the placement
  probe already measures and which has no default that is right on both sides.

## MH1: shared physical-NUMA validation (external hardware gate)

- [ ] **EP-HW.1** -- **Validate physical NUMA fidelity when a multi-node host becomes available.**
  This is the single shared follow-up under [EP-D-11](DESIGN-NOTES.md#ep-d-11), outside
  the software-completion path. Exercise actual gathering, node identities, affinity,
  allocation, observed residency, both transfer directions and failure handling against
  the same behavioral contracts as the faux environment. Record host-specific discrepancies
  and fix the owning layer. Hardware access is the trigger; no existing or future item
  waits on it solely for NUMA fidelity. Do not require performance baselines or invent
  memory-distance criteria; a separately requested performance claim needs its own scope.

## M-inf: parked, ungated

- [ ] **M-inf.1** -- Re-planning at runtime, when processors are parked, hot-added, or the process
  is given a different CPU-set allocation than it started with. Deliberately not scheduled: it needs
  the static case to exist first, and it is a different problem.
