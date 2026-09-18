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
- Measurement ownership in the new four-part architecture, since resolved by
  [EP-D-6](DESIGN-NOTES.md#ep-d-6).
- Consumer behavior for not-observed facts after the boundary split.

These are tracked as checklist work in [CHECKLIST.md](CHECKLIST.md) as `EP-1.4` (not-observed
behavior) and `EP-1+.3` (planner/model/adapter naming). Measurement ownership was completed by
`EP-1+.4` and `EP-R1.1`; the current decision is [EP-D-6](DESIGN-NOTES.md#ep-d-6).

## EP-D-6 rationale and discussion

The earlier session used "the synthesizer measures" as though selecting a measurement, authorizing
it, executing it, interpreting it, and retaining it were one responsibility. The review exposed that
later documents had split those responsibilities implicitly and therefore appeared to reverse the
session.

The engineer clarified that runtime planning is the product rather than an exceptional path. The
actual NUMA configuration and process allocation are not known until deployment, and the value of
the component is to keep I/O servicing, buffers, workers, and intentional cross-domain movement from
acquiring accidental remote-memory costs under load. A fixed topology checked into a client
repository would therefore solve the wrong problem.

Two repositories also serve different purposes. Measurement artifacts checked into this repository
demonstrate that the project understands the problem and give developers who have not run the probes
evidence to inspect. Clients with workloads that justify this component will run their own deeper
tests. What belongs in their repository is primarily a constraint specification that bounds and
audits autonomous runtime planning while trust in it develops.

That led to the ownership split recorded in [EP-D-6](DESIGN-NOTES.md#ep-d-6): the planner owns the
campaign and interpretation, the caller authorizes it, platform components execute neutral typed
measurement requests, and probes share those same kernels. Scenario-specific results stay out of
the abstract machine description and travel as evidence with the concrete plan.

The discussion also exposed a later boundary question rather than answering it: the project has
queue and I/O ring primitives, but it has not yet established how much higher-level work-item,
buffer, parsing-stage, serial-stream, parallel-stream, and cross-domain migration machinery it must
provide. That question is deliberately queued in [CHECKLIST.md](CHECKLIST.md) `EP-R1.7`.
