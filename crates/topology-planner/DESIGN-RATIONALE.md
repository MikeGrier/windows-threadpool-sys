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

## EP-D-7 rationale and discussion

EP-D-6 separated who chooses and interprets a measurement from who executes it. Applying that split
to EP-D-5 showed that the four-component count had hidden active measurement inside a boundary whose
other responsibilities did not fit it.

The inward adapter could not own measurement without becoming scenario-aware, expensive, and
dependent on queue and I/O mechanisms. The realizer could not own it because probes and planning
need measurement before an application topology exists. The neutral planner could not execute it
without acquiring Windows dependencies. A separate measurement foundation was therefore a
dependency consequence rather than another conceptual preference.

Reading the current probes found substantial reusable source material rather than a ready production
component. `windows-placement-probe` already separates pure placement selection from live execution,
and `windows-platform-probes` already calls into that crate instead of duplicating the placement
measurement. But the probe source and manifest explicitly reject production dependency and stable
measurement-code compatibility. The production boundary must therefore be extracted downward, with
the probes becoming clients of the extracted owner.

The engineer added a testability requirement: platform inputs must be mockable so degenerate and
erroneous behaviors can be generated in volume before hardware testing. The boundary recorded in
[EP-D-7](DESIGN-NOTES.md#ep-d-7) keeps that strong without confusing a simulated duration with
evidence about a real machine. Mocked tests prove interaction and validation behavior; real hardware
produces timing evidence.

Final component-local plans were not written under provisional names. Their first implementable
items also depend on contracts still owned by `EP-R1.7`. The completed architecture item therefore
spawns [CHECKLIST.md](CHECKLIST.md) `EP-R1.8`, after naming and contract work, rather than pretending
those prerequisites do not exist.

## EP-D-2 totality correction and scoped-universe rationale

The consumer handoff said "the order must be total" because it was trying to avoid an empty answer
for processors that shared no recorded cache or memory boundary. That stated the remedy as the
property. The implemented Windows relation model later made the distinction visible: its order is a
poset with a top, and incomparable relations are real.

The corrected requirement is that the **query** be total over a chosen universe. The universe
defaults to the supplied system, including a mocked system, and a topology specification may narrow
it to a subset. That subset becomes the top for the planning scope. Nothing requires two finer
physical relations to be comparable.

The engineer also identified developer-defined partitions as a separate input. Folding them into the
physical relation order would make a workload grouping look like evidence of hardware proximity.
They therefore constrain candidate plans without changing what a proximity answer means. Synthetic
hardware structure belongs in the abstract-machine input; logical workload structure belongs in the
topology specification.

A second consequence is diagnostic rather than mathematical. A logical partition that crosses a
physical boundary may be suspicious, but it may also express deliberate transfer. The planner can
cheaply state the crossing and its measured or structural consequence; whether it warns, accepts, or
refuses depends on intent carried by the constraint. That contract remains scheduled by
[CHECKLIST.md](CHECKLIST.md) `EP-R1.7`.
