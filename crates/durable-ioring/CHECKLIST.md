# Checklist: durable-ioring

What this component is: [COMPONENT.md](COMPONENT.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md).
The session behind both: [DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).

DI-M1, the contract, and DI-M2, the API shape, are complete -- see [CONTRACT.md](CONTRACT.md),
[API.md](API.md) and the archive in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Code begins
at DI-M3. The flush-failure spike (DI-M4) runs alongside and gates nothing. Headings marked `+` are parked behind the milestone they name, per the workspace's `M{n}+`
convention.

## DI-M3 -- First implementations

Graduated from the parked DI-M3+ on 2026-10-07, once DI-M2 was complete; its items were renumbered
from `DI-3+.n` to `DI-3.n`, which older records still cite.

- [x] **DI-3.1** -- The crate is scaffolded: a workspace member with `#![forbid(unsafe_code)]`, registered for release and publication. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-31)

- [ ] **DI-3.2** -- **dioring over `windows-ioring-sys`**, implementing the contract in
  [CONTRACT.md](CONTRACT.md), in the steps `DI-3.2.1` to `DI-3.2.7` below; checked when the last of
  them is. Re-planned 2026-10-07: as one item it covered the whole contract, and two of its
  requirements sat on items numbered after it. Each step lands what it implements, with its tests,
  and adds to the trait only the methods it implements -- the crate is unreleased, so the trait
  grows rather than shipping `todo!()` bodies. Two dependencies on later items were settled by the
  engineer on 2026-10-07, each by bringing the work forward:
  - **The worked-example doctests** land in `DI-3.2.4`, on a fault seam built there, rather than
    waiting for the fault-injecting implementation, `DI-3.3`.
  - **The conformance oracle** ([DI-D-24](DESIGN-NOTES.md#di-d-24)), formerly `DI-3.7`, is folded
    into these steps: `DI-3.2.2.1` builds it, and each later step adds the rules it implements, so
    dioring's own tests bind to it from the start. It is a reusable checker over the event stream,
    owned by dioring, that every implementation of the trait runs in its tests -- dioring's own,
    the fault-injecting one (`DI-3.3`), and layers above. Modelled on `windows-file-watcher`'s
    `ContractChecker`: it accepts the legal-but-surprising sequences as carefully as it rejects the
    illegal ones, and says which rules the stream cannot show.

  **Every later step that adds a trait operation a consumer calls on a live instance also adds its
  `&self` form to `EntryDelivery`'s `DeliveryHandle`** ([DI-D-32](DESIGN-NOTES.md#di-d-32)): the
  handle mirrors those operations rather than lending the instance, so nothing adds them for it.
  `pop` and `readiness` stay off it.

- [x] **DI-3.2.1** -- Types, trait and construction: the shared types, identities and trait in code, and `Dioring::new` with its refusals. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-321)

- [x] **DI-3.2.2.1** -- Plain I/O and the delivery every front end shares: pushes, `pop`, the readiness `Event`, and the conformance oracle with its readiness check. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-3221)

- [x] **DI-3.2.2.2** -- The Model A front end: `EntryDelivery` and its `DeliveryHandle`, delivering entries one at a time to a handler on pool threads. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-3222)

- [x] **DI-3.2.3** -- Seals and durability in one lineage, through the built-in default provider: `make_durable_through`, `durable_through`, `sealed_through` and `epoch_state`, with `Durable` reported as a prefix after the completions it covers. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-323)

- [x] **DI-3.2.4** -- Failures and their resolution: `Failed`, heal, abandon and close, `resolve`, `Blocked`, `import_failure`, the inventory, and a fault seam that drives CONTRACT.md's worked examples as doctests. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-324)

- [x] **DI-3.2.4.1** -- A write tagged with an abandoned epoch is refused as `EpochAbandoned`, open or sealed, per the engineer's decision DI-D-35. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#di-3241)

- [ ] **DI-3.2.4.2** -- **Nullifiers** ([DI-D-36](DESIGN-NOTES.md#di-d-36), the engineer's direction):
  a suspect write that completes as failed after its failure was observed gains an append-only
  marking in that failure's record, carrying the completion's error and when it was observed, so
  the history can be reconciled after the fact. Reporting only: the mark, what holds it, and what
  resolution needs are unchanged. Amends CONTRACT.md's "frozen ... and never grows" to "never gains
  a write", and DI-D-12 (b). Everything DI-D-36 allows for is built here: markings in the inventory
  and the `Abandoned` entry, and as queue entries of their own; a marking for a short write, with its
  transferred count; a marking type open to further kinds, starting with a suspect write later
  covered by a successful flush; and a failure's final record surviving its resolution, including a
  heal, which needs an entry of its own. A `Failed` entry's own observation time lands first, with
  the dependency, under win-time-sys' `WT-2.2.2`; every
  failure, synchronous or asynchronous, carries its error code -- a failed completion's and a failed
  flush's alike -- in a representation chosen here, which also settles how a consumer reads the code
  without `windows-ioring-sys`' `IoRingErrorExt` (DI-D-34's question). Timestamps are interrupt time
  ([DI-D-37](DESIGN-NOTES.md#di-d-37)). The oracle gains the rules: a nullifier names a write in the
  set whose completion reported `Failed`, a short-write marking one whose completion was short, and
  a covered marking one in a seal that later succeeded.

  > **CROSS-COMPONENT PREREQUISITE:** a safe interrupt-time read, which dioring cannot write itself
  > (DI-D-37). It comes from `win-time-sys`, created for it: component `crates/win-time-sys` ->
  > `WT-M1` -> `WT-1.3` (interrupt time) -- landed, see
  > [COMPLETED-CHECKLIST.md](../win-time-sys/COMPLETED-CHECKLIST.md#wt-13) -- and dioring's
  > dependency on it, with the `Failed` entry's timestamp: `WT-M2` -> `WT-2.2.2`, see
  > [CHECKLIST.md](../win-time-sys/CHECKLIST.md).

- [ ] **DI-3.2.5** -- **Lineages, gates and flush domains.** `mint_lineage`, `lineages()` and
  `default_lineage()`, with every rule of the steps above holding per lineage
  ([DI-D-19](DESIGN-NOTES.md#di-d-19)). Gated operations held until their epoch is durable, ending
  as `NeverIssued` when it is abandoned, a gate on an abandoned epoch refused as `GateAbandoned`,
  and a gate cycle refused as `GateCycle` ([DI-D-23](DESIGN-NOTES.md#di-d-23)). Make
  `WriteOptions::gate` and `ReadOptions::gate` public again: `DI-3.2.2.1` made them crate-private,
  because until gates are honoured a public setter would let a gated operation be issued ungated.
  Failures become the instance's rather than the default lineage's: a suspect set spans every
  lineage's held writes, a failure belongs to each lineage it suspects a write of, and
  `ImportScope::Lineage` narrows to that lineage's writes. (Flush-domain reach for flush failures
  and imports landed in `DI-3.2.4`, where the suspect set is defined -- [DI-D-34](DESIGN-NOTES.md#di-d-34);
  containment for a provider's domain is `DI-3.2.6`'s.) **Open, for the engineer, before this
  step:** what `epoch_state` answers for an
  epoch of a lineage the instance does not have. `EpochState` has no value for it, so since
  `DI-3.2.3` dioring panics there ([DI-D-33](DESIGN-NOTES.md#di-d-33)); a retired lineage raises
  the same question, which is why it is settled here.

- [ ] **DI-3.2.6** -- **The consumer's durability provider.** `Setup::provider` and the domains it
  serves; one `FlushRequest` per seal, naming files and `OpId`s per domain; `DomainCompletion`
  answered once from any thread, with a failure as `Cause::Provider` and a completion dropped
  unanswered as `Cause::ProviderAbandoned`; the readiness signal set by provider answers
  ([DI-D-27](DESIGN-NOTES.md#di-d-27)). A write is durable when every domain of its file has
  succeeded. A provider's failure of domain D reaches the held writes of every file whose declared
  domains contain D, and every file declared with none ([DI-D-21](DESIGN-NOTES.md#di-d-21)); it
  is observed through the same transition as a failed flush, with its own `Reach`.

- [ ] **DI-3.2.7** -- **Ending.** `remove_file` refused while the file has anything in flight, held
  or uncovered, reported whole in `FileBusy`; `retire_lineage` and `LineageBusy`; `end_lineage` and
  `LineageEnded` ([DI-D-30](DESIGN-NOTES.md#di-d-30)); and `close` and drop, waiting for every
  kernel operation and provider call in flight and returning `Leftovers`
  ([DI-D-25](DESIGN-NOTES.md#di-d-25)), with no callback after the end and tracing rather than
  panicking while unwinding.
- [ ] **DI-3.3** -- **A fault-injecting implementation for consumers' tests**: failed flushes,
  short writes and failed points on demand, deterministically. It answers the problem that flush
  failure cannot be produced on a healthy machine, for consumers' failure paths and not only this
  crate's, in the spirit of
  [RESPONSE-SPACE.md](../windows-ioring-sys/RESPONSE-SPACE.md). It does not replace DI-4.1.

- [ ] **DI-3.4** -- **The `epoch_log` merge-or-delete decision**, once DI-3.2 is proven
  ([DI-D-7](DESIGN-NOTES.md#di-d-7)).

- [ ] **DI-3.5** -- **Pin `windows-ioring-sys` by version once the set of changes is about to
  release** ([DI-D-16](DESIGN-NOTES.md#di-d-16)). Gated on that release, not on DI-3.4. Add the
  `version` of the `windows-ioring-sys` release carrying everything dioring relies on (at least
  [D-80](../windows-ioring-sys/DESIGN-NOTES.md#d-80) and
  [D-81](../windows-ioring-sys/DESIGN-NOTES.md#d-81)), keeping the `path`. This must land before
  dioring's first release PR merges, because its publish job fails on a path-only dependency.
  `DI-3.2.2.1` added two more path-only dependencies, pinned the same way: `win-sync-sys` (the
  readiness signal's `Event`), at its first release, and `windows-threadpool-sys` (the readiness
  check's wait), at the release carrying its `M-T14.1` (`From<Event> for WaitableHandle`).
  win-time-sys' `WT-2.2.2` adds a fourth, `win-time-sys` (the failure timestamps' clock), at its
  first release.

- [ ] **DI-3.6** -- **Emit the delay events** DI-2.11 designs, from a manifest-based ETW provider
  that dioring owns. Gated on DI-3.2, which creates the code the events describe. Carries what
  DI-2.11 deliberately left to implementation:
  - **Identity per event** -- what each event names (instance, lineage, epoch id, domain, lock),
    "going to depend on the event" (the engineer), and how the generic epoch id is rendered.
  - **The time base** is settled: interrupt time, dioring's one time base
    ([DI-D-37](DESIGN-NOTES.md#di-d-37)), until a requirement for finer resolution appears.
  - **The provider's name and GUID**, and whether it stands alone or joins a workspace scheme.
  - **Installing the manifest**: how an application shipping dioring registers it, or documents
    that it must.

- [x] **DI-3.7** -- **Folded into `DI-3.2` by the engineer, 2026-10-07: the conformance oracle
  is built from `DI-3.2.2.1`, and each later step adds its rules.** What the oracle is, and the
  readiness signal's harness check beside it, are stated under `DI-3.2` and `DI-3.2.2.1`.

- [ ] **DI-3.8** -- **The routing provider** (DI-2.12 point 1): a provider dioring ships that
  dispatches each seal's work by flush domain to providers registered with it when it is built,
  refuses a domain routed twice at build time, and passes each domain's outcome back as it
  arrives. It emits through dioring's own ETW provider ([DI-D-26](DESIGN-NOTES.md#di-d-26)), so
  the diagnostic surface stays one. Tested alone against fake providers. Gated on the provider
  trait from DI-2.12.

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

## DI-M5+ -- The retention layer (parked behind DI-M3)

- [ ] **DI-5+.1** -- **A separate crate that retains written data until it is durable and can rewrite
  it after a failure**, built on DI-1.2's resolution protocol and the inventory, and resolving
  overlapping suspect writes newest-wins, and resolving at the granularity of individual buffers it
  can reissue (DI-1.2 Q6). The layer [DI-D-5](DESIGN-NOTES.md#di-d-5) moves out of
  this crate; its first natural consumers are log-less designs such as FAT. Needs its own name and
  component when it begins.

## DI-M6+ -- Deliberately coarse (parked behind DI-M3)

Left coarse on purpose, per the workspace's resolution-gradient rule: each needs a working
implementation before it is answerable.

- [ ] **DI-6+.1** -- **Measure alternating rings**, which stretch "one durability sequence per
  instance" to two `IoRing`s, and the conditions under which dioring's own use of a ring barrier
  beats completion-gated flushes (DI-6+.5). The consumer-facing strategy comparison is gone:
  [DI-D-22](DESIGN-NOTES.md#di-d-22) offers the consumer no choice.

- [ ] **DI-6+.5** -- **Choose the flush mechanism dynamically.** The engineer, 2026-10-06: "as
  perf optimizations we may want to note if specialized flush domains are in use at all, or which
  are in a given ring so that we may apply a different synchronization strategy dynamically."
  [DI-D-22](DESIGN-NOTES.md#di-d-22) keeps the consumer from choosing; this is dioring choosing
  for itself, per seal, where a ring barrier adds no coupling of its own. Working position, coarse:
  - *State to keep:* whether any file declares domains at all (if none does, every file
    intersects every other, so no lineages are separable and the barrier cannot break the
    expectation), and per interned domain a count of operations outstanding on the ring, so a seal
    can tell whether everything it would wait on intersects its own domains.
  - *What the barrier buys:* the flush can go into the same submission as the writes it covers,
    with no round trip -- the case of a log append followed by a commit on one file.
  - *What it still costs when allowed:* it waits on unrelated earlier work, including reads and
    the lineage's own later epochs, and N flushes run one after another, so it suits few files
    on a quiet ring. Which policy wins is workload-dependent, so this item restores a
    measurement for DI-6+.1 to carry, and DI-2.11's events record which mechanism each seal used.
  - *The common case:* the engineer, 2026-10-06: "at least "naive" meaning no files with any flush
    domains set will be common." That is the case where the barrier is always admissible, decided
    by one flag rather than per-domain counts, so it is the first case to build and measure --
    the choice there is purely one of performance.
  Applies to the default provider (DI-2.12), the only one that uses dioring's wioring.
  Gated on DI-3.2.

- [ ] **DI-6+.6** -- **Make the default provider cheaper than per-file sync.** From DI-2.12 point 1;
  the engineer, 2026-10-06, asked these be recorded as "optimizations we should strongly consider",
  then: "this is a performance feature that we will tackle during development and if we can't make
  it safe, we will omit it. It is not required for shipping." Gated on DI-3.2. Candidates:
  - *One device sync per flush domain, for declared cache-free files.* Write-through and no
    buffering "are good for the filesystem cache but don't issue the flush at the device level, and
    fua is largely ignored nowadays" (the engineer), so such files still need a device sync. The
    consumer declares in `FileOptions` that a handle was opened `FILE_FLAG_NO_BUFFERING` with
    `FILE_FLAG_WRITE_THROUGH`, and dioring issues one syncing flush per flush domain, on one of its
    declared files. **The open safety question, settled during development or the feature is
    omitted:** Windows does not document that a flush on one file commits the device cache behind
    the others; if adopted, that becomes part of the consumer's warranty for the domain, not an
    assumption of dioring's. Also open: the representative flush should use the default mode, since
    a cheaper mode on a clean file may not send the device sync. A failed flush puts the whole
    domain at risk, as DI-D-21 already scopes.
  - *One flush per volume*, opt-in: `FlushFileBuffers` on a volume handle flushes "all open files on
    a volume", but "The caller must have administrative privileges" and it flushes other processes'
    files too. Whether IoRing's flush accepts a volume handle is unverified.
  - *A cheaper flush mode per file* (`Data`, `MinMetadata`), chosen in `FileOptions`.

- [ ] **DI-6+.8** -- **A completion fence.** From DI-2.6 point 3; parked, neither added nor dropped,
  because neither the engineer nor the assistant sees a use yet ("I assume the use of it would be
  the next layer up"). What it would be: a consumer-posted entry delivered by `pop` only once every
  operation pushed before it has completed -- a completion sequencing point, saying nothing about
  durability. Submission order needs no marker, since `OpId` is ordered by push order
  ([DI-D-15](DESIGN-NOTES.md#di-d-15)). dioring would deliver the fence from its own bookkeeping
  rather than a drain-flagged `Nop`, which would wait on everything on the ring -- the coupling
  [DI-D-22](DESIGN-NOTES.md#di-d-22) avoided -- and could therefore scope it to a lineage or a set
  of files. A likely user is the retention layer (DI-5+.1), releasing buffers at a fence.

- [ ] **DI-6+.7** -- **Credit one provider call to other seals.** From DI-2.4 point 3, which settled
  the contract: a call must make the sealing lineage's named writes durable, and making more durable
  is "not incorrect, that's a performance issue" (the engineer, 2026-10-06). The optimization: a
  call names every completed, not-yet-durable write on the files it covers, whichever lineage made
  it, so its success can be credited and a later seal -- or one arriving while the call is in
  flight, for writes that completed before it was issued -- needs no call of its own. Crediting is
  safe only for writes the call named, since the provider's warranty covers what it was asked
  about; coverage nobody asked for is never credited. It adds no failure exposure (those writes
  share the files, so DI-D-21 already puts them at risk) and no coupling between separable
  lineages, which share no files. Applies to every provider, so it lives in dioring's core.
  Gated on DI-3.2.

- [x] **DI-6+.3** -- **Withdrawn: rebinding a file to a new handle is not this layer's**
  ([DI-D-11](DESIGN-NOTES.md#di-d-11)). Nothing remains to do here; the obligation it left is in
  DI-2.2.

- [ ] **DI-6+.4** -- **A dioring-mediated cancel.** Not in v1 (the engineer's lean): cancel
  guarantees no response time, so it does not deliver what shutdown or deadlines want, and draining
  serves shutdown already. When it is taken up, the accounting is that a cancelled write's outcome is
  indeterminate, so it joins the suspect set and its epoch cannot be reported durable until resolved.
  The v1 design must not foreclose adding it.
