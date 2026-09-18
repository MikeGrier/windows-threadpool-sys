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
