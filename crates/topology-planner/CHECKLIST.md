# Checklist: the topology planner

Plans an arrangement of execution domains from a stated **goal** plus an **abstracted idealized**
description of a machine. See [COMPONENT.md](COMPONENT.md) for what this crate is and why it is
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
**M2+ onward are written against the superseded shape and are not yet re-cut.**

## Where this stands

**The production planner is not implemented.** MR1 is the active design milestone.
Its `EP-R1.7` experiment program is itemized here as MX1 through MX3; the isolated
read/checksum baseline is recorded in its
[COMPLETED-CHECKLIST.md](experiments/read-checksum/COMPLETED-CHECKLIST.md#rc-1).
M1 remains a *requirements* milestone rather than an implementation one.

**Deferred past PR #56, by direction.** This component contributes only planning documents to that
PR and no code. `windows-topology-sys`'s reshape lands there without it: per
[D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21) that crate publishes a refined view of what the
platform publishes, and an **adapter** absorbs whatever this component needs beyond it, so the two
are no longer coupled.

The design session that gated M2 onward **has concluded** -- its questions were answered as `D-13`
through `D-21`, and the `MMT-*` plan is what it produced. M2+ is now gated on this component's own
prerequisites rather than on someone else's decision.

| Milestone | State | What it is waiting on |
|---|---|---|
| MR1 design-review reconciliation | active | EP-R1.7.1 reconciles the EP-D-10 startup-cost boundary before further MX work |
| MX1 controlled read/checksum comparisons | paused for scope reconciliation | EP-X1.2 evidence retained; EP-R1.7.1 precedes closure or further execution |
| MX2 locality and ownership patterns | EP-X2.1 complete | Placement behavior validated with EP-R1.7.2; later work remains paused by EP-R1.7.1 |
| MX3 real endpoints, composition and synthesis | not started | MX2 ownership/ordering evidence and the permissions/hardware named by each experiment |
| M1 the input contract | 4 done, 1 open | `EP-1.4` waits on EP-R1.7 |
| M1+ scenario and naming | 2 done, 3 open | EP-R1.7 owns the topology specification and callbacks; `EP-1+.5` then settles remaining names |
| M2+ the plan as a value | parked, **and needs re-cutting** | MR1, then EP-R1.8 and the resulting `topology-model` work |
| M3+ the policies | parked | M2+ |
| M-inf parked | ungated | not scheduled, deliberately |
| MH1 physical NUMA fidelity | external hardware unavailable | EP-HW.1 is the single shared follow-up; does not gate other items or milestones |

## MR1: reconcile the 2026-09-18 design review

These items are in dependency order. Resolve and commit one item before beginning the next.
For `EP-R1.7`, execute and discuss the MX experiment items below before closing the parent
item; do not start `EP-R1.8` merely because one experiment has finished.

- [x] **EP-R1.1** -- Runtime measurement ownership and its data boundary are settled. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r11)

- [x] **EP-R1.2** -- The five-component architecture and its dependency order are settled. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r12)

- [x] **EP-R1.3** -- Stale external gates are replaced by the component's real internal dependencies. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r13)

- [x] **EP-R1.4** -- Query totality, the scoped universe, and specified partitions are distinguished from order totality. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r14)

- [x] **EP-R1.5** -- Settled naming is separated from contract-dependent component and type names. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r15)

- [x] **EP-R1.6** -- Tier 1 now distinguishes shipped Windows facts, adapter projections, I/O endpoint attachment, and planner-owned measurements. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r16)

- [ ] **EP-R1.7** -- **Develop the contracts and acceptance matrix through research and bounded experiments.**
  **In progress:** use [DESIGN-PROPOSAL-EP-R1.7.md](DESIGN-PROPOSAL-EP-R1.7.md) as a working
  basis, not a frozen contract; reconcile it under `EP-R1.7.1` before further experiments.
  [EP-D-10](DESIGN-NOTES.md#ep-d-10) governs startup cost. Start from the primary-source survey and simpler workloads in
  [DESIGN-RESEARCH-WORKLOAD-PATTERNS.md](DESIGN-RESEARCH-WORKLOAD-PATTERNS.md).
  Review the parameter-sweep [capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-2/README.md)
  before selecting the next experiment. Keep stage separation distinct from processing parallelism,
  and account for run-to-run variation before generalizing the catalog. Define each experiment's
  constraints and correctness/measurement obligations; review speculative paths for retention,
  revision, merge or deletion. The full contract need not be settled before experiments inform it.
  > **CROSS-COMPONENT PREREQUISITE:** `experiments/read-checksum` -> `RC` -> `RC-1`
  > supplies the first evidence and returns control here; see
  > [COMPLETED-CHECKLIST.md](experiments/read-checksum/COMPLETED-CHECKLIST.md#rc-1).
  Plan the scenario/goal contract, plan invariants and errors, deterministic policy and tie-breaking,
  measurement permission and failure behavior, callback semantics, JSON compatibility, storage,
  network, and interconnect policy, routed-hop representation, and synthetic acceptance cases.
  Define the common I/O endpoint contract plus typed storage and network capabilities, including
  network receive steering and RSS constraints. Decide how much
  higher-level work-item and buffer-flow machinery this project supplies between completed I/O,
  parsing, serial or parallel processing, workers, and cross-domain migration, including whether
  existing repository code or the Windows thread pool already owns any part. Define intent-aware
  diagnostics for constraints that cross physical cache or memory strata, so the planner reports the
  observed consequence without treating a deliberate transfer as inherently wrong. Include at least
  ten normal cases plus every identified edge case, and replace "deferred for litigation" with a
  linked decision or a concrete blocker and graduation trigger.
  Use MX1 through MX3 below as the experiment work queue, revising it as evidence changes the plan.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/topology-planner/experiments/read-checksum`
  > -> `MX1` -> `EP-X1.2` has returned its capture here for result discussion.
  > Reconcile scope under `EP-R1.7.1` before closing that review or selecting the next experiment; its
  > [PLANS.md](experiments/read-checksum/PLANS.md) points back here.

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

- [x] **EP-R1.7.2** -- Implement and adopt a consistent faux-NUMA test environment. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-r172)

- [ ] **EP-R1.8** -- **Materialize executable component-local plans after names and contracts are
  settled.** Create dependency-ordered checklists and reciprocal cross-component handoffs for
  `topology-model`, `topology-planner`, the inward adapter, the measurement foundation, and the
  realizer. Move the model, specification, query-trait, measurement-vocabulary, and plan-value work
  to their owning component. Each plan must identify its first implementable item without a
  provisional crate name or an unstated contract prerequisite.
  Carry EP-R1.7.2's consistent gathering/fake-realization acceptance into each owning
  component, including zero leakage of synthetic identities into live placement calls.

## Experiment execution rules for MX1 through MX3

**Further experiment execution is paused pending `EP-R1.7.1`; the authorized EP-X2.1 is complete.**
[EP-D-10](DESIGN-NOTES.md#ep-d-10) remains the startup constraint. The shared NUMA
validation policy is [EP-D-11](DESIGN-NOTES.md#ep-d-11); physical hardware is not an item
or milestone gate. Other items retain their work and remain
paused. Existing captures and completed work are unchanged.

Keep this file as the experiment program's work queue; component plan indexes link to
the owning items here rather than duplicating them in another checklist. Use the
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

Treat the order below as the current execution plan, not a promise that every hypothesis
survives. If evidence changes a dependency or the next useful question, revise this checklist
before continuing. If hardware, permission or a missing primitive blocks work, name the
blocker and discuss the workaround or reordering; leave unperformed work unchecked.
Faux NUMA establishes behavior, not hardware timings. Do not create per-item hardware
completion gates for the shared limitation; physical follow-up belongs only to EP-HW.1.
Actual software gaps and other external-boundary requirements remain pending. Before work moves into
another source-component, record the exact destination and reciprocal handoff against the
same item ID; do not create an unowned implementation queue.

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
  > **CROSS-COMPONENT PREREQUISITE:** parent `topology-planner` -> `EP-R1.7`
  > returns the `EP-X1.1` result review and its `RC-D8` controls before work resumes in
  > `experiments/read-checksum` -> `MX1` -> `EP-X1.2`.
  > **-> CROSS-COMPONENT HANDOFF:** return from `experiments/read-checksum` to
  > `topology-planner` -> `EP-R1.7` before advancing to `EP-X1.3`.

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
  > **-> CROSS-COMPONENT HANDOFF:** return from `experiments/read-checksum` to
  > `topology-planner` -> `EP-R1.7` before advancing to `EP-X1.4`.

- [ ] **EP-X1.4** -- **Measure independently paced arrivals and overload behavior.**
  **Question:** what happens below, near and beyond the service rate, including bursts?
  **Controls:** use deterministic scheduled-arrival traces and selected `EP-X1.2`/`EP-X1.3`
  configurations; keep resource limits and the admission/rejection contract equal.
  **Evidence:** offered, admitted, completed, rejected and cancelled identities; latency
  from scheduled arrival, including generator lateness and pre-admission waiting; queue
  pressure and recovery after a burst. **Review:** distinguish throughput from responsiveness,
  account for unfinished work, and refine the workload's admission/deadline constraints.
  > **-> CROSS-COMPONENT HANDOFF:** return from `experiments/read-checksum` to
  > `topology-planner` -> `EP-R1.7` before advancing to `EP-X1.5`.

- [ ] **EP-X1.5** -- **Separate generated-buffer, buffered-file and device-path evidence.**
  Use the generated-buffer companion implemented under EP-X2.1; this item still owns the
  buffered working-set and authorized device-path comparisons and their evidence review.
  **Question:** which differences remain when file/cache service is changed or removed?
  **Controls:** retain equal transforms, logical inputs and per-comparison budgets; compare
  a labelled generated-buffer companion, buffered working-set sweeps and an explicitly
  authorized device-path experiment with its alignment/cache conditions established.
  **Evidence:** the actual I/O mode, working set, completed bytes, CPU/latency distributions
  and observed cache/device conditions; do not infer physical service from a mode flag alone.
  **Review:** identify which claims each evidence class supports and what remains unmeasured.
  > **-> CROSS-COMPONENT HANDOFF:** return from `experiments/read-checksum` to
  > `topology-planner` -> `EP-R1.7` before advancing to `MX2` -> `EP-X2.1`.

## MX2: locality and ownership patterns

- [x] **EP-X2.1** -- Validate placement and directed buffer-transfer behavior. -> [completed 2026-09-19](COMPLETED-CHECKLIST.md#ep-x21)

- [ ] **EP-X2.2** -- **Compare shared completion service with assigned request lanes.**
  **Question:** how do shared workers and explicit ownership respond to independent
  request/reply work and imbalance? **Controls:** implement both as genuine candidates
  over one backend, with the same request/reply semantics, worker ceiling, outstanding-work
  bound and idle policy; use MX1's steady/burst traces and deterministic uneven service costs.
  **Evidence:** exact request/reply correlation, per-lane and aggregate response distributions,
  utilization, admission pressure and rundown. **Review:** distinguish scheduling flexibility
  from ownership locality; do not preselect pinning or invent an MPMC queue from an MPSC API.
  > **CROSS-COMPONENT PREREQUISITE:** `experiments/read-checksum` -> `MX2` ->
  > `EP-X2.1` has returned its [behavioral coverage](COMPLETED-CHECKLIST.md#ep-x21).
  > This item still waits on EP-R1.7.1 scope reconciliation, not physical NUMA hardware.

- [ ] **EP-X2.3** -- **Compare shared state with key-owned processing.**
  **Question:** when do routing and ownership change the cost of a stateful workload?
  **Controls:** use a small counter/lookup service with shared synchronized state versus
  key-owned workers, identical per-key ordering/results and total resource bounds; vary
  deterministic key skew, read/write mix and service cost while holding placement explicit.
  **Evidence:** reference state, reply identities, hot-key/lane latency, queue pressure,
  CPU use and cancellation outcomes. **Review:** specify the state-ownership and partitioning
  constraints that distinguish legal candidates, rather than treating skew as a machine fact.

- [ ] **EP-X2.4** -- **Compare serial and staged ordered ingestion.**
  **Question:** what overlap is legal and useful when operations have dependencies?
  **Controls:** give records dependent transform/publication steps; compare one owner with
  staged overlap while preserving the same declared publication order and bounded buffers.
  Vary batches, service-time imbalance and injected failure at each boundary.
  **Evidence:** a checked observable sequence, head-of-line delay, completed/aborted effects,
  pressure and rundown; add real output I/O only with an explicit effect/durability contract.
  **Review:** separate per-record dependency from global order and write down the cancellation
  and visibility obligations needed to describe either arrangement.

- [ ] **EP-X2.5** -- **Exercise scatter/gather and broadcast/branch/join constraints.**
  **Question:** how do partitioning, fan-out and joining change ownership and pressure?
  **Controls:** use deterministic partitionable transforms and a branched transform with a
  reference result; compare serial, partitioned and staged arrangements under matched
  aggregate budgets. Vary skew and a slow branch while preserving declared order and identity.
  **Evidence:** exact membership at the join, payload ownership/reclamation, bounded
  intermediate state, latency, pressure and cancellation of partially completed branches.
  **Review:** retain the required flow/cardinality constraints and reject invalid arrangements
  before measuring them; do not force distinct dependency shapes into one archetype.

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

- [ ] **EP-X3.3** -- **Synthesize the experimental constraints into the working planner contract.**
  **Question:** what must a workload description and candidate plan express to reproduce
  the tested legal arrangements? **Controls:** replay representative recorded descriptors
  against deterministic candidate construction and a shared constraint validator; include
  accepted and rejected arrangements, unavailable evidence and bounded search exhaustion.
  **Evidence:** trace each proposed constraint and acceptance case to an experiment or an
  explicit untested design obligation; keep measured costs allocation-specific.
  **Review:** reconcile the [working proposal](DESIGN-PROPOSAL-EP-R1.7.md), canonical decisions
  and acceptance matrix, and record retain/revise/merge/delete dispositions without freezing
  the catalog merely because this sequence ended. Return to `EP-R1.7`'s remaining contract
  decisions before closing it and handing off naming/component plans to `EP-R1.8`.

## M1: state what the planner needs from the topology

The point of doing this first: the design session asks what representation is most useful to
consumers, and **this crate is the consumer**. Answering in the abstract has already produced one
wrong answer this session. Each item below states a query the planner makes, why it makes it, and
whether the topology can answer it today -- so the model is designed against a real caller.

- [x] **EP-1.1** -- The shard-set query requirements and discovered efficiency-class sentinel defect are recorded. -> [completed 2026-09-18 18:50:30 +00:00](COMPLETED-CHECKLIST.md#ep-11)

- [x] **EP-1.2** -- **The proximity query, which is the crux.** For an ~~*ordered pair*~~
  **unordered pair** of processors, how close are they -- because that is what chooses SPSC versus
  MPSC versus a routed hop, and it is asked once per pair rather than once per machine. **The model
  available when this item was written could not answer it**: `outermost_partitioning_cache` reports one global level and
  `same_cache_domain` reduces it to a boolean at that level, so a client reconstructs the rest and,
  per `SH-16.9`, reconstructs it differently each time. State the query precisely enough that the
  session can design against it.
  **Done:** stated as [EP-D-2](DESIGN-NOTES.md#ep-d-2).
  **This item said "ordered pair" and was wrong**, corrected in place rather than quietly. The
  repository had already settled it: `windows-placement-probe` documents that its placement labels
  are "deliberately symmetric", that "the *relationship* between two processors genuinely is
  symmetric", and that "direction therefore lives where it is real, not in the label" -- with the
  measured side putting it as "a hop is not symmetric even though the link is". Proximity is the
  link and is unordered; direction is the hop, and belongs to EP-1.3's residency question.
  Three requirements came out of stating it. The answer needs the **membership** of the shared
  granularity, not just its identity, or the planner re-derives the grouping to size an MPSC
  fan-in. It needs to distinguish "tightest shared is X" from "**at most** X, and finer was not
  observed", since under the model's bar the planner cannot go and check. And the order being by
  inclusion rather than by firmware numbering means two granularities can be **incomparable**, so
  the answer is a set of minimal shared granularities -- almost always one, but not by construction.

- [x] **EP-1.3** -- **The residency query.** Which memory domain each processor belongs to, and --
  for a pair spanning two of them -- what it costs to place a shared buffer on one side rather than
  the other. **Gap already identified:** [D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20)
  removed `MachineMemoryTopology::distances` at the Win32 boundary, so this cost has to enter through
  the abstract model via the inward adapter/synthesizer path. That contract is still missing and
  remains tracked as `EP-1+.4`.
  **Done:** stated as [EP-D-3](DESIGN-NOTES.md#ep-d-3). This is where the direction EP-1.2 refused
  lands -- proximity is the link and symmetric, residency is the hop and is not.
  The processor-to-node half is answered, with one asymmetry worth preserving: an unknown *cache*
  domain costs an optimisation, but an unknown *memory* domain has no honest fallback, since the
  pool must be allocated somewhere and guessing means quietly allocating remote memory for the life
  of the process. `windows-placement-probe` already refuses on the second while tolerating the
  first, and that judgement was correct.
  **The cost half is now scoped to the current boundary.** The planner still needs directed
  residency-cost input plus measurement context, but those facts no longer live in
  `windows-topology-sys`; they are requirements on `topology-model` and the adapter that populates it.
  Historical trigger analysis remains in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#ep-d-3-rationale-and-history).

- [ ] **EP-1.4** -- **What the planner does when required evidence remains unavailable.** Runtime
  characterization may answer a fact the initial machine observation did not, but measurement can
  be disallowed, fail, exceed its budget, or remain inconclusive. Decide when the planner adapts to
  a documented weaker policy, emits a plan carrying an explicit assumption or unresolved constraint,
  or refuses to plan.
  **Unblocked and owned here.** [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21) requires
  `windows-topology-sys` only to state policy-free platform facts and distinguish absence; it does
  not decide consumer behavior and will not be reshaped to match the planner's goal. The remaining
  decision depends on the topology-specification, measurement-permission, and plan-evidence
  contracts, so it is resolved with `EP-R1.7`.

- [x] **EP-1.5** -- The locality-model handoff and shipped-model coverage reconciliation are complete. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-15)

## M1+: the scenario input, and the naming

Raised when the engineer described this component's function, which turned out to be richer than
"takes a topology, applies policy". No external model work gates these items. `EP-R1.7` owns the
topology specification and callback contract; `EP-1+.5` names the remaining components and types
after those responsibilities are settled.

- [ ] **EP-1+.1** -- **Describe the scenario input.** The synthesizer takes *two* inputs and only one
  is described anywhere. The scenario says what the caller intends to run, and it is what makes a
  measurement meaningful: [EP-D-3](DESIGN-NOTES.md#ep-d-3) established that a measured number means
  nothing without knowing what it measured, so at minimum the scenario must distinguish small-message
  handoff from large-buffer streaming. Its absence is why "what is most useful for consumers" was
  hard to answer in the abstract for so long.

- [ ] **EP-1+.2** -- **Decide what the caller-callback traits ask.** Planning is a negotiation: the
  component may call back for clarification the scenario did not settle. Enumerating those questions
  is what decides whether this is one trait or several, and it cannot be done before EP-1+.1 says
  what the scenario already answers.

- [x] **EP-1+.3** -- The planner, model, and conceptual input/output vocabulary are named. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-1+3)

- [x] **EP-1+.4** -- Measurement ownership for directed residency cost is assigned. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-1+4)

- [ ] **EP-1+.5** -- **Name the remaining platform components and public contract types after
  EP-R1.7 settles their responsibilities.** Apply [EP-D-8](DESIGN-NOTES.md#ep-d-8): platform crates
  use `windows-`, only direct low-level wrappers use `-sys`, role names are preferred over generic
  `adapter`, and physical facts, developer intent, and allocation-specific results use distinct
  nouns. Complete this before `EP-R1.8` creates component-local plans.

## M2+: the plan as a value

Parked, not pending. The Windows topology reshape has already shipped. This work waits on MR1,
`EP-R1.8`'s component-local plans, and implementation of the resulting shared `topology-model`
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

Parked. These are the choices the crate exists to make, and each is a decision item rather than an
implementation one.

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
