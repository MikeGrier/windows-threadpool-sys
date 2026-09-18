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
