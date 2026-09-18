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
