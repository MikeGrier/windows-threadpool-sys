# Checklist: durable-ioring

What this component is: [COMPONENT.md](COMPONENT.md). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md).
The session behind both: [DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).

DI-M1, the contract, and DI-M2, the API shape, are complete -- see [CONTRACT.md](CONTRACT.md),
[API.md](API.md) and the archive in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Code began at
DI-M3, which closed on 2026-10-11 with its open work restructured into DI-M7 to DI-M12, flatter,
because its items had nested six levels deep; the table from its old IDs to the new ones is in [the
archive](COMPLETED-CHECKLIST.md#di-m3-restructured), and older records still cite the old IDs. The
new milestones are numbered after the parked DI-M5+ and DI-M6+, which are cited too widely to
renumber. The flush-failure spike (DI-M4) and the reviews (DI-M9) run alongside and gate nothing. Headings marked `+` are parked behind the milestone they name, per the workspace's `M{n}+`
convention.

## DI-M9 -- Reviews: the engineer confirms or amends the assistant's choices (ungated; gates nothing)

Each item is one choice the assistant made while building and the engineer has not yet confirmed.
The code binds to each already, because the steps could not be built without choosing; what is open
is the review. These reviews gate no other work (the engineer, 2026-10-10: "I cannot be the blocking
factor"). Where the engineer had already decided part of a row, that part is not here. A rejected
point is reworked as an item of the milestone current when it is rejected, with its decision row
amended. Every item carries one of three tags, which say what is asked:

- **DECIDE** -- the choice changes what a consumer can do or must avoid, or trades against a real
  alternative. The action is to accept it or amend it; "accept 9.1.1" is a complete answer. Each says
  what the code does now.
- **RATIFY** -- a name, a placement, an internal shape or a test method. It stands unless the
  engineer objects; "ratify 9.3.3" closes it.
- **MINE** -- not a choice: a gap or check the assistant owes. Nothing is asked of the engineer.

A group may be answered whole ("accept all DECIDE in 9.1"). The wording of each choice as first
written, which these items condense, is in git at the revision before the one that restructured this
milestone; the decision rows named below carry the reasons.

### DI-9.1 -- Lineage handles ([DI-D-42](DESIGN-NOTES.md#di-d-42), from `DI-3.2.5.1`)

- [x] **DI-9.1.1** -- A lineage handle's release runs on whichever thread drops the last copy. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-911)

- [x] **DI-9.1.2** -- An ended lineage is not live from the moment it ends. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-912)

- [x] **DI-9.1.3** -- Heals take effect as soon as nothing can delay them. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-913)

- [x] **DI-9.1.4** -- The oracle drops DI-D-39's tightening, and gains four rules for an ended lineage. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-914)

- [x] **DI-9.1.5** -- The lineage handles' shapes. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-915)

### DI-9.2 -- Gates and seals ([DI-D-43](DESIGN-NOTES.md#di-d-43), from `DI-3.2.5.2`; the refusal of a seal that closes a cycle is the engineer's own)

- [x] **DI-9.2.1** -- A held write whose own epoch is abandoned ends as `NeverIssued` naming that epoch. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-921)

- [x] **DI-9.2.2** -- A held write is outstanding for its lineage's seals, and no failure suspects it until it is released. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-922)

- [x] **DI-9.2.3** -- `make_durable_through` returns `SealError::{UnknownLineage, GateCycle}` in place of `io::Error`. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-923)

- [x] **DI-9.2.4** -- The gate and seal shapes, and the oracle's gate rules. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-924)

### DI-9.3 -- The provider ([DI-D-44](DESIGN-NOTES.md#di-d-44), from `DI-3.2.6`; read it with its amendments by DI-D-46 and DI-D-50)

- [x] **DI-9.3.1** -- The provider is called only from the consumer's next call, with dioring's lock released. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-931)

- [x] **DI-9.3.2** -- Every provider request is built before any is handed over. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-932)

- [x] **DI-9.3.3** -- The provider request's shape, and when a write is covered. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-933)

### DI-9.4 -- Faults, diagnostics, the clock ([DI-3.10.3.1 to DI-3.10.3.4](COMPLETED-CHECKLIST.md))

- [x] **DI-9.4.1** -- On Windows the default diagnostic sink is `NoDiagnostics`, until `DI-12.1`. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-941)

- [x] **DI-9.4.2** -- A diagnostic sink is called with dioring's lock held. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-942)

- [x] **DI-9.4.3** -- Two `IoFault`s are equal when they name the same source and code and share the one report. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-943)

- [x] **DI-9.4.4** -- The fault, diagnostic and clock renames and shapes. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-944)

### DI-9.5 -- The backend trait and the core on it ([DI-3.10.3.5.1 to DI-3.10.3.5.5](COMPLETED-CHECKLIST.md))

- [x] **DI-9.5.1** -- Registered buffers are given at construction only. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-951)

- [x] **DI-9.5.2** -- What a Windows commit and a target claim by default. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-952)

- [x] **DI-9.5.3** -- The backend trait's surface. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-953)

- [x] **DI-9.5.4** -- The in-memory backend. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-954)

- [x] **DI-9.5.5** -- The core on the backend trait. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-955)

- [x] **DI-9.5.6** -- The Windows backend's mapping. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-956)

### DI-9.6 -- The driver on the core ([DI-3.10.3.5.6.1 to DI-3.10.3.5.6.2.6](COMPLETED-CHECKLIST.md))

- [x] **DI-9.6.1** -- What the backend refuses for good, the core ends ([DI-D-54](DESIGN-NOTES.md#di-d-54)). -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-961)

- [x] **DI-9.6.2** -- `Dioring` requires `R: Send + 'static`. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-962)

- [x] **DI-9.6.3** -- A lineage's last-handle release reports a core refusal as not live, and ignores the default lineage's. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-963)

- [x] **DI-9.6.4** -- A declared geometry is now enforced on the driver's path. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-964)

- [x] **DI-9.6.5** -- The driver's structure. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-965)

- [x] **DI-9.6.6** -- The trait and type shapes. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-966)

- [x] **DI-9.6.7** -- The sabotage migration. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-967)

### DI-9.7 -- From DI-M7 on (each later step adds its own here as `DI-9.7.n`, with a tag)

- [x] **DI-9.7.1** -- `pump` on a pool-built instance is a runtime `Unsupported`, not a type error. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-971)

- [x] **DI-9.7.2** -- `completion_signal` hands out a full-access duplicate of dioring's own auto-reset event. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-972)

- [x] **DI-9.7.3** -- An allocation that fails is an error the caller receives, not a panic or an abort. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-973)

- [x] **DI-9.7.4** -- `NumaBuffer`'s buffer impls move from the ring crate to `win-numa-sys`. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-974)

- [x] **DI-9.7.5** -- The portable crate's public surface for other drivers. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-975)

- [x] **DI-9.7.6** -- The renames the split forced, and the clock's bounds. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-976)

- [x] **DI-9.7.7** -- Delivery modes. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-977)

- [x] **DI-9.7.8** -- Construction, the fault responder, and their tests. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-978)

- [x] **DI-9.7.9** -- The example and the sibling crates. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-979)

- [x] **DI-9.7.10** -- The buffer crate and the core's tests. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-9710)

### DI-9.8 -- Owed by the assistant

- [x] **DI-9.8.1** -- Each declared blind spot is covered, or records what would cover it. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-981)

- [ ] **DI-9.8.2** -- **MINE: read the portable job's first CI run.** The crate's tests have not run
  on a non-Windows host. The branch was pushed at the end of `DI-M9`, but [ci.yml](../../.github/workflows/ci.yml)
  runs on a pull request or a push to `main`, and the branch has neither, so this is blocked on a pull
  request, which is the engineer's call: it shows 60-odd commits, two of them breaking changes to published
  crates, to everyone watching the repository.

## DI-M7+ -- Delivery follow-ups

- [ ] **DI-7+.1** -- **A wait-only handle on the ring's own completion event, for a consumer who
  wants no callback.** From `DI-7.5`: [DI-D-55](DESIGN-NOTES.md#di-d-55)'s `Signalled` tees the
  event through a thread-pool wait, which a consumer who would rather wait on the ring's event
  directly pays for with a callback. A duplicate with `SYNCHRONIZE` access alone gives the consumer
  a handle that cannot set or reset the event, but not one that cannot be duplicated wider by a
  consumer that sets out to ([WS-D-7](../win-sync-sys/DESIGN-NOTES.md#ws-d-7)), and it does not
  remove the shared edge ([D-19](../windows-ioring-sys/DESIGN-NOTES.md#d-19)); since a pump drains
  before it waits, a consumer taking the edge costs only its own wakeup. The primitive,
  [WS-1+.5](../win-sync-sys/COMPLETED-CHECKLIST.md#ws-1+5), is built in `win-sync-sys`:
  `duplicate_handle_with_access`, which returns an `OwnedHandle` this crate can lend without
  `unsafe`. The shape here is not decided: a fourth `DeliveryMode`, an option on `Pumped`, or a second accessor
  beside `completion_signal`. Note that the ring crate's
  [D-21](../windows-ioring-sys/DESIGN-NOTES.md#d-21) allows one waiter per ring, which the shared
  edge bends for the consumer's waiter and the pump's.

## DI-M10 -- Target backings

- [ ] **DI-10.1** -- **The Win32 file object as a target backing** -- the separate concern DI-D-48
  names: what such a target warrants by buffering class, and whether dioring reads the class from
  the handle (`FileModeInformation`) rather than taking it on declaration.

- [ ] **DI-10.2** -- **A raw partition as a target backing** (the engineer, 2026-10-09: "ok"): one
  volume handle with regions in it, one commit per volume, failure sharing found from the volume's
  disks (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`), and optionally the volume lock
  (`FSCTL_LOCK_VOLUME`); implemented to the interface. How
  [D-44](../windows-ioring-sys/DESIGN-NOTES.md#d-44) applies is open under DI-D-48.

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

- [ ] **DI-12.4** -- **Pin `windows-ioring-sys` by version once the set of changes is about to
  release** ([DI-D-16](DESIGN-NOTES.md#di-d-16)). Gated on that release, not on `DI-12.3`. Add the
  `version` of the `windows-ioring-sys` release carrying everything dioring relies on (at least
  [D-80](../windows-ioring-sys/DESIGN-NOTES.md#d-80) and
  [D-81](../windows-ioring-sys/DESIGN-NOTES.md#d-81)), keeping the `path`. This must land before
  dioring's first release PR merges, because its publish job fails on a path-only dependency.
  `DI-3.2.2.1` added two more path-only dependencies, pinned the same way: `win-sync-sys` (the
  readiness signal's `Event`), at its first release, and `windows-threadpool-sys` (the readiness
  check's wait), at the release carrying its `M-T14.1` (`From<Event> for WaitableHandle`).
  win-time-sys' `WT-2.2.2` adds a fourth, `win-time-sys` (the failure timestamps' clock), at its
  first release. Since `DI-8.2` these dependencies are `win-durable-ioring`'s, with a fifth: its
  path-only dependency on `durable-ioring`, pinned at `durable-ioring`'s first release, which must
  publish first. `durable-ioring` itself has only `io-buffers`, already pinned.

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

## DI-M5+ -- The retention layer (parked behind DI-M12)

- [ ] **DI-5+.1** -- **A separate crate that retains written data until it is durable and can rewrite
  it after a failure**, built on DI-1.2's resolution protocol and the inventory, and resolving
  overlapping suspect writes newest-wins, and resolving at the granularity of individual buffers it
  can reissue (DI-1.2 Q6). The layer [DI-D-5](DESIGN-NOTES.md#di-d-5) moves out of
  this crate; its first natural consumers are log-less designs such as FAT. Needs its own name and
  component when it begins.

## DI-M6+ -- Deliberately coarse (parked behind DI-M12)

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

- [ ] **DI-6+.9** -- **Let the oracle check when a heal takes effect, by a per-lineage entry in the
  stream.** From `DI-9.1.4`, which accepted the oracle's weaker rule
  ([DI-D-42](DESIGN-NOTES.md#di-d-42)). The stream records when a seal is accepted, not when it
  starts, so the oracle cannot tell a `Durable` from a seal that started after a heal from one that
  started before it and finished after; it accepts both, and the core's own tests are what pin the
  timing. Working position, not a decision: an entry such as `HealEffective { failure, lineage }`,
  reported when a heal takes effect in a lineage, would let the oracle require it before a `Durable`
  passes that failure's epochs there. It adds a public entry kind that every consumer's `match` sees,
  and it changes [CONTRACT.md](CONTRACT.md) and the shared oracle together. Gated on a driver or a
  consumer whose own tests need the oracle to check this, since that is what would show what the
  entry should carry.

- [ ] **DI-6+.10** -- **Fault seams in `win-sync-sys` and `windows-threadpool-sys`, and a safe `Mutex`, so
  the Windows driver's remaining declared gaps can be tested.** From `DI-9.8.1`. Creating the readiness
  event, the pool and signalled deliveries' thread-pool waits and events, and duplicating the notification
  fail only when the process runs out of handles or memory, which a test cannot cause without breaking its
  own harness; the ring crate has a seam for such failures (its `kernel-seam`) and neither of these crates
  does. The notification's wait ending abandoned needs a `Mutex` to abandon, which is
  [WS-1+.3](../win-sync-sys/CHECKLIST.md). Working position, not a decision: each is a change to a crate that
  is not this one, and what the seam should look like is shaped by the first failure it has to produce.
  Gated on a consumer, or this crate's own review, wanting those paths tested.
