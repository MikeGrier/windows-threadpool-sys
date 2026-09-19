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

- Platform component naming, now governed by [EP-D-8](DESIGN-NOTES.md#ep-d-8) and tracked by
  [CHECKLIST.md](CHECKLIST.md) `EP-1+.5`.
- Measurement ownership in the new four-part architecture, since resolved by
  [EP-D-6](DESIGN-NOTES.md#ep-d-6).
- Consumer behavior for not-observed facts after the boundary split.

These are tracked as checklist work in [CHECKLIST.md](CHECKLIST.md) as `EP-1.4` (not-observed
behavior) and `EP-1+.5` (remaining component and public type names). Measurement ownership was
completed by `EP-1+.4` and `EP-R1.1`; the current decision is
[EP-D-6](DESIGN-NOTES.md#ep-d-6).

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

## EP-D-8 naming rationale

The review found one naming item treated as both complete and open. It had originally bundled the
planner name, the input and output vocabulary, both adapter names, and eventual public type names.
EP-D-4 and EP-D-5 had settled only the first part, while the checklist status described the whole
item as settled.

Forcing the remaining names now would repeat the defect the item was meant to prevent: the first
implementation would choose names before the responsibilities were known. The outward component's
name depends directly on the unresolved work-item and buffer-flow boundary in `EP-R1.7`, and exact
public type names depend on the contract shapes established there.

The completed and open halves are therefore separated. [EP-D-8](DESIGN-NOTES.md#ep-d-8) records the
settled crate and conceptual vocabulary plus the naming rules; [CHECKLIST.md](CHECKLIST.md)
`EP-1+.5` retains the contract-dependent decisions. `EP-R1.8` cannot materialize component-local
plans until those names are settled.

## <a id="ep-d-1-shipped-coverage-history"></a>EP-D-1 shipped-coverage history

The first shard-set analysis correctly identified the facts the planner needed but described the
then-current implementation as though it were the enduring boundary. At that time CPU-set
enumeration was absent, availability observations were unavailable, and efficiency class could be
read through `Processor::capacity`, whose zero sentinel conflated offline, unplaced, and genuine
class-zero processors.

The shipped reshape added `ProcessorFacts` and `MachineMemoryTopology::shard_set()`. That assembled
view preserves missing observations, core gaps, genuine class zero, parked state, explicit CPU-set
allocation, and memory placement as distinct values. The CPU-set allocation flags also produced a
new fact the original requirement could not anticipate: on the measured Windows build they remained
zero even after explicit allocation and therefore cannot support a `usable()` policy decision.

The current decision is [EP-D-9](DESIGN-NOTES.md#ep-d-9). This section retains why the old text once
said availability was absent and why the no-sentinel requirement remains load-bearing.

## <a id="ep-d-2-shipped-coverage-history"></a>EP-D-2 shipped-coverage history

The first proximity analysis found only a machine-wide outermost cache choice and consumer-side
boolean helpers, so it correctly reported that no pairwise proximity answer existed at that time.
It proposed a pairwise query as the primary surface and called for a total order.

The shipped reshape corrected both shapes. The primary surface is the relation collection ordered by
processor-set inclusion, because membership and grouping are properties of relations rather than
pairs. `proximity()` is derived from that collection. The order is partial, the whole machine is a
top, and multiple minimal shared relations remain visible rather than being forced into an arbitrary
linear order.

[EP-D-2](DESIGN-NOTES.md#ep-d-2) further scopes the neutral query to a selected planning universe.
That adapter-level contract is stricter than the Windows convenience query without implying that
the policy-free Windows representation is defective.

## EP-D-9 rationale and I/O endpoint discussion

Refreshing the three original queries against shipped code showed that a binary covered/not-covered
classification was too coarse. `windows-topology-sys` now supplies substantial processor, memory,
and relation facts, while deliberately declining the planner-shaped judgments those facts inform.
The inward component must preserve that distinction while projecting the facts into the neutral
selected-universe contract.

The storage question exposed a missing category in the coverage review. Windows documents
[`FSCTL_QUERY_VOLUME_NUMA_INFO`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/fsctl-query-volume-numa-info)
for the current NUMA node of a volume and
[`DEVPKEY_Numa_Proximity_Domain`](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/devpkey-numa-proximity-domain)
for a device instance's firmware proximity domain. Those are topological attachment observations and should not be discarded merely because
`windows-topology-sys` is intentionally scoped to processor and memory facts. The inward Windows
component can gather them from separate platform surfaces and present a unified neutral machine
without changing the identity of the lower-level crate.

The engineer then generalized the same shape to very high-performance network adapters. Windows'
RSS configuration confirms both halves of that analogy. The standardized
[`*NumaNodeId`](https://learn.microsoft.com/en-us/windows-hardware/drivers/network/standardized-inf-keywords-for-rss)
is the preferred node for adapter memory allocations and for initial RSS processor preference, and
its documentation warns that a PCI card's closest node depends on the slot. At the same time,
[`Get-NetAdapterRss`](https://learn.microsoft.com/en-us/powershell/module/netadapter/get-netadapterrss)
exposes a processor set and indirection-based receive configuration. A NIC therefore has a physical
attachment like an NVMe controller, plus steerable queue and processor behavior that belongs in
typed network capabilities and runtime evidence.

The attachment does not establish end-to-end I/O cost. Windows documents that the ACPI SLIT
relative-distance matrix is
[not exposed by Windows functions](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnumaproximitynode),
and a volume may add filesystem, virtualization,
aggregation, or other layers above a physical NVMe controller. Runtime measurement therefore stays
necessary even when discovery names a home node.

The engineer identified the planning consequence: high-throughput pipelines may assign different
logical stages and tasks to different NVMe drives and domains. One side may read and parse while
another collates, formats, and writes. The same structure applies to NIC receive and transmit paths,
including mixed network-to-storage and storage-to-network flows. I/O endpoints and logical pipeline
partitions are therefore first-class inputs to the same flow-planning problem as processors,
buffers, queues, and intentional cross-domain transfers. The exact executable stage and ownership
contract remains part of [CHECKLIST.md](CHECKLIST.md) `EP-R1.7`.

## EP-X1.1: comparison evidence

The first read/checksum capture kept processor roles fixed and put extra checksum
capacity and stage separation in adjacent comparisons. EP-X1.1 exercised both
orientations without modifying any scheduler, using a balanced treatment sequence
and a same-code control before and after the compute-heavy workload.
The [capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-1/README.md)
retains the observations and their limits. Its heavy-work direct/pipeline overlap
and low samples were not removed to produce a cleaner ranking.

The resulting experiment decision is
[DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md) -> `RC-D8`.
The working proposal links that decision at its evidence boundary; production
component names, contracts and selection policy remain under `EP-R1.7`.

## EP-X1.2: independent budgets and scheduling batches

The initial reader used `depth` for both the pending-read ceiling and payload-buffer
allocation. A depth sweep would therefore have changed two resource allowances.
The experiment's [DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md) ->
`RC-D9` separates them and defines what batching means in these schedulers.
The block-size sweep normalizes buffer count to hold payload bytes fixed; it does
not conceal the resulting changes in block count or outstanding byte ceiling.

Scheduling quanta were chosen to measure refill, forwarding and processing cadence
without introducing a new I/O backend or changing queue operation semantics.
The selected interaction combines queue capacity with batch size, while other
dimensions remain fixed. Reversed point order and bracketing controls retain
variation rather than converting a single faster sample into a default.

The [capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-2/README.md)
contains the observations, summary repair and proposed next cases. Result discussion
remains queued in [CHECKLIST.md](CHECKLIST.md) -> `EP-X1.2`; implementation and
capture alone do not close it.

## EP-D-10: startup cost clarification

The engineer expected architectural guidance from the application's design and the discovered
system: memory domains and I/O attachment anchor buffers and their primary processing, with queues
and transfers distributing work beyond those anchors. The expanding read/checksum sweeps raised
the concern that deployment might need nontrivial pre-execution workload measurement.

The engineer clarified that some extremely small planning and realization cost is expected, but
not cost proportional to running workloads long enough to validate size mixes. Runtime matching
had been conflated with routine active characterization in EP-D-6 and the component overview.
Sharing probe mechanisms does not require a deployed planner to run the offline research program.

[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `EP-D-10` records the corrected boundary. The existing
captures do not establish that workload sweeps are necessary for architectural matching; nor do
they settle endpoint/cross-NUMA placement. They remain research evidence, without being discarded
or promoted into universal defaults. The pending work is to reconcile the contract and queue,
not to silently drop the measurement foundation or any experiment.

[CHECKLIST.md](CHECKLIST.md) -> `EP-R1.7.1` owns that reconciliation before more MX work.
The discussion is preserved in
[DESIGN-SESSION-2026-09-18-topology-first-startup.md](design-sessions/DESIGN-SESSION-2026-09-18-topology-first-startup.md).

### EP-X2.1 offline exception
**Hardware-dependent closure superseded by [EP-D-11](DESIGN-NOTES.md#ep-d-11); the original authorization and capture history below remain recorded.**

The engineer next requested EP-X2.1 and approved proceeding as offline research after
being told that the host exposes only NUMA node 0 and the generated-buffer prerequisite
was missing. Its scope now includes that companion and independent buffer placement;
synthetic selection tests and real local placements proceed while cross-NUMA timing
remains unrun. This is an explicit sequence exception, not acceptance of startup
benchmarking or completion of EP-R1.7.1. The authorized work is in
[CHECKLIST.md](CHECKLIST.md) -> `EP-X2.1`; the shared physical follow-up is now `EP-HW.1`.

The generated and buffered companions now share byte identity and owned payload
primitives while retaining their separate source-service paths. The heap baseline
remains available alongside explicit NUMA-backed allocation. Selection reports core,
memory and each data/unified cache relationship independently rather than assigning
a proximity rank; Windows node labels are never replaced by relation positions.

The [local capture](experiments/read-checksum/captures/2026-09-19-ep-x2-1/README.md)
records available core/cache placements and the missing cross-NUMA evidence.
The shared-memory-node host can exercise allocation preference and observation, but
cannot supply a remote-memory comparison. That observation remains unchanged, but
it no longer prevents behavioral completion of EP-X2.1 under EP-D-11.

## EP-D-11: shared NUMA fidelity

After the local capture, the engineer questioned why cross-NUMA timing kept EP-X2.1
open and asked whether NUMA artifacts could instead enter through data gathering.
The assistant had treated physical cost measurement as a completion obligation even
though the product goal was architectural behavior, not performance baselines.

The engineer explicitly accepted the remaining fidelity limitation: until physical
hardware is available, the tests must use a consistent faux-NUMA model uniformly.
Nothing in that model mimics actual memory distances. Repeating this limitation as
unfinished work across items and milestones adds no validation and prevents honest
completion of software already tested against its specified inputs.

[EP-D-11](DESIGN-NOTES.md#ep-d-11) records the decision. The shared environment and
its adoption by actual experimental consumers are now recorded in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ep-r172). A private platform boundary
was added to the existing gathering and scheduler code rather than testing a second
implementation of their decisions. Generated input avoids OS completion ports;
fake payloads preserve logical residency and record cross-thread use and release.
Controlled refusals exercise cancellation and cleanup through the same loops.
Sabotaging provider use establishes that the tested consumers depend on the injected
state. Existing live tests and captures remain separate; no NUMA timing fidelity is
claimed. The one physical follow-up remains `EP-HW.1`, and future component adoption
is queued in [CHECKLIST.md](CHECKLIST.md) -> `EP-R1.8`. The discussion continues in
[DESIGN-SESSION-2026-09-18-topology-first-startup.md](design-sessions/DESIGN-SESSION-2026-09-18-topology-first-startup.md#follow-up-consistent-faux-numa).
