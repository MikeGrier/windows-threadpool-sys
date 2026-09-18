# Design rationale: topology-planner

Historical design context and exploratory analysis for this component. Current canonical decisions are
in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## <a id="ep-d-3-rationale-and-history"></a>EP-D-3 rationale and history

Earlier drafts of EP-D-3 were written against a now-deleted `MachineMemoryTopology::distances`
field and analyzed whether that field could be repurposed. That framing is retained here as history:
it explains why a scalar, symmetric distance matrix was considered insufficient for the directional
residency question and why measured values must carry measurement context, not only provenance.

After [D-20](../windows-topology-sys/DESIGN-NOTES.md#d-20), the relevant boundary is explicit:
`windows-topology-sys` does not publish below Win32 topology APIs, so residency-cost data must be
provided by the abstract model path and its adapter/synthesizer inputs.

## <a id="ep-d-4-open-follow-ups"></a>EP-D-4 open follow-ups (historical)

EP-D-4 intentionally left several follow-ups unresolved:

- Adapter naming.
- Measurement ownership in the new four-part architecture.
- Consumer behavior for not-observed facts after the boundary split.

These are tracked as checklist work in [CHECKLIST.md](CHECKLIST.md) rather than as canonical
decisions.
