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
