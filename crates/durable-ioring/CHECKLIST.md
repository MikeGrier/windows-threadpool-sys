# Checklist: durable-ioring

What this component is: [COMPONENT.md](COMPONENT.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md).
The session behind both: [DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).

DI-M1, the contract, and DI-M2, the API shape, are complete -- see [CONTRACT.md](CONTRACT.md),
[API.md](API.md) and the archive in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Code began at
DI-M3, which closed on 2026-10-11 with its open work restructured into DI-M7 to DI-M12, flatter,
because its items had nested six levels deep; the table from its old IDs to the new ones is in [the
archive](COMPLETED-CHECKLIST.md#di-m3-restructured), and older records still cite the old IDs. The
new milestones are numbered after the parked DI-M5+ and DI-M6+, which are cited too widely to
renumber. The flush-failure spike (DI-M4) runs alongside and gates nothing. This file holds only work in
scope now; parked work, gated (`M{n}+`), on the horizon or deferred, is in [PARKED.md](PARKED.md).

## DI-M12 -- Beside the core: diagnostics, routing, and release

- [ ] **DI-12.1** -- **Emit the delay events** DI-2.11 designs, as typed events the core reports to
  its diagnostic sink, and **the Windows sinks** in `win-durable-ioring`: `EtwDiagnostics`, from a
  manifest-based ETW provider that dioring owns and the Windows driver's default, and
  `DebugOutputDiagnostics`, through `OutputDebugStringW` ([DI-D-51](DESIGN-NOTES.md#di-d-51)).
  Gated on `DI-8.2`, which creates `win-durable-ioring`. Carries what DI-2.11 deliberately left to
  implementation:
  - **Identity per event** -- what each event names (instance, lineage, epoch id, domain, lock),
    "going to depend on the event" (the engineer), and how the generic epoch id is rendered.
  - **The time base** is settled: interrupt time, dioring's one time base
    ([DI-D-37](DESIGN-NOTES.md#di-d-37)), until a requirement for finer resolution appears.
  - **The provider's name and GUID**, and whether it stands alone or joins a workspace scheme.
  - **Installing the manifest**: how an application shipping dioring registers it, or documents
    that it must.

- [ ] **DI-12.2** -- **The routing provider** (DI-2.12 point 1): a provider dioring ships that
  dispatches each seal's work by flush domain to providers registered with it when it is built,
  refuses a domain routed twice at build time, and passes each domain's outcome back as it arrives.
  It reports to the instance's diagnostic sink ([DI-D-51](DESIGN-NOTES.md#di-d-51)), so its
  diagnostics go wherever the consumer sent the instance's. Tested alone against fake providers.
  Gated on the provider trait from DI-2.12.

- [ ] **DI-12.3** -- **The `epoch_log` merge-or-delete decision**, once DI-3.2 is proven
  ([DI-D-7](DESIGN-NOTES.md#di-d-7)).

  > **-> UNBLOCKS PARKED WORK:** completing `DI-M12` (this item, `DI-12.1` and `DI-12.2`) unblocks
  > [DI-M5+](PARKED.md#di-m5----the-retention-layer-parked-behind-di-m12) and
  > [DI-M6+](PARKED.md#di-m6----deliberately-coarse-parked-behind-di-m12) in [PARKED.md](PARKED.md).

- [x] **DI-12.4** -- The five path-only dependencies of `win-durable-ioring` are pinned by version, keeping the path. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#di-124)

## DI-M18 -- Each failure test at the layer that owns what it checks

Queued by the engineer, 2026-10-20, from the question [DI-D-75](DESIGN-NOTES.md#di-d-75) left unruled: both paths for
fault injection stay, and the tests in `win-durable-ioring` that only check how the core answers a failure move to the
portable crate, where the in-memory backend's script ([DI-D-74](DESIGN-NOTES.md#di-d-74)) produces it without a
Windows ring. What stays at the Windows level is what the Windows driver adds.

- [ ] **DI-18.1** -- **Census every test and example that arms the Windows fault seam, and classify each.** The census is
  produced by a search recorded with it, for `fail_next_flush` and `fail_next_commit` across `win-durable-ioring`'s
  source, tests and README, and not from recollection. Each is one of: *core* -- its assertions are about how the core
  answers a failed flush (an epoch blocks, which writes are suspect, a heal, a lineage, a marking, a stamp, how a
  lineage ends) and name nothing the Windows driver adds; *windows* -- its assertions are about what the driver adds (a
  Win32 code reaching its `ErrorCode` and back through `win32_code_of`, the real ring, the delivery, a worked example in
  the README); or *both*, which is split. The classification is recorded beside the census, with the reason for each.

- [ ] **DI-18.2** -- **Move each *core* test to `durable-ioring` over the script, with the same assertions.** It goes
  where the behaviour lives (`io_core/tests` or `durability/tests`), built on the in-memory backend's script and not on
  a detached target. A *both* test keeps its Windows half where it is. Nothing is deleted yet: the moved test must pass
  in its new place first. For each, the sabotage entries it was the catching test for, in either crate's manifest, are
  still caught once it has moved, by it or by another test, which is run and not assumed.

- [ ] **DI-18.3** -- **Delete the moved tests from `win-durable-ioring`.** What remains there is the *windows* set. The
  seam itself and the README's examples stay ([DI-D-75](DESIGN-NOTES.md#di-d-75)). Both crates' tests pass, and the
  sabotage entries checked in `DI-18.2` are still caught.

## DI-M4 -- Evidence: Windows' flush-failure behaviour (ungated; gates nothing)

- [ ] **DI-4.1** -- **The flush-failure spike** ([D-44](../windows-ioring-sys/DESIGN-NOTES.md#d-44)).
  Make a flush fail on a real Windows stack and observe whether a later flush covers, or hides, the
  earlier loss -- separately for buffered and `NO_BUFFERING` handles -- whether a rewrite plus a
  successful flush re-establishes durability, and whether a write-back failure is reported to every
  handle open on the file or only to one (which decides how often the external-failure import of
  DI-1.2 is needed), and whether another handle's failed flush on the same device observably loses
  this handle's unflushed `NO_BUFFERING` writes. Windows documents none of this; the contract is
  justified on the permitted response space and does not wait for the answer. Producing the failure
  on demand is the hard part (a detachable VHD or an error-injecting filter are the candidates).
  What the published record does say -- Windows' own "the data has been lost" write-behind codes,
  Microsoft assigning the handling to the application, and nothing on retention after a failed
  flush -- was gathered on 2026-10-09 and is in
  [the design session](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md#what-the-published-record-says-research-2026-10-09).
