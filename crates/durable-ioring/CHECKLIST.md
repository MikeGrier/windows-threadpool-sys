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

- [ ] **DI-12.1** -- **Emit the delay events** DI-2.11 designs, as typed events the core reports to its diagnostic
  sink, and **the Windows sinks** ([DI-D-51](DESIGN-NOTES.md#di-d-51)). Restructured on 2026-10-20 into `DI-12.1.1` to
  `DI-12.1.6` after the engineer's answers ([DI-D-76](DESIGN-NOTES.md#di-d-76)); it is done when they all are. What
  DI-2.11 left to implementation is settled, and each step carries its part:
  - **Identity per event** is the instance plus what the event is about: a lock wait names the lock, a flush the
    lineage, the epoch and the domain.
  - **The time base** is interrupt time, dioring's one ([DI-D-37](DESIGN-NOTES.md#di-d-37)).
  - **The provider** is `durable-ioring-events`, in a crate of its own, and records whether or not its manifest is installed.

- [x] **DI-12.1.1** -- The delay events are in the portable core: lock waits, flush issue and answer, and a seal's wait, built only for a sink that wants them. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1211)

- [x] **DI-12.1.2** -- `win-debugging-sys` is created, with `output_debug_string` and `output_debug_str` over `OutputDebugStringW`, and a NUL refused as the crate's own error. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1212)

- [x] **DI-12.1.3** -- `DebugOutputDiagnostics` writes each event as a `dioring:` line to the debugger, through `win-debugging-sys`, and wants every kind. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1213)

- [x] **DI-12.1.4** -- `win-etw-sys` is created: a `Registration` over `EventRegister`, `EventUnregister`, `EventEnabled`, `EventProviderEnabled` and `EventWrite`, with an event's data built from typed fields. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1214)

- [x] **DI-12.1.4.1** -- `win-etw-sys`' session tests were run elevated and pass, and what they observed is recorded in the crate's notes. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-12141)

- [ ] **DI-12.1.5** -- **Create `durable-ioring-events`: the provider, its manifest and `EtwDiagnostics`.** The provider is
  named `durable-ioring-events` (the engineer, 2026-10-20: "Change the name of the event source to
  durable-ioring-events", having first said "win-dioring, I'd put it in its own crate"), with a GUID generated
  once and recorded in the crate's notes. Its manifest ships with the crate and maps each event of `DI-12.1.1`
  one-to-one. It records whether or not the manifest is installed ("you don't actually have to install the manifest to
  record events"); the crate's documentation says that a reader needs it to see names and fields, and how to install it.
  An event is built only when a session is listening, so it costs nothing otherwise. Registered with the workspace,
  release-please and the publish workflow. Tests: each event reaches a session, and none is built when nobody listens.

  > **CROSS-COMPONENT PREREQUISITE:** `DI-12.1.1` and `DI-12.1.4`, in this checklist.

- [ ] **DI-12.1.6** -- **`win-durable-ioring`'s default sink is `EtwDiagnostics`**, so a consumer who chooses nothing
  still gets the events ([DI-D-51](DESIGN-NOTES.md#di-d-51) T4), and any sink, `NoDiagnostics` included, can be passed
  when the driver is built. Tests: the default, and a chosen sink replacing it.

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
