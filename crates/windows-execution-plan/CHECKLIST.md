# Checklist: the topology planner

Plans an arrangement of execution domains from a stated **goal** plus an **abstracted idealized**
description of a machine. See [COMPONENT.md](COMPONENT.md) for what this crate is and why it is
separate from both the topology crate and the runtime, and
[EP-D-4](DESIGN-NOTES.md#ep-d-4) for the architecture it now sits in.

**The component is being re-scoped**, per [EP-D-4](DESIGN-NOTES.md#ep-d-4). It is named
`topology-planner`; it queries an abstract model covering processors, memory, storage, interconnects,
distances and bottlenecks rather than `MachineMemoryTopology` directly; and **adapters** bracket it
-- one exposing its traits over the Windows topology objects, one realizing a plan as buffers, rings
and threads. The directory is still `windows-execution-plan` pending the layout decision that
EP-D-4 leaves open. **M2+ onward are written against the superseded shape and are not yet re-cut.**

## Where this stands

**Nothing is implemented.** M1 is the only active milestone, and it is deliberately a
*requirements* milestone rather than an implementation one: its output is the concrete statement
of what the model must answer, which the open design session needs in order to settle it.

> **-> CROSS-COMPONENT PREREQUISITE:** M2 onwards cannot begin until
> [DESIGN-SESSION-2026-09-02-cache-locality-model.md](../../design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md)
> concludes and `SH-16.8` lands in
> [CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md). The
> planner's central query has no answer in the current model.

| Milestone | State | What it is waiting on |
|---|---|---|
| M1 the input contract | 3 done, 2 blocked | as far as it can go before the model exists |
| M1+ scenario and naming | **partly answered** | the name is settled (EP-D-4); the goal input is deferred for litigation, by direction |
| M2+ the plan as a value | parked, **and needs re-cutting** | the layout decision, then M1 |
| M3+ the policies | parked | M2+ |
| M-inf parked | ungated | not scheduled, deliberately |

## M1: state what the planner needs from the topology

The point of doing this first: the design session asks what representation is most useful to
consumers, and **this crate is the consumer**. Answering in the abstract has already produced one
wrong answer this session. Each item below states a query the planner makes, why it makes it, and
whether the topology can answer it today -- so the model is designed against a real caller.

- [x] **EP-1.1** -- **The shard-set query.** Which processors may host a domain: online, with
  identity carried as `(group, number)` rather than a bare number, with efficiency class and SMT
  structure available so a policy can choose one domain per core or per thread and can decide
  whether efficiency cores are peers. **Gap already identified:** parked and allocated state is not
  available at all, and pinning a domain to a parked processor is a defect a client cannot detect.
  Tracked as `SH-16.10`.
  **Done:** stated as [EP-D-1](DESIGN-NOTES.md#ep-d-1), with each of its five inputs checked against
  the model rather than assumed. Three are answered cleanly; availability is not answered at all;
  and the fourth turned up a defect the item had not anticipated.
  **`Processor::capacity` is unsafe for reading efficiency class.** It is
  `online.then(find owning Core).flatten().unwrap_or(0)`, so `0` means offline, *or* in no core
  domain, *or* genuinely class zero -- and the third is every processor on every non-hybrid machine,
  so the sentinel collides with the common legitimate value. Worse here than elsewhere, because
  Windows orders class `0` as *least* performant: on a hybrid part an unknown processor is
  indistinguishable from an efficiency core, so a policy excluding them silently drops a possible
  performance core and a policy tiering them mis-tiers it. Neither fails a functional test. Filed
  against the owning crate as `SH-16.12`; use `DomainKind::Core { efficiency_class }` meanwhile.

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
  the other. **Gap already identified:** `MachineMemoryTopology::distances` exists, is never populated, and Win32
  cannot populate it; the measurement exists in `windows-placement-probe` and reaches nothing.
  Tracked as `SH-16.11`. The probe measures this per node pair with a dedicated ring-placement
  column precisely because it was found to matter.
  **Done:** stated as [EP-D-3](DESIGN-NOTES.md#ep-d-3). This is where the direction EP-1.2 refused
  lands -- proximity is the link and symmetric, residency is the hop and is not.
  The processor-to-node half is answered, with one asymmetry worth preserving: an unknown *cache*
  domain costs an optimisation, but an unknown *memory* domain has no honest fallback, since the
  pool must be allocated somewhere and guessing means quietly allocating remote memory for the life
  of the process. `windows-placement-probe` already refuses on the second while tolerating the
  first, and that judgement was correct.
  **The cost half needs SH-16.11 restated, and it was.** That item read as though someone had
  forgotten to populate a field. Two sharper problems replace it: `distances` can never carry
  `Measured` provenance **by construction** -- its only inputs are a literal (`Synthetic`) and a file
  (capped at `Restored`) -- so populating it would not help; and even populated it is SLIT-shaped,
  one symmetric workload-independent scalar, while the question is directional. `D-9` in the
  topology crate already deferred the attributed edge list that would answer it, naming *asymmetry*
  among what it would absorb, with the trigger being that a scalar "demonstrably mismodels a machine
  somebody is tuning for" -- and this planner is that machine-tuner.
  **The trigger is approached, not met**, and the gap is a measurement nobody here can take: both
  development hosts are single-node, so every directional run prints "VACUOUS ON THIS MACHINE".
  Recorded so D-9 is reopened on evidence rather than on argument.

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
  [windows-topology-sys](../windows-topology-sys/CHECKLIST.md) `M4+.1` the ordered collection is the
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

- [ ] **EP-1+.3** -- **Settle the naming, before any type is written.** Both inputs and the output
  are graphs of processors and their relations, so "topology" fits all of them and distinguishes
  none -- and a reader seeing the word twice will eventually take one for the other. Decide whether
  the observed machine keeps the bare name (qualified only by its crate), gains a qualifier, or is
  renamed outright, and what the synthesized arrangement is called. Cheap now; expensive once either
  name is public. This one blocks nothing but should not be settled by whoever writes the first type.

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
