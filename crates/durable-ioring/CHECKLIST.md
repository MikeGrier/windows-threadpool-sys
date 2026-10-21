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

- [x] **DI-12.1** -- The delay events are emitted as typed events the core reports to its sink, and the Windows sinks exist: the debugger sink, the ETW provider in its own crate, and the ETW sink as the driver's default. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-121)

- [x] **DI-12.1.1** -- The delay events are in the portable core: lock waits, flush issue and answer, and a seal's wait, built only for a sink that wants them. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1211)

- [x] **DI-12.1.2** -- `win-debugging-sys` is created, with `output_debug_string` and `output_debug_str` over `OutputDebugStringW`, and a NUL refused as the crate's own error. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1212)

- [x] **DI-12.1.3** -- `DebugOutputDiagnostics` writes each event as a `dioring:` line to the debugger, through `win-debugging-sys`, and wants every kind. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1213)

- [x] **DI-12.1.4** -- `win-etw-sys` is created: a `Registration` over `EventRegister`, `EventUnregister`, `EventEnabled`, `EventProviderEnabled` and `EventWrite`, with an event's data built from typed fields. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1214)

- [x] **DI-12.1.4.1** -- `win-etw-sys`' session tests were run elevated and pass, and what they observed is recorded in the crate's notes. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-12141)

- [x] **DI-12.1.5** -- `durable-ioring-events` is created: the provider, its manifest rendered from one table of events, and `EtwDiagnostics`, which wants a kind only when a session listens. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1215)

- [x] **DI-12.1.6** -- `win-durable-ioring`'s default sink is `EtwDiagnostics`, so an instance built with none writes its delay events to ETW, and a sink given replaces it. -> [completed 2026-10-20](COMPLETED-CHECKLIST.md#di-1216)

- [ ] **DI-12.2** -- **The routing provider** (DI-2.12 point 1, which called it `DI-3+.8`): a provider dioring ships that
  dispatches each seal's work by flush domain to providers registered with it when it is built, refuses a domain routed
  twice at build time, and passes each domain's outcome back as it arrives. The core holds the built-in default and at most
  one consumer-supplied provider ([DI-2.12](COMPLETED-CHECKLIST.md#di-212), the engineer, 2026-10-06: "do your clean form of
  b"), so the router is that one provider, composed of others; it is portable and lives in `durable-ioring`. Restructured on
  2026-10-20 into `DI-12.2.1` to `DI-12.2.4`, because it is four pieces that can each be built and checked alone and one of
  them is blocked; it is done when they all are. The gate this item carried, the provider trait, is met: `DurabilityProvider`
  and `DomainCompletion` exist. Two questions are open and are not decided, and each step says which it waits on:
  - **What the router reports.** The core already reports each domain's flush issued and answered, with its name and how long it
    took ([DI-D-77](DESIGN-NOTES.md#di-d-77)), so what a router would add is not settled; and a router is built before the
    instance, so it does not know the instance number an `Inconsistency` event carries. Candidate answers: nothing but an
    inconsistency for a domain no provider owns; or a per-provider event for how long that provider's call took, from dispatch
    to its last domain answering; or leave it coarse until the router exists and the core's events show whether anything is
    missing, which is a legitimate answer.
  - **What decides the domains a registered provider serves.** The provider's own `domains()`, which the trait already has, or
    domains named to the router at registration, which would let one provider be registered for a subset and let a router's
    domains disagree with what the provider says it serves.

- [ ] **DI-12.2.1** -- **The router and its build.** A `RoutingProvider` in `durable-ioring`, itself a `DurabilityProvider`,
  built from providers registered with it; its `domains()` is the union of theirs, read once as the trait says. A domain
  routed to two providers is refused at build, naming the domain and both registrations: earlier and clearer than the core's
  `DuplicateDomain` for a single provider. Waits on the second open question of `DI-12.2` for how a provider's domains are
  known; until it is answered the provider's own `domains()` is the working position and is not decided. Tests: each of one,
  two and many providers, a provider that serves no domain, a router with no provider, a domain routed twice by adjacent and by
  distant registrations, and the union's order.

- [ ] **DI-12.2.2** -- **Dispatch.** `make_durable` splits a request by the provider that owns each domain and makes one call
  per provider that has parts, in registration order and with each provider's parts in the request's order. Each
  `DomainCompletion` is handed to its owner unchanged, so a domain's outcome returns as it arrives and nothing is buffered or
  re-answered. A panic in one provider propagates, as the trait says, and the providers not yet called have their completions
  dropped, which is `ProviderAbandoned` for their domains. A domain no provider owns cannot come from the core; it fails that
  domain and is not a panic. Tested alone against fake providers that answer inline, answer late from another thread, never
  answer, drop their completions, and panic, with fake completion homes, and with several domains per provider.

- [ ] **DI-12.2.3** -- **What the router reports.** BLOCKED on the first open question of `DI-12.2`: what, if anything, the
  router adds to the diagnostics the core already reports, and how it knows the instance's number. When answered this becomes
  the router's events or the decision that it reports only the one inconsistency; until then nothing is built, and this is
  recorded as a blocker and not as "no consumer".

- [ ] **DI-12.2.4** -- **The router through the driver, and its record.** A router given as `Setup::provider` to a real
  instance over two fake providers: a seal that touches both is one call to each; a file whose domains span providers is durable
  only when every one succeeds (DI-2.12 point 5); one provider failing fails only its own domains; coupled domains split across
  providers are correct and cost physical flushes, documented and not enforced. The README and
  [API.md](API.md) say how to build one; the decision is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md); the sabotage manifest
  gets entries and a control.

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
