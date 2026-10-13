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

- [ ] **DI-9.2.1** -- **DECIDE: a held write whose own epoch is abandoned ends as `NeverIssued`
  naming that epoch.** A *held* write is a write pushed with a gate: the core has accepted it and
  holds it, and nothing has reached the backend, until the gate's epoch (another lineage's, or the
  same lineage's lower one) is durable ([CONTRACT.md](CONTRACT.md), "Gates"). Now: if the write's
  own epoch is abandoned while it waits, it can never be issued, since no write may join an
  abandoned epoch ([DI-D-35](DESIGN-NOTES.md#di-d-35)), so it ends at once as `NeverIssued` naming
  its own epoch rather than waiting on the gate. When its gate is also abandoned, the gate is the one
  named, as the contract says.

- [ ] **DI-9.2.2** -- **DECIDE: a held write is outstanding for its lineage's seals, as one in
  flight is, and no failure suspects it until it is released.**

- [ ] **DI-9.2.3** -- **DECIDE: `make_durable_through` returns `SealError::{UnknownLineage,
  GateCycle}` in place of `io::Error`.** A public API change for consumers.

- [ ] **DI-9.2.4** -- **RATIFY:** a gate naming an ended or retired lineage is refused as
  `UnknownLineage`; a cycle is listed as each held write's epoch followed by its gate; released
  operations are pushed at the consumer's next call on the instance, normally the `pop` that returns
  the `Durable` releasing them, because a registered-span push needs the registration and an owned
  push its buffer's bound, which a held owned operation carries as a function pointer; the oracle
  gains "a gated operation completes only after its gate is reported durable", "`NeverIssued` comes
  only for one", and "no failure suspects a held write".

### DI-9.3 -- The provider ([DI-D-44](DESIGN-NOTES.md#di-d-44), from `DI-3.2.6`; read it with its amendments by DI-D-46 and DI-D-50)

- [ ] **DI-9.3.1** -- **DECIDE: the provider is called only from the consumer's next call, with
  dioring's lock released.** Now: a seal's request is made by the consumer's next `pop`,
  `make_durable_through`, `resolve` or `end_lineage`, never by a completion callback on a pool
  thread. Two constraints force it: [DI-D-27](DESIGN-NOTES.md#di-d-27) lets a provider answer
  inside `make_durable`, and that answer takes the lock; and a provider panic can only propagate to
  a caller. Nothing is stranded, since a seal starts at its own call or at its last write's
  completion, which queues an entry the consumer pops.

- [ ] **DI-9.3.2** -- **DECIDE: every request is built before any is handed over.** Now: a panic
  drops the rest, and each completion dropped unanswered is a failure of its domain,
  `Cause::ProviderAbandoned`.

- [ ] **DI-9.3.3** -- **RATIFY:** a request is one per seal, naming only the served domains the
  seal's writes touched, in interned-id order, each with its files and each file's writes in push
  order; a seal naming no served write asks nothing; a write is covered when its file's flush
  succeeded, if the default answers for the file, and every served domain of its file answered
  success; a failure of domain D is observed with `Reach::Domains([D])`, which is
  [DI-D-27](DESIGN-NOTES.md#di-d-27)'s rule, so needs no reach of its own.

### DI-9.4 -- Faults, diagnostics, the clock ([DI-3.10.3.1 to DI-3.10.3.4](COMPLETED-CHECKLIST.md))

- [ ] **DI-9.4.1** -- **DECIDE: on Windows the default diagnostic sink is `NoDiagnostics`, until
  `DI-12.1`.** Now: `Setup.diagnostics` is an `Option<Arc<dyn DiagnosticSink>>`, `None` meaning the
  driver's default, so an inconsistency the core finds is reported nowhere unless the consumer
  supplies a sink. `StderrDiagnostics` exists for one who does, prefixing each line `dioring:`.

- [ ] **DI-9.4.2** -- **DECIDE: a sink is called with dioring's lock held.** It must return promptly
  and must never call back into the instance; one that does deadlocks, and one that is slow holds up
  every other path.

- [ ] **DI-9.4.3** -- **DECIDE: two `IoFault`s are equal when they name the same source and code and
  share the one report (`Arc` identity).** So a nullifier agrees with its write's completion only if
  it relays that very report, and a fault rebuilt from the same words is not equal.

- [ ] **DI-9.4.4** -- **RATIFY:** `SuspectWrite::file` and `IoTargetBusy::file` are renamed `target`,
  and `IoTargetBusy` displays "target"; the Windows path's fault for a short write carries no code
  and the message "the write completed short: n of N bytes"; a write's requested length rides in its
  sidecar and no longer in the core; the inconsistency for a code-less failed write is gone, the
  code having become optional; `DiagnosticKind` and `Diagnostic` are non-exhaustive, `Inconsistency`
  carrying the instance's number, `what` and the formatted detail; the clock trait is `StampClock`,
  its reading bounded by a public `Instant` trait (`std::time::Instant` shares the name), the time
  type an associated type of `Identities` -- with [DI-D-57](DESIGN-NOTES.md#di-d-57) amending how the
  driver names it.

### DI-9.5 -- The backend trait and the core on it ([DI-3.10.3.5.1 to DI-3.10.3.5.5](COMPLETED-CHECKLIST.md))

- [ ] **DI-9.5.1** -- **DECIDE: registered buffers are given at construction only.** Now: they come
  through `WindowsBackendSetup`, whose refusal hands everything back as `Setup`'s does; targets given
  at construction are registered and addressed by index, and those attached later are addressed by
  their `SharedHandle`.

- [ ] **DI-9.5.2** -- **DECIDE: what a Windows commit and a target claim by default.** Now: a commit
  is an unordered flush in the default mode, as before; a `Win32FileTarget` defaults to
  `BestEffort`, unaligned, with no capabilities, its class taken on declaration until `DI-3.10.4`;
  and the in-memory backend ignores `WriteCaching`.

- [ ] **DI-9.5.3** -- **RATIFY: the backend trait's surface.** `IoBackend`, in a public `backend`
  module with the in-memory backend in `memory`; `IoTarget`, `IoTargetDescription`, `WarrantyClass`,
  `Geometry`, `GeometryError` and `Capabilities` at the crate root; the target module is `io_target`
  because the root `.gitignore`'s `target` ignores any directory so named; `Geometry` holds three
  alignments (offset, length, buffer address), each a power of two its constructors enforce, with
  `admits` and `admits_buffer` the one statement of the rule; `Capabilities` names write-through
  only, its fields private; `IoTarget` requires `Send`, `describe` provided; `SubmissionToken` a
  public `u64`, `RegisteredRegion` three `u32`s, a completion's result a `u32` count or an `IoFault`,
  a commit transferring zero; refusal causes `SubmitNeeded`, `DrainNeeded`, `SubmitAndDrainNeeded`,
  `Busy`, `UnknownTarget`, `Unsupported`, `Invalid`, `Unusable(IoFault)`, `Unknown(IoFault)`,
  non-exhaustive, with `needs_submit`, `needs_drain`, `is_retryable`; `submit`, `pop` and
  `notification` report an `IoFault`; a submission for a detached target completes with a fault;
  registration is a backend's construction concern.

- [ ] **DI-9.5.4** -- **RATIFY: the in-memory backend.** Buffers are `Vec<u8>`; it executes at
  submit; it refuses a drain when queued and unpopped submissions fill the completion queue,
  checking the target, then the submission's shape, then capacity; its notification is a count with
  `wait_past`; its faults name the source `"memory"` with no code; queue sizes are `NonZeroU32`; a
  commit copies bytes to `durable_contents`.

- [ ] **DI-9.5.5** -- **RATIFY: the core on the trait.** A consumer operation takes the caller's
  push as a closure and is recorded only once the push accepts it; the core's own submissions come
  out in `CoreActions` and return through `refused`, re-emitted with the next actions or by `retry`;
  `CoreActions` is `submit` plus a `deliver` flag raised on the queue's empty-to-non-empty
  transition, `#[must_use]`; a seal returns its answer and actions as a pair; backend refusals reach
  the consumer as `PushRefusal::Backend(RefusalCause)`; a duplicate target hands back its
  description in `DuplicateTarget`; `new` takes a clock and an `Arc<dyn DiagnosticSink>` with no
  default; a completion with no record, a second completion, and a consumer operation handed back
  as refused are each an inconsistency ([DI-D-46](DESIGN-NOTES.md#di-d-46)); the module is `io_core`
  because `core` would shadow the `core` crate; `Event::into_entry` is the one event-to-entry
  conversion; a held operation spends its id when held and is emitted in the actions of the change
  that released it, kept and emitted again if refused; `write` and `read` take `WriteOptions` and
  `ReadOptions`; lineage calls take a plain `Lineage`, an ended or retired one refused as `Foreign`;
  the core holds no provider, `with_provider` takes the served domains and refuses a repeat as
  `DuplicateDomain`, requests come out as `ProviderRequest`s in `CoreActions::requests`, answers go
  back through `provided` with a public `DomainOutcome`; `import_reach` is the one function both
  paths share; `close` hands back the held operations, failures and lineages; `pushed` and
  `pushed_gated` take a `length` for every operation, a read's being its buffer and unchecked, and
  the short-write rule is exact (a write transferring more than its length is `WriteNotInFull`, as a
  shorter one is).

- [ ] **DI-9.5.6** -- **RATIFY: the Windows backend's mapping.** Each push is issued at once, the
  ring submitting a batch as it ends, and `submit` reissues what a failed submission left queued, so
  the trait says a push is issued no later than the next `submit`, and `submit` reports a
  `RefusalCause`; a full submission queue maps to `SubmitNeeded`, a full completion queue to
  `DrainNeeded`, a submission running to `Busy`, a corrupt ring to `Unusable`, any other ring
  condition to `Unknown` with the report, and the ring's `Unsupported` and `InvalidInput` to
  `Unsupported` and `Invalid`; a target's geometry is checked before the ring is asked; the
  notification is the ring's completion event, duplicated as an `OwnedHandle` and attached at
  construction; `fail_next_commit` joins `fail_next_flush` in the fault seam; the short-write rule
  lives in the backend as `transfer`; `register_files` takes handles rather than `TargetSetup`s; the
  backend and `Win32FileTarget` live in a public `windows_backend` module.

### DI-9.6 -- The driver on the core ([DI-3.10.3.5.6.1 to DI-3.10.3.5.6.2.6](COMPLETED-CHECKLIST.md))

- [ ] **DI-9.6.1** -- **DECIDE: what the backend refuses for good, the core ends
  ([DI-D-54](DESIGN-NOTES.md#di-d-54)).** Now: a gated operation is put to `IoBackend::check` before
  it is held; a submission of the core's own that the backend refuses for a non-retryable cause
  completes with a `REFUSED` fault instead of being re-emitted; an `Unknown` cause counts as
  permanent. The alternative, re-emitting every refusal, is the hang the row records.

- [ ] **DI-9.6.2** -- **DECIDE: `Dioring` requires `R: Send + 'static`.** Now: the registration
  lives behind the engine's lock, which pool threads take.

- [ ] **DI-9.6.3** -- **DECIDE: which refusals on the lineage paths are reported.** Now: a refusal
  from the core when `end_lineage` ends a lineage by its last handle is an inconsistency reported
  through the hub; a refusal of a handle's release is ignored, as the old relay ignored it.

- [ ] **DI-9.6.4** -- **DECIDE: a declared geometry is now enforced on the driver's path.** Now: the
  backend's `check` and `push` refuse an operation that does not fit a target's declared geometry,
  which the old driver did not.

- [ ] **DI-9.6.5** -- **RATIFY: the driver's structure.** A portable `Engine` is the one place
  `CoreActions` become backend pushes, with the drain every delivery shares; a drain runs until a
  pass pops nothing and does not call `retry`; pool delivery tells the driver through an `on_ready`
  closure called after the lock is released; `into_engine` stops delivery and returns the engine;
  one `Hub` holds the engine's slot, the readiness event and the count of unanswered provider
  completions, and is where a provider's answer and a handle's release arrive; the slot is an
  `Option` that ending empties; an answer is counted down before it reaches the engine, which is
  safe because ending empties the slot before it waits; the instance's own refusals of a duplicate
  key and of ending the default lineage stay in the driver, ahead of the core's; `pop` submits and
  pops but no longer retries refused submissions, the retry having been an equivalent mutant.

- [ ] **DI-9.6.6** -- **RATIFY: the trait and type shapes.** `Readiness` carries no bound, while
  `EntryDelivery` and `check_readiness` bind it to `Event`; the ring trait's `Target` is bounded by
  `IoTarget`; `add_target_with` goes with `FileOptions`, leaving one `add_target`; `AddTargetError`
  is generic, carrying the target; the test fake's target is `MemoryTarget`; `TargetSetup` carries
  the `Win32FileTarget` whole; two constructors, `new` for an `IoBufMut` buffer and
  `without_owned_reads` for one that may be only `IoBuf`, with a crate-private `enable_owned_reads`;
  the owned read's push is a function pointer, behind a `Via`; accessors are renamed
  `with_registered_buffer` and `with_registered_buffer_mut`, generic over the closure's return, so
  the trait is not object-safe, as it was not before ([DI-D-24](DESIGN-NOTES.md#di-d-24)); `Setup`
  has two lifetimes, `Setup<'env, 'pool, ..>`, because a callback environment is invariant behind
  `&mut`; `RefusalCause` implements `std::error::Error`; the ring crate gained a public `check_span`
  rather than dioring repeating the bounds.

- [ ] **DI-9.6.7** -- **RATIFY: the sabotage migration.** Thirty-three of the old path's sabotages
  were re-anchored where their rules now live and ten retired: nine sabotage the same predicate at
  the same site in the same way as an existing core or backend entry, and one was the relay's
  control, retired with the relay.

### DI-9.7 -- From DI-M7 on (each later step adds its own here as `DI-9.7.n`, with a tag)

- [ ] **DI-9.7.1** -- **DECIDE: `pump` on a pool-built instance is a runtime `Unsupported`, not a
  type error ([DI-D-55](DESIGN-NOTES.md#di-d-55)).** Now: a pump that waited would be a second waiter
  on the ring's event, which the ring crate's [D-21](../windows-ioring-sys/DESIGN-NOTES.md#d-21)
  forbids, so it refuses; the crate split kept it as it was. The alternatives, a type parameter on
  `Dioring` or a type per delivery mode, were rejected as churn through every bound.

- [ ] **DI-9.7.2** -- **DECIDE: `completion_signal` hands out a full-access duplicate of dioring's
  own auto-reset event.** Now: the consumer may wait on it, set it or reset it, and the worst that
  costs is its own wakeup, since a pump drains before it waits. `Event::try_clone_wait_only`
  ([WS-D-7](../win-sync-sys/DESIGN-NOTES.md#ws-d-7)) has since given a duplicate with no `set` or
  `reset`; whether to hand that instead is open. A wait-only duplicate of the ring's own event is
  `DI-7+.1`, below.

- [ ] **DI-9.7.3** -- **DECIDE: an unallocatable request is a panic where it was an abort.** Now:
  from `IB-2+.4`, which took the engineer's route (a), `windows-namespace-request-sys` takes the
  allocation error with `expect` at each site, and `SecurityDescriptor::clone` panics when the copy
  cannot be allocated ([that crate's notes](../windows-namespace-request-sys/DESIGN-NOTES.md#d-8)).

- [ ] **DI-9.7.4** -- **DECIDE: `NumaBuffer`'s buffer impls move from the ring crate to
  `win-numa-sys`**, which then implements a trait it does not define, ending its deliberately
  trait-free stance in that one respect ([IB-D-4](../io-buffers/DESIGN-NOTES.md#ib-d-4)).

- [ ] **DI-9.7.5** -- **DECIDE: the portable crate's public surface for other drivers.** Now: a
  public `driver` module re-exporting `Engine`, `EngineGuard`, `Slot`, `lock`, `present`, the pump
  types, `Home`, `Answer`, `DomainCall`, `InstanceId`, `first_repeat` and `inconsistent`; `Lineage`
  and `OpId` with public fields; `LineageHandle::detached` and `mint` public; `Waitable` a public
  one-method trait so a backend of another crate can implement it; and the Windows crate
  re-exporting the portable crate's API with `pub use durable_ioring::*`.

- [ ] **DI-9.7.6** -- **RATIFY: the renames the split forced
  ([DI-D-57](DESIGN-NOTES.md#di-d-57)).** The driver's clock is a `StampClock`, not a `win-time-sys`
  clock, with `InterruptStamps` the default and `FromClock` the adapter; the notification is
  `CompletionEvent`; `ErrorCode::of` is `error_code_of`; `DioringIds` has no default time type and
  the driver names it `WindowsIds`.

- [ ] **DI-9.7.7** -- **RATIFY: delivery modes.** `Setup`'s `environment` became `delivery:
  DeliveryMode` (`Pool`, `Pumped`, `Signalled`), touching every construction site and
  `CONTRACT.md`'s three worked examples, over a flag or a second constructor; `Dioring::pump` returns
  `io::Result<bool>`, whether entries became poppable; a pump in either pumped mode drains before it
  waits and returns at once if that delivered, so `waited` is `false` when no wait was made; a wait
  that failed or timed out drains too, and a failed one then reports; the readiness event is still
  set under pumped delivery; the signalled mode's event is auto-reset, since a manual-reset one would
  stay set and a pump's wait would return at once for ever; the blocking wait is
  `win-sync-sys::wait_for_single_object`, its first slice of the parked waits
  ([WS-D-6](../win-sync-sys/DESIGN-NOTES.md#ws-d-6)); the engine's slot helpers moved to
  `engine.rs`; the delivery enum is in `dioring/delivery.rs` and `SignalledDelivery` in
  `windows_backend/signalled.rs`; `MemoryNotification` keeps a per-handle record of the raisings its
  own waits have seen, beginning at none.

- [ ] **DI-9.7.8** -- **RATIFY: construction, the fault responder, and their tests.** The responder
  that fails submits is asked of the ring crate as a safe `sys::submit_faults` (its `M37.1`) rather
  than implemented behind `expect(unsafe_code)` or a macro, per
  [DI-D-4](DESIGN-NOTES.md#di-d-4); tests reach it through a `cfg(test)`
  `WindowsBackend::submit_faults`, `windows-ioring-sys` being a dev-dependency enabling
  `kernel-seam`; the pool-thread test holds the engine's lock across both pushes so the failing
  drain is guaranteed; the failure armed is `ERROR_IO_DEVICE`, surfacing as
  `RefusalCause::Unknown`; construction's two knobs travel in one crate-private `Construction`
  rather than a shortened constant under `cfg(test)` or a thread-local; a `RingHook` alias names the
  hook's type; the construction tests are two sibling modules; a stalled registration is tested with
  a 50ms bound against a 600ms stall, asserting on the bound the error names and on the ring running
  down, not on elapsed time; the ring crate's `M38.1` is the stalled submit
  ([D-87](../windows-ioring-sys/DESIGN-NOTES.md#d-87)).

- [ ] **DI-9.7.9** -- **RATIFY: the example and the sibling crates.** `blocking_push` is an example,
  not an API ([DI-D-56](DESIGN-NOTES.md#di-d-56)): it takes the buffer and context by value and the
  push as a closure, backs off in sleeps of at most a millisecond under the pool, excludes a
  registered-span push, runs its tests with the crate's, and has its sabotages in this crate's
  manifest. `win-sync-sys`: `duplicate_handle_with_access` over any handle with a typed `Access`
  mask; `WaitOnlyEvent` is a type, not an `Event` that fails at run time, and guards against misuse
  without being a boundary against a holder that duplicates it wider; alertability is an `Alertable`
  argument; the waits stay over `AsHandle` with no `Waitable` trait, which the item had proposed;
  wait for any and wait for all are two functions; a `Signalable` trait says what
  `SignalObjectAndWait` may signal; `queue_user_apc` takes a Rust closure
  ([WS-D-8](../win-sync-sys/DESIGN-NOTES.md#ws-d-8)).

- [ ] **DI-9.7.10** -- **RATIFY: the buffer crate and the core's tests.** The crate's name,
  `io-buffers`; `RegisteredSpan` moves into it with the contract; the ring crate re-exports the
  contract at the old paths ([D-89](../windows-ioring-sys/DESIGN-NOTES.md#d-89)); `AlignedBuf` is
  fallible to build, has no `Clone` and cannot grow ([IB-D-6](../io-buffers/DESIGN-NOTES.md#ib-d-6)).
  From `DI-8.1`: the core's tests share one portable clock and fault in `test_support`; a stamp in
  them is a plain `u64`; the test of the driver's default clock stamping with interrupt time stays in
  the driver's own tests; the readiness-check tests, being over a Windows event, sit in a module of
  their own.

### DI-9.8 -- Owed by the assistant

- [ ] **DI-9.8.1** -- **MINE: close or restate each declared blind spot.** The sabotage manifests
  declare as not covered: that a pool's callback environment is honoured, since no behaviour test
  can tell which pool ran a drain; the Windows notification's failing wait and an abandoned one,
  since reaching them needs `unsafe`; and the signalled and pool modes' construction failures. Each
  is to be covered, or recorded with what would cover it.

- [ ] **DI-9.8.2** -- **MINE: read the portable job's first CI run.** The crate's tests have not run
  on a non-Windows host. Blocked on a push, which is the engineer's call.

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
