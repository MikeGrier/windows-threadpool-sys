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

## DI-M16 -- Hand the request crate the handles already held

- [ ] **DI-16.1** -- **`win-durable-ioring` stops duplicating handles it holds.** The request crate's `NR-6`
  ([D-32](../windows-namespace-request-sys/DESIGN-NOTES.md#d-32)) has its entries hold a `SharedHandle`. An
  `OpenedFile` already holds one, yet `final_location` in
  [opened_file.rs](../win-durable-ioring/src/windows_backend/opened_file.rs) and `HandleKindQuery::of` in
  [handle_kind.rs](../win-durable-ioring/src/windows_backend/handle_kind.rs) duplicate it to ask its final path and
  kind. Both take the `SharedHandle` instead and give the entry a clone; `HandleKindQuery::of` changes signature,
  so this is breaking for `win-durable-ioring`, in a commit of its own. `OpenedVolume` and `OpenedDisk` are
  checked for the same pattern. Tests: the existing ones, and that asking an opened file's kind makes no handle.
  Lands after `NR-6.1` and before `NR-6.2`.

  > **CROSS-COMPONENT PREREQUISITE:** [NR-6.1](../windows-namespace-request-sys/CHECKLIST.md).

## DI-M11 -- The core driven by script

- [ ] **DI-11.1** -- **An in-memory backend whose targets and backend fault on a script**
  ([DI-D-50](DESIGN-NOTES.md#di-d-50) P13), public and deterministic, in the portable crate: an
  operation failed, held, or completed late or out of order; a submission refused; a full queue; a
  short count, to test a backend's short-write policy. It answers the problem that flush failure
  cannot be produced on a healthy machine, for consumers' failure paths and not only this crate's,
  in the spirit of [RESPONSE-SPACE.md](../windows-ioring-sys/RESPONSE-SPACE.md), and is the basis
  for unit testing and driving the core (the engineer: "In-memory is a good basis for unit testing
  and driving"). It does not replace DI-4.1.

- [ ] **DI-11.2** -- **The fault seam's merge-or-delete decision**, once `DI-11.1` is proven: the
  `fault-injection` feature's `fail_next_flush`, and the Windows backend's `fail_next_commit` added
  beside it in `DI-3.10.3.5.5`, duplicate what the in-memory backend scripts
  ([DI-D-50](DESIGN-NOTES.md#di-d-50)), and stay until then under the duplicate-then-decide rule.

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
