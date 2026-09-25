# Checklist: the topology planner

Plans arrangements of execution domains from a **dataflow description of an application** -- its
input, output, and processing code paths -- plus an **abstracted idealized** description of a
machine. See [COMPONENT.md](COMPONENT.md) for what this crate is and why it is separate from both
the topology crate and the runtime, [EP-D-4](DESIGN-NOTES.md#ep-d-4) for the architecture it now
sits in, and [EP-D-6](DESIGN-NOTES.md#ep-d-6) for the two-stage shape and the plural answer.

**The component has been re-scoped**, per [EP-D-4](DESIGN-NOTES.md#ep-d-4) and
[EP-D-5](DESIGN-NOTES.md#ep-d-5). It is named `topology-planner` and the directory now matches; it
queries an abstract model covering processors, memory, storage, interconnects, distances and
bottlenecks rather than `MachineMemoryTopology` directly; and **adapters** bracket it -- one exposing
the model's traits over the Windows topology objects, one realizing a plan as buffers, rings and
threads. The model, its traits, and the plan type live in a separate `topology-model` crate that
everything depends on and that depends on nothing.
**M2+ onward are written against the superseded shape and are not yet re-cut.**

## Where this stands

**Nothing is implemented.** M1 is the only active milestone, and it is deliberately a
*requirements* milestone rather than an implementation one: its output is the concrete statement
of what the model must answer, which the open design session needs in order to settle it.

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
| M1 the input contract | 3 done, 2 open | `EP-1.4` and `EP-1.5`'s coverage half, which want a settled model |
| M1+ scenario and naming | **partly answered** | the name is settled (EP-D-4); the goal input's *shape* is settled as a dataflow description (EP-D-6), and its concrete vocabulary is `EP-1+.1`'s remaining work |
| M2+ the plan as a value | parked, **and needs re-cutting** | re-cut against EP-D-4/EP-D-5, then the topology reshape landing |
| M3+ the policies | parked | M2+ |
| M-inf parked | ungated | not scheduled, deliberately |

## M1: state what the planner needs from the topology

The point of doing this first: the design session asks what representation is most useful to
consumers, and **this crate is the consumer**. Answering in the abstract has already produced one
wrong answer this session. Each item below states a query the planner makes, why it makes it, and
whether the topology can answer it today -- so the model is designed against a real caller.

- [x] **EP-1.1** -- The shard-set query requirements and discovered efficiency-class sentinel defect are recorded. -> [completed 2026-09-18 18:50:30 +00:00](COMPLETED-CHECKLIST.md#ep-11)

- [x] **EP-1.2** -- **The proximity query, which is the crux.** For an ~~*ordered pair*~~
  **unordered pair** of processors, how close are they -- because that is what chooses SPSC versus
  MPSC versus a routed hop, and it is asked once per pair rather than once per machine. **The
  current model cannot answer it**: `outermost_partitioning_cache` reports one global level and
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

- [ ] **EP-1.4** -- **What the planner does with an unanswered query**, given the model's bar is
  that it answers without further measurement. A fact that was not observed cannot be acquired at
  planning time, so decide per query whether the planner degrades to a documented weaker policy,
  refuses to plan, or emits a plan carrying an explicit "this was chosen without knowing X" marker.
  The third is the only one that survives review of a plan by a human, which is one of the reasons
  a plan is a value.
  **BLOCKED, and not merely because it is downstream.** EP-1.1 through EP-1.3 push requirements
  *into* the model's design, which is why they were worth doing against today's model and found real
  defects in it. This item reads behaviour *out* of the model -- it asks what the planner does when
  the answer is "not observed", a state the model cannot currently express reliably -- so doing it
  now would be analysing a shape that does not exist yet.
  **It is also a duplicate.** The design session's fourth open question, "what a consumer does when a
  needed fact is `not measured`", is this same decision seen from the model's side; the two were
  filed independently before anyone noticed. Taken separately they can disagree: a planner that
  degrades in a way the model does not support, or a model offering a fallback no consumer wants.
  Answer them together, in the session.
  **Narrowed by [D-19](../windows-topology-sys/DESIGN-NOTES.md#d-19).** The item says "decide per
  query", and that is now more work than the model requires. A subject the two sources genuinely
  contested is one the unified view does not cover, which is indistinguishable from not-observed to a
  consumer -- so this is one decision about one degradation path, not one per reason a fact is
  missing. The three candidate behaviours are unchanged.
  **And it is no longer a duplicate**, per [D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21).
  `windows-topology-sys` publishes a refined view of what the platform publishes; what a consumer
  *does* with an unobserved fact is not a question about that view. That crate owes only that the
  absence be representable and distinguishable, which its `M2+.5` implements. `MMT-1.3` is closed on
  those grounds, so **this item is now this component's decision alone** -- there is nothing left to
  take jointly, and it no longer blocks anything in the model crate.

- [ ] **EP-1.5** -- **Hand the resulting requirements to the design session** as the consumer-side
  input it asked for, and record in the session which of them the settled model answers and which
  it deliberately does not.
  **Half done, and split because the halves have different prerequisites.** The *handover* is
  complete: the session now carries the three queries in a table, plus the four model properties that
  follow from them -- a pairwise query must exist, the order must be total, an answer must be able to
  be an upper bound, and a measured number must carry what it measured. That was the part the model
  designer needs in front of them, and it did not depend on the model existing.
  **One of the four has since been corrected**, and it is recorded here rather than rewritten in the
  session, which is an append-only record of what was handed over. "A pairwise query must exist" is
  right about the requirement and wrong about the shape: per
  [windows-topology-sys](../windows-topology-sys/COMPLETED-CHECKLIST.md) `M4+.1` the ordered collection is the
  surface and the pairwise query is derived from it, because an answer obliged to carry the block
  containing both processors is a question about the partition rather than about the pair.
  The *coverage* half -- recording which requirements the settled model answers and which it
  deliberately does not -- can only be written once there is a settled model. It stays open here.
  > **-> CROSS-COMPONENT HANDOFF:** next work is in the repository root ->
  > [DESIGN-SESSION-2026-09-02-cache-locality-model.md](../../design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md)
  > -> `SH-16.8` in
  > [CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md).

## M1+: the scenario input, and the naming

Raised when the engineer described this component's function, which turned out to be richer than
"takes a topology, applies policy". Both are gated on the locality-model session, but neither is a
model question -- they are this component's own.

**Re-planned 2026-09-23 by [EP-D-6](DESIGN-NOTES.md#ep-d-6)**, which settled the input's shape and
in doing so added work this milestone did not anticipate: the two-stage split means stage 1 has an
output that is a value in its own right, and the plural answer means stage 2 returns a set rather
than a plan. `EP-1+.1` is narrowed accordingly and `EP-1+.5` / `EP-1+.6` are new.

- [ ] **EP-1+.1** -- **Give the dataflow description a concrete vocabulary.** Its *shape* is settled
  by [EP-D-6](DESIGN-NOTES.md#ep-d-6) -- a sufficiently abstract definition of the application's
  input, output, and processing code paths -- so what remains is the types: how a caller names a
  source, a sink and a processing step, and how they express an edge's characteristics. That last
  part is what makes a measurement meaningful, since [EP-D-3](DESIGN-NOTES.md#ep-d-3) established
  that a measured number means nothing without knowing what it measured; at minimum an edge must
  distinguish small-message handoff from large-buffer streaming. **Note the correction EP-D-6
  carried:** that distinction is an attribute *of an edge*, not the whole scenario, which is how
  this item originally framed it.

- [ ] **EP-1+.2** -- **Decide what the caller-callback traits ask.** Planning is a negotiation: the
  component may call back for clarification the scenario did not settle. Enumerating those questions
  is what decides whether this is one trait or several, and it cannot be done before EP-1+.1 says
  what the scenario already answers.

- [ ] **EP-1+.3** -- **Settle the naming, before any type is written.** There are now **four** graphs
  in play and "topology" fits several of them while distinguishing none -- a reader meeting the word
  twice will eventually take one for the other. They are: the **machine** (processors and their
  relations), the **application's dataflow description** (sources, sinks and processing steps), the
  **connectivity graph** stage 1 infers from it (the same nodes, with directed flow), and each
  **realization** stage 2 emits (threads on processor groups, with queues between them). Note that
  only two of the four are graphs of *processors*, which is itself a correction:
  [EP-D-6](DESIGN-NOTES.md#ep-d-6) made the input an application-shaped graph, where this item
  previously assumed every graph was machine-shaped. Decide whether the observed machine keeps the
  bare name (qualified only by its crate), gains a qualifier, or is renamed outright; what each of
  the other three is called; and whether the inward/outward adapters keep those role names or gain
  more specific crate/type names. Cheap now; expensive once any of those names are public. This one
  blocks nothing but should not be settled by whoever writes the first type. **The connectivity
  graph is the one with no name at all today.**

- [ ] **EP-1+.4** -- **Assign measurement ownership for directed residency cost in the four-part
  architecture.** [EP-D-3](DESIGN-NOTES.md#ep-d-3) requires directed cross-domain cost input with
  measurement context. Record which layer owns collecting, validating, and supplying that measurement
  context to `topology-model` through the inward adapter/synthesizer path.

- [ ] **EP-1+.5** -- **Give stage 1's output a type, and decide which crate holds it.**
  [EP-D-6](DESIGN-NOTES.md#ep-d-6) establishes that the connectivity graph with its directed flow is
  derived before any machine exists, which makes it a value that can be inspected and reviewed
  without even a synthetic machine -- a stronger version of the argument
  [EP-D-5](DESIGN-NOTES.md#ep-d-5) used to make the plan a value. EP-D-5's placement rule points at
  `topology-model` for both this type and the dataflow description, on the grounds that a component
  which only describes or realizes must not depend on planning policy. **EP-D-6 recorded that as an
  argument and deliberately did not take it as a decision**; this item takes it, either way, and says
  what depends on the answer.

- [ ] **EP-1+.6** -- **Make the plural answer explicit in the plan vocabulary.** The planner returns
  *one or more* suggested realizations ([EP-D-6](DESIGN-NOTES.md#ep-d-6)), so the type a caller
  receives is a set and the thing `M2+.2` renders is a set. Decide what a candidate carries beyond
  the arrangement itself -- at minimum, enough for a developer to tell two candidates apart and say
  why they would pick one, which is the whole purpose of returning more than one. Ranking is
  explicitly *not* in scope here: the planner declines to rank because it cannot know what the
  developer values, and `M3+` owns whether that ever changes.

## M2+: the plan as a value

Parked, not pending. Gated on the topology model landing. Shape recorded so it is not lost, per the
`M{n}+` convention.

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
