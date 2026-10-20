# Parked: durable-ioring

Work that is not in scope now: gated on a deliverable (`M{n}+`), on the horizon (`M-inf`), or deferred by a
ruling. It lives here and not in [CHECKLIST.md](CHECKLIST.md), which holds only what is in scope, under the
workspace's rule "Parked work lives in `PARKED.md`, never in `CHECKLIST.md`" in
[copilot-instructions.md](../../.github/copilot-instructions.md). Every item keeps its ID. A section parked
behind a milestone is pointed to from that milestone in [CHECKLIST.md](CHECKLIST.md) by an `UNBLOCKS PARKED
WORK` callout.

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

- [ ] **DI-6+.11** -- **Revisit `CREATE_ALWAYS` and `TRUNCATE_EXISTING`, which v1 leaves out** (the engineer,
  2026-10-16, "just to simplify the matrix", to be revisited at a later date). Parked, neither added nor
  dropped. What an answer needs is in [DI-D-64](DESIGN-NOTES.md#di-d-64): `TRUNCATE_EXISTING` was
  accepted only with the literal `GENERIC_WRITE`, and `CREATE_ALWAYS` emptied a file under every access
  mask including a read-only one, so offering either means deciding what a handle typed not to write may
  be opened to do. It is answerable once the three dispositions of [DI-13.7.3](COMPLETED-CHECKLIST.md#di-137) are in use.
  **Added 2026-10-18 by [DI-14.4.1](COMPLETED-CHECKLIST.md#di-1441):** offering either brings in a
  documented condition on the arguments that is then a check under
  [the documented-contract rule](../../DESIGN-NOTES.md#refusals-rest-on-the-documented-contract):
  `FILE_FLAG_OPEN_REPARSE_POINT` "cannot be used with the `CREATE_ALWAYS` flag", and `TRUNCATE_EXISTING`
  needs "the `GENERIC_WRITE` bit" in the access, which this crate's data access does not ask for
  ([DI-D-64](DESIGN-NOTES.md#di-d-64), "What the reading pass of DI-14.4.1 found").

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

## DI-M-inf -- The horizon (parked, no gating deliverable)

- [ ] **DI-15.3** -- **Opening relative to a directory handle.** **Deferred by the engineer, 2026-10-19: "let's
  defer any relative opening work at this time."** Parked from `DI-M15` with no gate; it is picked up when the
  engineer brings it back. The item as written: confirm that no documented Win32 call opens a name relative to a
  directory handle (the only route the assistant knows is `NtCreateFile` with a root directory, which the
  repository's rule forbids), and if none does, record the substitute: list the directory through its handle, take
  the child's identity, and open it by identity ([DI-15.2](COMPLETED-CHECKLIST.md#di-152)). What was found before
  the deferral, so it is not redone:
  - `GetFileInformationByHandleEx` takes the handle, the class, the buffer and its size, and nothing else, so its
    directory classes (`FileIdExtdDirectoryInfo` and its restart form) return every entry and cannot be narrowed to
    a name. `windows-file-enumeration-sys` filters after the read, by its own compiled patterns.
  - `FindFirstFileExW` takes a name with wildcards, but by path, which walks the directory's name again and so
    reopens the race a relative open exists to avoid. `NtQueryDirectoryFile`, which filters on a handle, is an
    `Nt*` call.
  - The substitute differs from `openat` in three ways a decision would have to state: it pins the object found
    when the directory was listed, not the one the name reaches at the open; it cannot create a file, since
    `OpenFileById` has no disposition; and finding one name costs reading entries until it appears, the whole
    directory at worst.
  - Not yet checked against the documentation: `CreateFile2`'s extended parameters, and the documented `FSCTL_*`
    controls. A directory opens by identity without the backup flag ([DI-D-70](DESIGN-NOTES.md#di-d-70)), which
    the engineer ruled allowed ([DI-D-72](DESIGN-NOTES.md#di-d-72)), so the substitute reaches directories too.
