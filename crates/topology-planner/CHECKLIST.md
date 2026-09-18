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

**Nothing is implemented.** MR1 is the active milestone: it records and resolves the contradictions
and execution gaps found in the 2026-09-18 design review before the existing requirements work
resumes. M1 remains a *requirements* milestone rather than an implementation one.

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
| MR1 design-review reconciliation | 6 done, 2 open | active; work in item order |
| M1 the input contract | 4 done, 1 open | `EP-1.4` waits on EP-R1.7 |
| M1+ scenario and naming | 2 done, 3 open | EP-R1.7 owns the topology specification and callbacks; `EP-1+.5` then settles remaining names |
| M2+ the plan as a value | parked, **and needs re-cutting** | MR1, then EP-R1.8 and the resulting `topology-model` work |
| M3+ the policies | parked | M2+ |
| M-inf parked | ungated | not scheduled, deliberately |

## MR1: reconcile the 2026-09-18 design review

These items are in dependency order. Resolve and commit one item before beginning the next.

- [x] **EP-R1.1** -- Runtime measurement ownership and its data boundary are settled. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r11)

- [x] **EP-R1.2** -- The five-component architecture and its dependency order are settled. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r12)

- [x] **EP-R1.3** -- Stale external gates are replaced by the component's real internal dependencies. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r13)

- [x] **EP-R1.4** -- Query totality, the scoped universe, and specified partitions are distinguished from order totality. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r14)

- [x] **EP-R1.5** -- Settled naming is separated from contract-dependent component and type names. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r15)

- [x] **EP-R1.6** -- Tier 1 now distinguishes shipped Windows facts, adapter projections, I/O endpoint attachment, and planner-owned measurements. -> [completed 2026-09-18](COMPLETED-CHECKLIST.md#ep-r16)

- [ ] **EP-R1.7** -- **Complete the contracts and acceptance matrix before implementation.** Plan
  the scenario/goal contract, plan invariants and errors, deterministic policy and tie-breaking,
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

- [ ] **EP-R1.8** -- **Materialize executable component-local plans after names and contracts are
  settled.** Create dependency-ordered checklists and reciprocal cross-component handoffs for
  `topology-model`, `topology-planner`, the inward adapter, the measurement foundation, and the
  realizer. Move the model, specification, query-trait, measurement-vocabulary, and plan-value work
  to their owning component. Each plan must identify its first implementable item without a
  provisional crate name or an unstated contract prerequisite.

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

## M-inf: parked, ungated

- [ ] **M-inf.1** -- Re-planning at runtime, when processors are parked, hot-added, or the process
  is given a different CPU-set allocation than it started with. Deliberately not scheduled: it needs
  the static case to exist first, and it is a different problem.
