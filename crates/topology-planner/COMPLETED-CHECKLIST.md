# Completed checklist

## Moved 2026-09-18 18:50:30 +00:00 -- Archive completed EP-1.1 details

### <a id="ep-11"></a>EP-1.1 -- The shard-set query requirements and discovered efficiency-class sentinel defect are recorded. *(completed 2026-09-18 18:50:30 +00:00)*

- [x] **EP-1.1** -- **The shard-set query.** Which processors may host a domain: online, with
  identity carried as `(group, number)` rather than a bare number, with efficiency class and SMT
  structure available so a policy can choose one domain per core or per thread and can decide
  whether efficiency cores are peers. **Gap already identified:** parked and allocated state is not
  available at all, and pinning a domain to a parked processor is a defect a client cannot detect.
  Tracked as [CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md)
  -> `SH-16.10`.
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
  against the owning crate as [CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md)
  -> `SH-16.12`; use `DomainKind::Core { efficiency_class }` meanwhile.

## Moved 2026-09-18 18:53:28 -04:00 -- Settle runtime measurement ownership

### <a id="ep-r11"></a>EP-R1.1 -- Runtime measurement ownership and its data boundary are settled. *(completed 2026-09-18 18:53:28 -04:00)*

Recorded as [EP-D-6](DESIGN-NOTES.md#ep-d-6). The runtime planner owns the autonomous measurement
campaign: it decides what the scenario and current allocation require, sequences and interprets the
measurements, and decides when evidence is sufficient. The caller authorizes the campaign through a
checked-in topology specification. Neutral measurement contracts live in `topology-model`;
platform-specific components execute them; and developer-facing probes and the planner share the
same underlying measurement kernels.

Scenario-specific measurements remain planning evidence rather than abstract machine facts. The
concrete allocation-specific plan retains enough context, assumptions, and results to explain its
choices. The client repository normally stores constraints and permissions rather than an exact
processor and queue arrangement.

### <a id="ep-1+4"></a>EP-1+.4 -- Measurement ownership for directed residency cost is assigned. *(completed 2026-09-18 18:53:28 -04:00)*

Completed by [EP-D-6](DESIGN-NOTES.md#ep-d-6). The planner requests and interprets directed
residency measurements; a platform measurement component executes the neutral request; and the
result travels with the concrete plan as scenario-specific evidence rather than being inserted into
the abstract machine description.

## Moved 2026-09-18 19:13:56 -04:00 -- Settle the measurement-foundation architecture

### <a id="ep-r12"></a>EP-R1.2 -- The five-component architecture and its dependency order are settled. *(completed 2026-09-18 19:13:56 -04:00)*

Recorded as [EP-D-7](DESIGN-NOTES.md#ep-d-7). The architecture contains `topology-model`,
`topology-planner`, an inward Windows topology adapter, a Windows measurement foundation, and an
outward realizer. The planner owns the campaign and interpretation; the measurement foundation owns
active kernels and their live backend; and probes call the same kernels rather than supplying a
second implementation.

The existing probe code is reusable source material but not the production dependency: its current
contracts explicitly describe it as experimental. The extracted foundation has an injected
platform-operations boundary so degenerate, erroneous, and partial-success behavior can be tested in
volume. Simulated interactions prove validation and cleanup; only the live backend produces
hardware timing evidence.

Final component-local checklists require the names and contracts still owned by `EP-R1.5` and
`EP-R1.7`. Their materialization is scheduled as `EP-R1.8` rather than performed under provisional
crate names.

## Moved 2026-09-18 19:28:20 -04:00 -- Replace stale external gates

### <a id="ep-r13"></a>EP-R1.3 -- Stale external gates are replaced by the component's real internal dependencies. *(completed 2026-09-18 19:28:20 -04:00)*

The locality-model session concluded and the Windows topology reshape shipped, so neither remains a
gate. `EP-1.4` now waits on the topology-specification and plan-evidence contracts in `EP-R1.7`;
`EP-1.5`'s coverage half is assigned to `EP-R1.6`; M1+ maps to `EP-R1.5` and `EP-R1.7`; and M2+
waits on MR1, `EP-R1.8`, and the resulting `topology-model` implementation.

The reconciliation also restates the boundary owned by
[D-21](../windows-topology-sys/DESIGN-NOTES.md#d-21): `windows-topology-sys` is a policy-free,
directionless, memory-safe elevation of Win32 processor and memory topology facts. A mismatch with
the planner's goal is handled in the adapter or neutral model rather than by reshaping that facts
crate; defects in the facts layer are still fixed at their source.

## Moved 2026-09-18 19:41:49 -04:00 -- Correct locality query totality

### <a id="ep-r14"></a>EP-R1.4 -- Query totality, the scoped universe, and specified partitions are distinguished from order totality. *(completed 2026-09-18 19:41:49 -04:00)*

[EP-D-2](DESIGN-NOTES.md#ep-d-2) now defines physical locality as a partial order by membership.
The planning universe is an input that defaults to the supplied real or mocked system and may be
narrowed by a constraint. Its selected membership is the top for that scope, so every valid
non-empty known-processor query has an answer without forcing incomparable physical relations into
an arbitrary linear order.

Developer-defined partitions constrain candidate plans but remain distinct from physical-locality
facts. A cheap structural diagnostic may report that a constraint crosses a cache or NUMA boundary,
but its intent decides whether that is accidental or deliberate. The intent-aware diagnostic
contract is scheduled by `EP-R1.7`.

## Moved 2026-09-18 19:46:04 -04:00 -- Separate settled and open naming work

### <a id="ep-r15"></a>EP-R1.5 -- Settled naming is separated from contract-dependent component and type names. *(completed 2026-09-18 19:46:04 -04:00)*

[EP-D-8](DESIGN-NOTES.md#ep-d-8) records the settled `topology-model` and `topology-planner` crate
names, the conceptual distinction among facts, developer constraints, and allocation-specific
plans, and the naming rules for later work. Names that depend on EP-R1.7's unresolved responsibilities
remain explicitly owned by `EP-1+.5` rather than being chosen by the first implementation.

### <a id="ep-1+3"></a>EP-1+.3 -- The planner, model, and conceptual input/output vocabulary are named. *(completed 2026-09-18 19:46:04 -04:00)*

The neutral crates are `topology-model` and `topology-planner`. Current prose distinguishes Windows
machine facts, the neutral abstract machine, the checked-in topology specification, and the concrete
runtime plan. Final names for the platform components and public Rust types are deliberately moved
to `EP-1+.5`, after their contracts are settled.

## Moved 2026-09-18 19:54:44 -04:00 -- Reconcile shipped fact coverage and I/O endpoint topology

### <a id="ep-r16"></a>EP-R1.6 -- Tier 1 now distinguishes shipped Windows facts, adapter projections, I/O endpoint attachment, and planner-owned measurements. *(completed 2026-09-18 19:54:44 -04:00)*

[EP-D-1](DESIGN-NOTES.md#ep-d-1) now records that the shipped shard-set surface supplies processor
identity, online state, core and SMT membership, efficiency class, memory placement, and CPU-set
observations without converting them into a planner eligibility judgment.
[EP-D-2](DESIGN-NOTES.md#ep-d-2) now records that the shipped relation collection and derived
proximity query supply policy-free physical structure, while the inward component scopes those
facts to the selected planning universe and the neutral model owns the stricter planner-facing
contract.

[EP-D-9](DESIGN-NOTES.md#ep-d-9) records the resulting coverage boundary. Storage and network
endpoints are first-class topology resources with observed NUMA attachment when Windows can report
it. Logical pipeline partitions may assign responsibilities across drives, NICs, and domains, while
runtime measurement establishes directed endpoint, queue, buffer, and worker cost. Historical
claims about missing CPU-set and proximity surfaces moved to
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

### <a id="ep-15"></a>EP-1.5 -- The locality-model handoff and shipped-model coverage reconciliation are complete. *(completed 2026-09-18 19:54:44 -04:00)*

The design session received the shard-set, proximity, and residency requirements. Current
documentation preserves the requirements while correcting two proposed shapes: the physical
relation order is partial rather than total, and the relation collection is primary while
proximity is derived. [EP-D-9](DESIGN-NOTES.md#ep-d-9) records which requirements are available as
policy-free Windows facts, which the inward component translates into the client-shaped neutral
model, and which require planner-owned runtime measurement.

## Moved 2026-09-18 21:57:21 -07:00 -- Controlled read/checksum comparisons

### <a id="ep-x11"></a>EP-X1.1 -- Separate repeatability, stage separation and processing parallelism. *(completed 2026-09-18 21:57:21 -07:00)*

Implemented balanced role-swapped trials, explicit CPU/read/buffer budgets and
per-worker checksum counts without changing the separate schedulers. The bounded
protocol and disposition are in
[DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md) -> `RC-D7` and `RC-D8`;
the [capture record](experiments/read-checksum/captures/2026-09-19-ep-x1-1/README.md)
retains the measurements and verification. The working proposal now links the
evidence requirements; `EP-X1.2` carries the repeat and role controls forward.

> **CROSS-COMPONENT PREREQUISITE:** parent `topology-planner` -> `EP-R1.7` selected
> this comparison using completed `read-checksum` -> `RC-1`; see
> [COMPLETED-CHECKLIST.md](experiments/read-checksum/COMPLETED-CHECKLIST.md#rc-1).
> **-> CROSS-COMPONENT HANDOFF:** return from `experiments/read-checksum` -> `MX1` ->
> `EP-X1.1` to `topology-planner` -> `MR1` -> `EP-R1.7` for the recorded result
> discussion before advancing to `EP-X1.2` in [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-09-19 00:47:15 -07:00 -- Faux-NUMA behavioral completion

### <a id="ep-r172"></a>EP-R1.7.2 -- Implement and adopt a consistent faux-NUMA test environment. *(completed 2026-09-19 00:47:15 -07:00)*

The existing experiment now uses one injected platform for gathering, binding,
allocation, CPU observations and residency. Its test-only faux provider shares
topology, logical binding and allocation state across the real schedulers; owned
fake payloads record processing and release. End-to-end tests exercise both transfer
directions, changed topology, sparse nodes/groups, unknown/mismatched residency,
refusals and cleanup after partial setup and processing failures. Sabotage confirms
that bypassing gathering, binding or allocation, or ignoring the requested node,
is detected. Live tests remain a separate check of the real Windows provider.

Recorded as experiment [DESIGN-NOTES.md](experiments/read-checksum/DESIGN-NOTES.md)
-> `RC-D11`, applying [EP-D-11](DESIGN-NOTES.md#ep-d-11). Adoption by future production
components is explicitly carried into EP-R1.8 in [CHECKLIST.md](CHECKLIST.md), not
claimed implemented here. This item and EP-X2.1 landed together because the injected
boundary and its first actual-consumer validation are one coupled change.

### <a id="ep-x21"></a>EP-X2.1 -- Validate placement and directed buffer-transfer behavior. *(completed 2026-09-19 00:47:15 -07:00)*

Retain the separate schedulers, heap/NUMA allocation paths and generated companion
as experimental paths with no production tuning default. The prior
[local capture](experiments/read-checksum/captures/2026-09-19-ep-x2-1/README.md)
remains exploratory evidence. The new gathering-to-selection-to-scheduler tests
complete behavioral acceptance under EP-D-11; they do not model memory-distance
costs. Physical fidelity remains solely EP-HW.1, non-blocking for completion.

> **CROSS-COMPONENT PREREQUISITE:** parent `topology-planner` -> `EP-R1.7.2`
> supplies the consistent environment used by `experiments/read-checksum` -> `EP-X2.1`.
> **-> CROSS-COMPONENT HANDOFF:** return to `topology-planner` -> `MR1` ->
> `EP-R1.7.1` for scope reconciliation in [CHECKLIST.md](CHECKLIST.md).
> EP-X2.2 is not started by this completion; its hardware gate is discharged by policy,
> and its scope hold remains unchanged.

## Moved 2026-09-19 01:23:05 -07:00 -- Independent request/reply candidates

### <a id="ep-x22"></a>EP-X2.2 -- Compare shared completion service with assigned request lanes. *(completed 2026-09-19 01:23:05 -07:00)*

Implemented separate shared competing-receiver and assigned-lane layouts over the
same bounded MPMC channel backend. Both use equal worker/aggregate queue/credit
budgets, deterministic steady/burst and skewed service traces, a fixed blocking
idle policy, correlated terminal replies and joined cancellation/rundown. The
common verifier checks identities, reference results, owner assignments, scheduled
latency origin, credit conservation and worker/per-lane census. Tests accept legal
reordering and reject corruption, ownership errors and resource violations.

The [demonstration](experiments/request-reply/captures/2026-09-19/README.md) retains
raw outcomes, per-lane/aggregate response distributions, service-active wall time,
pressure and same-code retakes. No pinning, NUMA inference, throughput ranking or
performance acceptance threshold was introduced. Both candidates are retained
under [DESIGN-NOTES.md](experiments/request-reply/DESIGN-NOTES.md) -> `RR-D4`.
The working proposal now points to its ownership/admission/outcome requirements.

Package tests, live CLI checks, doctests, formatting, workspace Clippy and sabotage
passed. Real routing, correlation, timestamp-origin, credit and verifier-bypass
defects were caught; the equivalent compute-chunk control survived. The required
arrival traces were brought into this item by explicit authorization; EP-X1.4's
remaining workload-specific comparisons are not marked complete by that work.

> **CROSS-COMPONENT PREREQUISITE:** `experiments/read-checksum` -> `EP-X2.1`
> supplied its completed behavioral coverage and shared fidelity policy.
> **-> CROSS-COMPONENT HANDOFF:** return from `experiments/request-reply` -> `MX2`
> -> `EP-X2.2` to parent `topology-planner` -> `MR1` -> `EP-R1.7.1` in
> [CHECKLIST.md](CHECKLIST.md). Later experiments remain paused for scope reconciliation.
