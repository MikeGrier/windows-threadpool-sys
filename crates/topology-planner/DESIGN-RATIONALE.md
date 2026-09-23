# Design rationale: topology-planner

Historical design context and exploratory analysis for this component. Current canonical decisions are
in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## <a id="ep-d-3-rationale-and-history"></a>EP-D-3 rationale and history

Earlier drafts of EP-D-3 were written against a now-deleted `MachineMemoryTopology::distances`
field and analyzed whether that field could be repurposed. The current contract boundary is owned by
[D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20); this section keeps only the historical reasons
that the old field-centric framing stayed open.

The first reason was shape: the planner's question is directional -- which side of a spanning pair
hosts the shared buffer -- while a scalar, symmetric distance matrix can at best describe the link
between two domains and not the hop-specific residency choice.

The second reason was evidence. `windows-placement-probe` already supplied directional measurements,
but every host available when this was written was single-node, so the measured result was vacuous:
it confirmed that the probe carried direction and measurement context correctly, but it did not yet
show a real cross-node asymmetry.

That left the D-9 reopening trigger approached but unmet. The planner had reached the point of
needing a directional cost, but without a machine whose measured result demonstrated that a scalar
distance mismodels the machine somebody is tuning for, reopening the old topology-field decision
remained deferred rather than claimed as proved.

## <a id="ep-d-4-open-follow-ups"></a>EP-D-4 open follow-ups (historical)

EP-D-4 intentionally left several follow-ups unresolved:

- Adapter naming.
- Measurement ownership in the new four-part architecture.
- Consumer behavior for not-observed facts after the boundary split.

These are tracked as checklist work in [CHECKLIST.md](CHECKLIST.md) as `EP-1.4` (not-observed
behavior), `EP-1+.3` (planner/model/adapter naming), and `EP-1+.4` (measurement ownership), rather
than as canonical decisions.

## Why the goal turned out to be a dataflow description, and the answer plural

[DESIGN-NOTES.md](DESIGN-NOTES.md#ep-d-6) records `EP-D-6`. This is how it was reached.

The goal's shape had been deferred since `EP-D-4` (2026-09-03) and the deferral was **named**, which
is what made it survivable: `COMPONENT.md`, the checklist status table and the decision body all said
"deferred for litigation" rather than quietly omitting the input, so nothing was built on a guess in
the meantime. It was settled on 2026-09-23 by the engineer stating the component's purpose directly,
in the course of correcting a milestone that had been written in the wrong crate.

**Two candidate shapes had been implicitly in play, and neither was what was chosen.** `EP-1+.1`
framed the input as a *scenario* -- "what the caller intends to run" -- with a minimum bar of
distinguishing small-message handoff from large-buffer streaming. That framing is a set of workload
*characteristics*, and it would have made the planner's input a bag of tuning hints. The other
implicit shape was a *goal* in the literal sense, some statement of what to optimize (latency,
throughput, footprint), which would have made the planner a solver over an objective function.

What was chosen is neither: the input describes **the application's own structure** -- its inputs,
its outputs, and the processing paths between them. The characteristics `EP-1+.1` named do not
disappear, but they demote from being the scenario to being **attributes of an edge** in that
structure, which is a strictly more informative place for them: "large buffers" is not a property of
a workload, it is a property of a particular flow within it, and a real application has several
flows that differ.

**The two-stage split was not stated as a separate decision and follows from the input's shape.**
Once the input is the application's structure rather than a set of hints, the connectivity implied by
that structure can be derived with no machine present at all -- and a derivation that does not need a
machine should not be entangled with one. That yields an intermediate artifact that is stable for the
life of the application, where only the second stage is redone per machine. The alternative, deriving
connectivity and placement together, would make the application's own shape re-derivable only in the
presence of a machine, which is the coupling the
[adoption thesis](../../DESIGN-NOTES.md#the-adoption-thesis) exists to object to.

**The plural answer is the part most likely to be eroded later, so the reason is recorded here.** A
single returned plan is easier to consume, easier to test, and easier to document, and every one of
those pressures argues for collapsing the set at some future convenient moment. The reason not to is
that ranking candidates requires knowing what the developer values, which is the one thing this
component structurally does not know -- it was given a description of an application, not a statement
of preference. A planner that returns one arrangement has either acquired a preference it was not
given or hidden a choice it was not entitled to make. That is the same argument as OPTION INTEGRITY
in [copilot-instructions.md](../../.github/copilot-instructions.md), arriving at component scale
rather than at documentation scale.

**What was deliberately not decided**, and is queued instead: where the two new types live.
`EP-D-5`'s placement rule points at `topology-model` for both, and the argument is recorded in
`EP-D-6` as an argument. Taking it in the same breath as the decision it follows from would have made
one decision carry two, and the placement question has a consequence -- whether a caller can hold a
dataflow description without depending on planning policy -- that deserves to be litigated on its
own. It is `EP-1+.5`.