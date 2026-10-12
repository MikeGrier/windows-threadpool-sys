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

## DI-M7 -- The driver's delivery, finished

The driver runs on the core over the Windows backend, drained on the thread pool or by the
consumer's own thread, which the pool may tell. What remains is the blocking push.

- [x] **DI-7.1** -- Failed submissions reached by tests, through the ring crate's safe submit-fault responder registered on the instance's ring. -> [completed 2026-10-11](COMPLETED-CHECKLIST.md#di-71)

- [x] **DI-7.2** -- Construction's registration paths reached by tests, through a hook that registers the ring crate's safe call-fault responder on the instance's ring as it is created. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#di-72)

- [x] **DI-7.3** -- Pumped delivery: the consumer's own thread waits on the backend's notification and drains through `pump`, over the Windows backend and the in-memory one. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#di-73)

- [ ] **DI-7.4** -- **A blocking push -- proposed, awaiting the engineer's ruling** (DI-D-52 Q4): a
  driver policy above refusal that submits, waits on the notification and retries; never from inside
  a callback, and pumping for itself under pumped delivery. Removed if the engineer declines it.

- [x] **DI-7.5** -- A third delivery mode, `DeliveryMode::Signalled`: the pool tells the consumer through an event it can wait on beside its own handles, and the consumer drains; and a pump that drains before it waits. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#di-75)

## DI-M8 -- The split: a portable crate and a Windows crate

Per [DI-D-50](DESIGN-NOTES.md#di-d-50): `durable-ioring` portable, and `win-durable-ioring` for
the driver, the Windows backend and its targets, registered for release.

- [ ] **DI-8.1** -- **The core's tests made portable, while still in one crate.** They read
  interrupt time and Windows faults today, so they move to a portable mock clock and fault, which
  the split needs: once `durable-ioring` has no Windows dependency, they could not build.

- [ ] **DI-8.2** -- **The split**: `win-durable-ioring` created and registered for release, holding
  the driver, the Windows backend and file target, `ErrorCode`'s reading from an `io::Error`, the
  readiness `Event`, the interrupt clock, the oracle's readiness check, and `DioringIds`' default
  time type; `durable-ioring` left with no Windows dependency. CONTRACT.md's worked examples drive
  the Windows driver, so they move in the same step, to `win-durable-ioring`'s documentation,
  compiled there as doctests, and CONTRACT.md links them; its "Vocabulary ahead of the code" note is
  removed.

- [ ] **DI-8.3** -- **`durable-ioring` added to CI's `portable` job**, so its non-Windows build,
  tests and lints are checked on Linux and macOS, as [DEVELOPMENT.md](../../DEVELOPMENT.md)
  describes for a crate with a portable core.

## DI-M9 -- Reviews: the engineer confirms or amends the assistant's choices (ungated; gates nothing)

The code binds to each choice already, because the steps could not be built without choosing; what
is open is the review. Each item reviews only what its decision row marks as the assistant's: where
the engineer had already decided part of a row, that part is not reopened. A rejected point is
reworked as an item of the milestone current when it is rejected, with its decision row amended.
These reviews gate no other work, because the engineer is not to be the blocking factor (the
engineer, 2026-10-10: "I cannot be the blocking factor"); they were placed ahead of the scripted
backend, now `DI-11.1`, so that a rework could not change the trait it implements, and a rework that
does is now that item's to absorb.

- [ ] **DI-9.1** -- **[DI-D-42](DESIGN-NOTES.md#di-d-42)**, from `DI-3.2.5.1`: `Tag` borrowing the
  handle, never the instance; construction returning the default lineage's handle beside the
  instance; `Result<_, UnknownLineage>` for a name the instance cannot answer for; the end and
  retire refusals, handing the handle back; an ended lineage not live from the moment it ends;
  `LineageEnded.abandoned_through`; ending a lineage releasing a heal that waits on it; the oracle's
  rules for an ended lineage; and a handle's release running on whichever thread drops the last
  copy.

- [ ] **DI-9.2** -- **[DI-D-43](DESIGN-NOTES.md#di-d-43)**'s assistant-chosen part, from
  `DI-3.2.5.2` -- the refusal of a seal that closes a cycle is the engineer's own: a gate naming an
  ended or retired lineage refused as `UnknownLineage`; a held write whose own epoch is abandoned
  ending as `NeverIssued` naming that epoch; a held write outstanding for its lineage's seals and
  suspected by no failure until released; `SealError` in place of `io::Error`; a cycle listed as
  each held write's epoch followed by its gate; and released operations pushed at the consumer's
  next call.

- [ ] **DI-9.3** -- **[DI-D-44](DESIGN-NOTES.md#di-d-44)**, from `DI-3.2.6`: the provider called
  from the consumer's next call with dioring's lock released; every request built before any is
  made; what a request names, and in what order; when a write is covered; a domain's failure
  observed with `Reach::Domains` of that one domain; and answers no request asked for recorded as
  inconsistencies. The restatement of what a request names, which this waited for, has landed, so
  the shapes are read as they stand.

- [ ] **DI-9.4** -- **The assistant's choices in `DI-3.10.3.1` to `DI-3.10.3.4`**: From
  `DI-3.10.3.1`: `SuspectWrite::file` and `IoTargetBusy::file` renamed `target`, and `IoTargetBusy`
  displaying "target". From `DI-3.10.3.2`: two `IoFault`s are equal when they name the same source
  and code and share the one report (`Arc` identity), so a nullifier agrees with its write's
  completion only if it relays that very report; the Windows path's fault for a short write carries
  no code and the message "the write completed short: n of N bytes"; a write's requested length
  rides in its sidecar for that check and no longer in the core; the inconsistency for a code-less
  failed write is gone, P8 having made the code optional; and the oracle's no-short-write rule is
  queued to `DI-3.10.3.5`. From `DI-3.10.3.3`: `DiagnosticKind` and `Diagnostic` are non-exhaustive
  enums, `Inconsistency` carrying the instance's number, `what` and the formatted detail; a sink is
  called with dioring's lock held, so it must return promptly and never call back into the instance;
  `Setup.diagnostics` is an `Option<Arc<dyn DiagnosticSink>>`, `None` meaning the driver's default;
  `StderrDiagnostics` prefixes each line `dioring:`; and the Windows driver's default is
  `NoDiagnostics` until `DI-3.6`. From `DI-3.10.3.4`: the clock trait is `StampClock`, its reading
  type bounded by a public `Instant` trait (a name `std::time::Instant` shares); the time type is an
  associated type of `Identities` and a second, defaulted parameter of `DioringIds`; and the core's
  types take it as a parameter, the Windows code naming it through a `WinInstant` alias.

- [ ] **DI-9.5** -- **The assistant's choices in `DI-3.10.3.5.1` to `DI-3.10.3.5.5`**: From
  `DI-3.10.3.5.1`: the backend trait is `IoBackend`; its types live in a public `backend` module and
  the in-memory backend in a public `memory` module, while `IoTarget`, `IoTargetDescription`,
  `WarrantyClass`, `Geometry`, `GeometryError` and `Capabilities` are at the crate root; the target
  module is `io_target`, since the root `.gitignore`'s `target` ignores any directory so named; a
  `Geometry` holds three alignments -- offset, length, buffer address -- each a power of two its
  constructors enforce, with `admits` and `admits_buffer` the one statement of the rule;
  `Capabilities` names write-through only, its fields private; `IoTarget` requires `Send`, and
  `describe` is a provided method; `SubmissionToken` is a public `u64`, a `RegisteredRegion` three
  `u32`s, a completion's result a `u32` count or an `IoFault`, and a commit transfers zero; the
  refusal causes are `SubmitNeeded`, `DrainNeeded`, `SubmitAndDrainNeeded`, `Busy`, `UnknownTarget`,
  `Unsupported`, `Invalid`, `Unusable(IoFault)` and `Unknown(IoFault)`, non-exhaustive, with
  `needs_submit`, `needs_drain` and `is_retryable`, an unknown cause not retryable; `submit`, `pop`
  and `notification` report an `IoFault`; a submission for a target detached before it ran completes
  with a fault; registration is a backend's construction concern, `registered_buffers` counting it.
  The in-memory backend's buffers are `Vec<u8>`; it executes at submit; it refuses a drain when
  queued and unpopped submissions fill the completion queue, checking the target, then the
  submission's shape, then capacity; its notification is a count with `wait_past`; its faults name
  the source `"memory"` with no code; its queue sizes are `NonZeroU32`; a commit copies the bytes to
  `durable_contents`; and it ignores `WriteCaching`. From `DI-3.10.3.5.2`: a consumer operation
  takes the caller's push as a closure and is recorded only once the push accepts it, which is how a
  refused one is undone with nothing to roll back, while the core's own submissions -- commits --
  come out in `CoreActions` and go back through `refused`, re-emitted with the next actions or by
  `retry`; `CoreActions` is `submit` and a `deliver` flag raised on the queue's transition from
  empty, and is `#[must_use]`; a seal returns its answer and its actions as a pair; the core's write
  takes an `Epoch`, not a `Tag`, handles staying the driver's, and a `WriteCaching` rather than
  `WriteOptions` until gates arrive in `DI-3.10.3.5.3`; geometry and registration are the backend's
  to check, their refusals reaching the consumer as a new `PushRefusal::Backend(RefusalCause)`,
  beside `Ring` until `DI-3.10.3.5.6`; a duplicate target hands back its description in a
  `DuplicateTarget`, and removing one returns it; `new` takes a clock and an `Arc<dyn
  DiagnosticSink>`, with no default; a completion with no record, a second completion, and a
  consumer operation handed back as refused are each an inconsistency (DI-D-46); until
  `DI-3.10.3.5.3`, every target routes to the built-in default, `IoTargetBusy::held_for_gate` is
  zero, and a gate release, a provider request or a never-issued operation reaching the core is an
  inconsistency; the module is `io_core`, as `core` would shadow the `core` crate; and the
  event-to-entry conversion is one method, `Event::into_entry`, which the relay now uses too. From
  `DI-3.10.3.5.3`: a held operation spends its id when held, and once released is emitted in the
  actions of the change that released it, recorded as issued at emission, and kept and emitted again
  if refused, as a commit is; `write` and `read` take `WriteOptions` and `ReadOptions`, replacing
  the plain `WriteCaching`; lineage calls take a plain `Lineage`, an ended or retired one refused as
  `Foreign`, whose doc on both refusal enums now says so, rather than a new variant; the core holds
  no provider -- `with_provider` takes the served domains and refuses a repeat as `DuplicateDomain`,
  a seal's request comes out as a `ProviderRequest` of `ProviderPart`s in `CoreActions::requests`,
  which makes `CoreActions` generic over the identities, and each answer goes back through
  `provided` with a now-public `DomainOutcome`, whose one conversion to a cause the relay uses too;
  an answer naming another core's lineage or an unserved domain is an inconsistency;
  `IoTargetWrites` loses its `file` on both paths (DI-D-50 P6), the driver's test no longer checking
  a lent handle; an import's reach is one function, `import_reach`, both paths share; `close` hands
  back the held operations, the failures and the lineages, leaving what is in flight to the backend;
  and `io_core` is a facade with `lineages`, `failures` and `provider` parts. From `DI-3.10.3.5.4`:
  `pushed` and `pushed_gated` take a `length` after the kind, for every operation -- a read's is its
  buffer and is not checked, since a read may complete short; the rule is exact, so a write
  transferring more than its length is `WriteNotInFull` as a shorter one is; and the core's test rig
  reports each accepted push, each seal answered `Submitted` and each heal, and observes every entry
  it pops. From `DI-3.10.3.5.5`: the Windows backend issues each push at once, its ring submitting a
  batch as it ends (the ring crate's D-5), and `submit` reissues what a failed submission left
  queued -- so the backend trait now says a push is issued no later than the next `submit`, and
  `submit` reports a `RefusalCause` where it reported an `IoFault`; a full submission queue maps to
  `SubmitNeeded`, a full completion queue to `DrainNeeded`, a submission running to `Busy`, a
  corrupt ring to `Unusable`, any other ring condition to `Unknown` with the report, and the ring's
  own `Unsupported` and `InvalidInput` rejections to `Unsupported` and `Invalid`; a target's
  geometry is checked before the ring is asked -- offset, length, and an owned buffer's address, a
  registered region's address being the registration's; targets given at construction are registered
  and addressed by index, those attached later by their `SharedHandle`, and registered buffers are
  given at construction only, through a `WindowsBackendSetup` whose refusal hands everything back as
  today's `Setup` does; the notification is the ring's completion event, duplicated as an
  `OwnedHandle` and attached at construction; a `Win32FileTarget` defaults to `BestEffort`,
  unaligned and no capabilities, its class taken on declaration until `DI-3.10.4`; a commit is an
  unordered flush in the default mode, as today; the fault seam gains `fail_next_commit` on the
  backend, queued with `fail_next_flush` under `DI-3.11`; the short-write rule moved into the
  backend as `transfer`, which the relay's `op_outcome` now calls; `register_files` takes handles
  rather than `TargetSetup`s; and the backend lives in a public `windows_backend` module,
  `Win32FileTarget` with it rather than at the crate root.

- [ ] **DI-9.6** -- **The assistant's choices in `DI-3.10.3.5.6.1` to `DI-3.10.3.5.6.2.6`**, the
  driver moved onto the core: From `DI-3.10.3.5.6.1`: the driver's core and backend sit together in
  a portable, crate-private `Engine`, the one place `CoreActions` become backend pushes -- refusals
  back to the core, provider requests kept for the driver -- and the drain every delivery shares, so
  pumped delivery reuses it; a drain runs until a pass pops nothing and does not call `retry`, since
  the core re-emits what was refused with every completion's actions; pool delivery tells the driver
  through an `on_ready` closure, called after the engine's lock is released, rather than setting an
  `Event` itself, so the driver chooses the signal; the backend lends its ring through a
  crate-private `WindowsBackend::wake`; a failed construction hands the engine back boxed;
  `into_engine` stops delivery and returns it; the engine, the pool delivery and `wake` carry
  `expect(dead_code)` outside tests until the driver moves onto them, so the build fails when they
  are used and the expectations go; and that the callback environment is honoured is a declared
  blind spot, no behaviour test being able to tell which pool ran a drain. From `DI-3.10.3.5.6.2.1`:
  [DI-D-54](DESIGN-NOTES.md#di-d-54) itself -- `check` asked only for an operation the core will
  hold, and after the core's own checks; `Unknown` treated as permanent; and the ended submission
  completed as the backend would have with a fault rather than through a new outcome; `check` a
  required trait method, with no default, so a backend cannot forget it; the core's `write` and
  `read` take the backend, `&mut impl IoBackend`, where they took a push closure; `refused` returns
  actions, and the engine works through them first in, first out; the fault's source `REFUSED`
  (`"refused"`), its error the `RefusalCause` boxed so it is recovered by downcasting, its kind
  `NotFound`, `Unsupported` or `InvalidInput` where the cause has one; `RefusalCause` implementing
  `std::error::Error`, its source the backend's report; the ring crate gaining a public `check_span`
  (its M34.1) rather than dioring repeating the bounds; and the Windows push still leaving spans to
  the ring, so only `check` asks first. From `DI-3.10.3.5.6.2.2`: `Readiness` carries no bound, so
  the trait says nothing about how a signal is waited on; `EntryDelivery` and the oracle's
  `check_readiness`, which both wait through a thread-pool wait, bind `Readiness = Event` rather
  than a waitable trait of their own, so a driver with another signal needs its own front end and
  check -- the pumped delivery of `DI-3.10.3.5.7` is where that is met. From `DI-3.10.3.5.6.2.3`:
  the ring trait's `Target` is bounded by `IoTarget`, so a consumer can always read what a target
  states; `add_target_with` goes with `FileOptions`, leaving one `add_target` with no defaulted
  form; `AddTargetError` keeps its name and becomes generic, carrying the target, rather than the
  contract reusing the backend module's `DuplicateTarget`; the test fake's target is `MemoryTarget`;
  `TargetSetup` carries the `Win32FileTarget` whole; and until the swap, today's driver reads only a
  target's file and flush domains, so a declared geometry is not enforced on its path -- the
  backend's `check` and `push` enforce it once the swap (`DI-3.10.3.5.6.2.6`) lands. From
  `DI-3.10.3.5.6.2.4`: two constructors where there was one -- `new` for a buffer type that is
  `IoBufMut`, reading into owned buffers, and `without_owned_reads` for one that may be only `IoBuf`
  -- rather than one constructor and a separate switch, so a direct user of the backend cannot
  forget to enable reads; a crate-private `enable_owned_reads`, for the driver to call where its
  read's `IoBufMut` bound holds; the owned read's push a function pointer, as today's driver carries
  one, with the registered and shared forms of a target behind a `Via`; and an owned read without
  that push refused as `Unsupported` at `check` and at `push` alike. From `DI-3.10.3.5.6.2.5`: the
  accessors renamed `with_registered_buffer` and `with_registered_buffer_mut`, the names
  `DeliveryHandle` already used, and generic over the closure's return, so the trait is not
  object-safe -- it was not before either, being "generic only" (DI-D-24); kept on `&mut self`, as
  the borrow forms were; and the Windows backend's own borrow-returning accessors left as they are,
  since whoever owns a backend holds it `&mut`. From `DI-3.10.3.5.6.2.6`: what the driver's paths
  off the consumer's thread share is one `Hub` -- the engine's slot, the readiness event, and the
  count of unanswered provider completions -- and the hub is where a provider's answer and a lineage
  handle's release arrive; the engine's slot an `Option` that ending empties, so a late answer or
  release finds nothing to act on rather than an ended instance; an answer counted down before it
  reaches the engine, which is safe because ending empties the slot before it waits; `Setup` with
  two lifetimes, `Setup<'env, 'pool, ..>`, because a callback environment is invariant behind `&mut`
  and one lifetime would tie the caller's borrow to the pool's; `R: Send + 'static` now required on
  `Dioring`, since the registration lives behind the engine's lock, which pool threads take; `pop`
  submits and pops but no longer retries the core's refused submissions -- a retryable cause always
  leaves completions to come, whose actions the pool drain re-emits them with, so the retry was an
  equivalent mutant -- and still makes the provider's requests, as a drain's request waits for a
  consumer call; geometry now enforced on the driver's path, by the backend; the instance's own
  refusals of a duplicate key and of the default lineage's end kept in the driver, ahead of the
  core's, so their reasons stay exact; a refusal from the core when `end_lineage` ends a lineage by
  its last handle reported as an inconsistency through the hub, as the old core reported a second
  end, rather than ignored -- the swap first ignored it, and the full sweep caught that as a
  survivor; while a refusal of a handle's release is still ignored, as the old relay ignored it; and
  of the old path's sabotages, 33 re-anchored where their rules now live and 10 retired -- nine that
  sabotage the same predicate, at the same site and in the same way, as an existing core or backend
  entry, now that the driver's copy is gone, and the relay's control with the relay.

- [ ] **DI-9.7** -- **The assistant's choices from DI-M7 on**, each step adding its own here as it
  lands. From `DI-7.1`: the responder that fails submits is asked of the ring crate, as a safe
  `sys::submit_faults` (its `M37.1`), rather than implemented here behind an `expect(unsafe_code)`
  or reached through a macro that hides the `unsafe` -- [DI-D-4](DESIGN-NOTES.md#di-d-4) says to add
  the safe wrapper to the lower crate, and either alternative is the lint relaxed by another name.
  Also from `DI-7.1`: the tests that need the ring reach it through a `cfg(test)`
  `WindowsBackend::submit_faults`, which registers the responder and returns only its control, so
  the ring's private sidecar type stays out of the signature, once per backend; `windows-ioring-sys`
  is a dev-dependency beside the normal one, enabling `kernel-seam` for test builds; the pool-thread
  test holds the engine's lock across both pushes, so the drain that fails is a guaranteed one
  rather than a race; and the failure armed is `ERROR_IO_DEVICE`, which no ring condition names, so
  it surfaces as a `RefusalCause::Unknown` carrying its code, that `pop` reports as an error
  downcasting to the `RefusalCause`. From `DI-7.2`: construction's two knobs, the test-only hook and
  the registration bound, travel together in one crate-private `Construction`, handed from
  `Dioring::with_construction` to `WindowsBackend::construct`, rather than the constant being made
  shorter under `cfg(test)`, which would change every test's wait and could fail a slow machine's
  registration, or the hook being a thread-local or a global; `without_owned_reads` and
  `Dioring::with_clock` keep their signatures and pass the production default; `Sidecar`, the
  backend's per-operation record, becomes crate-visible, and the hook has a `RingHook` alias, so the
  hook can name the ring's type; the construction tests are two sibling modules, the backend's
  because its `tests.rs` is already over 32 KiB, and `TempFile::is_held` is made crate-visible for
  them; a stalled registration is tested with a 50ms bound against a 600ms stall, which asserts on
  the bound the error names and on the ring running down after the stall, rather than on elapsed
  time; and the ring crate's `M38.1`, which this step asked for, is the stalled submit rather than
  withheld completions, for the reason its [D-87](../windows-ioring-sys/DESIGN-NOTES.md#d-87) gives.
  From `DI-7.3`: pumped delivery needed a blocking wait on a handle, which this crate cannot write,
  so `win-sync-sys` gained `wait_for_single_object` as the first slice of its parked wait functions
  ([WS-D-6](../win-sync-sys/DESIGN-NOTES.md#ws-d-6)), rather than the wait being written in the ring
  crate; `Waitable` is a public one-method trait, so a backend of another crate can implement it;
  `Setup`'s `environment` became `delivery: DeliveryMode`, which touched every construction site and
  `CONTRACT.md`'s three worked examples, rather than a flag or a second constructor; `Dioring::pump`
  returns `io::Result<bool>`, whether entries became poppable, rather than a struct of what woke it
  and what it drained; a pump whose wait failed drains anyway and then reports the failure, and one
  whose wait timed out drains too; `pump` on an instance built with a pool is a runtime
  `Unsupported`, not a type error, which the crate split revisits; the readiness event is still set
  under pumped delivery; the engine's slot helpers moved from `pool.rs` to `engine.rs`, being
  generic already; the delivery enum is in `dioring/delivery.rs`, so `dioring.rs`, already over
  32 KiB, grows by little; `MemoryNotification` gained a per-handle record of the raisings its own
  waits have seen, beginning at none, so a raising before the first wait is seen; the Windows
  notification's failing wait and an abandoned one are declared blind spots in the sabotage
  manifest, since reaching them needs `unsafe`; and waiting beside other handles was raised as
  `DI-7.5` rather than built, because handing out the event touches
  [DI-D-10](DESIGN-NOTES.md#di-d-10). From `DI-7.5`: the mode is named `Signalled` and takes the
  callback environment; the signal is auto-reset, since a manual-reset one would stay set and a
  pump's wait on it would return at once for ever; `completion_signal` hands out a full-access
  duplicate of one event, the event being the consumer's own and not the ring's; a pump in either
  pumped mode drains before it waits and returns at once if that delivered, which changed `Pumped`'s
  behaviour and tests from `DI-7.3`, its `waited` now `false` when no wait was made; a pump on a
  signalled instance waits on the signal, so a consumer who waits on it too and then pumps with a
  limit works, because of the drain first; `SignalledDelivery` is in `windows_backend/signalled.rs`
  beside the pool's, and `PumpedDelivery` gained `with_notification`; the wait-only duplicate of the
  ring's own event is not built and stays available, needing a duplicate-with-access in
  `win-sync-sys`; and the signalled mode's construction failures and its callback environment are
  declared in the sabotage manifest as not covered, as the pool's are.

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
  first release.

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
