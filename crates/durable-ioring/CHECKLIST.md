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

- [x] **DI-3.2** -- dioring over `windows-ioring-sys`, implementing the contract in steps `DI-3.2.1` to `DI-3.2.8`, from construction through ending. -> [completed 2026-10-08](COMPLETED-CHECKLIST.md#di-32)

- [ ] **DI-3.9** -- **The engineer confirms or amends the shapes the assistant chose while
  implementing `DI-3.2`**, one decision per step below; checked when the last of them is. The code
  binds to each already, because the steps could not be built without choosing; what is open is
  the review. A rejected point is reworked in its step, with its decision row amended. Placed
  before `DI-3.3`, because a reworked point can change the trait the fault-injecting
  implementation implements. Each step reviews only what its decision row marks as the
  assistant's: where the engineer had already decided part of a row, that part is not reopened.

- [x] **DI-3.9.1** -- The engineer confirmed DI-D-34 with three amendments: unmatched import domains reported, a heal released only by a seal reaching the re-issue, and inconsistencies panicked on where found. -> [completed 2026-10-08](COMPLETED-CHECKLIST.md#di-391)

- [x] **DI-3.9.2** -- The engineer confirmed DI-D-39 with amendments: every report cites its provenance, a covered marking means covered successfully and cites who answered, and a nullifier's code is a plain `ErrorCode`. -> [completed 2026-10-08](COMPLETED-CHECKLIST.md#di-392)

- [ ] **DI-3.9.3** -- **[DI-D-42](DESIGN-NOTES.md#di-d-42)**, from `DI-3.2.5.1`: `Tag` borrowing
  the handle, never the instance; construction returning the default lineage's handle beside the
  instance; `Result<_, UnknownLineage>` for a name the instance cannot answer for; the end and
  retire refusals, handing the handle back; an ended lineage not live from the moment it ends;
  `LineageEnded.abandoned_through`; ending a lineage releasing a heal that waits on it; the
  oracle's rules for an ended lineage; and a handle's release running on whichever thread drops
  the last copy.

- [ ] **DI-3.9.4** -- **[DI-D-43](DESIGN-NOTES.md#di-d-43)**'s assistant-chosen part, from
  `DI-3.2.5.2` -- the refusal of a seal that closes a cycle is the engineer's own: a gate naming an
  ended or retired lineage refused as `UnknownLineage`; a held write whose own epoch is abandoned
  ending as `NeverIssued` naming that epoch; a held write outstanding for its lineage's seals and
  suspected by no failure until released; `SealError` in place of `io::Error`; a cycle listed as
  each held write's epoch followed by its gate; and released operations pushed at the consumer's
  next call.

- [ ] **DI-3.10** -- **The contract over targets, and dioring as a portable algorithm**
  ([DI-D-48](DESIGN-NOTES.md#di-d-48), [DI-D-49](DESIGN-NOTES.md#di-d-49)): the unit of I/O and
  durability becomes a target, defined only by its obligations, files leave the contract's
  vocabulary, and the core becomes a portable public state machine with targets and a backend
  beneath it and a threaded driver above it. Checked when the last step below is. Placed before
  `DI-3.9.5` and `DI-3.3`, which are written in the file vocabulary this restates; `DI-3.9.3` and
  `DI-3.9.4` do not use it, and stay ahead.

- [x] **DI-3.10.1** -- The target design: `IoTarget` and its obligations, `DurableIoCore` and its backend, `FailureCause` and `IoFault`, and two crates (DI-D-50). -> [completed 2026-10-09](COMPLETED-CHECKLIST.md#di-3101)

- [x] **DI-3.10.2** -- CONTRACT.md restated over targets: the obligations, warranty classes by backing, and markers on the decision rows it changes. -> [completed 2026-10-09](COMPLETED-CHECKLIST.md#di-3102)

- [ ] **DI-3.10.3** -- **The portable core, the Windows backend and the driver, restated in place**
  as `DI-3.10.1` and `DI-3.10.2` settle them: the durability core as a public state machine naming
  no platform type; targets and a backend beneath it, with `windows-ioring-sys` inside the Windows
  backend; the threaded driver above it implementing `DurableRing`; the trait, types, oracle and
  fake restated over targets. Split as [DI-D-50](DESIGN-NOTES.md#di-d-50) names: `durable-ioring`,
  portable, and `win-durable-ioring` for the driver, the Windows backend and its targets,
  registered for release. Today's tests and sabotages are the proof the Windows backend passes,
  and today's file path stands in as its target until `DI-3.10.4`. The portable crate joins CI's
  `portable` job. CONTRACT.md's worked examples are restated in the new API, and its "Vocabulary
  ahead of the code" note is removed. Diagnostics per [DI-D-51](DESIGN-NOTES.md#di-d-51): the
  `DiagnosticSink` trait, `NoDiagnostics`, `StderrDiagnostics` and `FanOutDiagnostics`, the
  inconsistency report sent through the sink, and the `trace` feature removed. Done in the steps
  below, each compiling and passing the tests and the sabotage sweep on its own.

- [x] **DI-3.10.3.1** -- DI-D-50's renames in place: `IoTargetKey`, `IoTargetBusy`, `IoTargetWrites`, `AddTargetError`, `RemoveTargetError`, `add_target`, `remove_target`, `TargetSetup`, `FailureCause::Commit`. -> [completed 2026-10-09](COMPLETED-CHECKLIST.md#di-31031)

- [x] **DI-3.10.3.2** -- `IoFault` on `Outcome::Failed`, `FailureCause::Commit` and `Nullified`; `ErrorCode`'s meaning the source's; no short writes, the Windows path turning a short count into a fault. -> [completed 2026-10-09](COMPLETED-CHECKLIST.md#di-31032)

- [x] **DI-3.10.3.3** -- The diagnostic sink: `DiagnosticSink` with `NoDiagnostics`, `StderrDiagnostics` and `FanOutDiagnostics`, given in `Setup`, carrying the inconsistency report; the `trace` feature removed. -> [completed 2026-10-09](COMPLETED-CHECKLIST.md#di-31033)

- [x] **DI-3.10.3.4** -- The core made platform-free: `StampClock` and an `Instant` on `Identities`, `DioringIds<E, I>`, and no Windows type in `dioring/durability*` outside its tests. -> [completed 2026-10-09](COMPLETED-CHECKLIST.md#di-31034)

- [ ] **DI-3.10.3.5** -- **Targets, the backend, and the public core**, per
  [DI-D-50](DESIGN-NOTES.md#di-d-50) P10-P11 and [DI-D-52](DESIGN-NOTES.md#di-d-52): built as a
  new path beside today's driver (`DI-3.10.3.5.1` to `DI-3.10.3.5.5`), which the driver then moves
  onto, deleting the old path (`DI-3.10.3.5.6`) -- the merge-or-delete decision for the
  duplication, taken when the new path is proven. Done in the steps below.

- [x] **DI-3.10.3.5.1** -- The target and backend traits: `IoTarget` and its description, `IoBackend` with the core's submission and completion shapes and the typed `RefusalCause`, and a minimal in-memory backend and target. -> [completed 2026-10-10](COMPLETED-CHECKLIST.md#di-310351)

- [x] **DI-3.10.3.5.2** -- `DurableIoCore`, public: targets, writes and reads, completions, the seal and its commits, refusals, the entry queue and in-flight counts, over the in-memory backend. -> [completed 2026-10-10](COMPLETED-CHECKLIST.md#di-310352)

- [ ] **DI-3.10.3.5.3** -- **`DurableIoCore`: gates, seals, providers, lineages and failures**: the
  rest of today's `Dioring` and `Core` state moved over the core -- held operations and their
  release, provider requests and answers,
  lineages minted, ended and retired, `resolve` and `import_failure` -- so the core alone carries
  the whole `DurableRing` surface. Tested on the in-memory backend.

- [ ] **DI-3.10.3.5.4** -- **The oracle beside the core**: the push hook gains each write's length,
  and the rule that a write never completes short (DI-D-50), queued here from `DI-3.10.3.2`; the
  oracle run beside the core's tests on the in-memory backend.

- [ ] **DI-3.10.3.5.5** -- **The Windows backend and the Win32 file target**: the backend over
  `windows-ioring-sys`' `IoRing`, threadless -- push, submit, pop, and the ring's completion event
  as its notification -- its refusal causes mapped from `RingCondition`, with a test for each
  mapping a developer machine can reach; and the Win32 file target over a `SharedHandle`, carrying
  its flush domains and warranty class.

- [ ] **DI-3.10.3.5.6** -- **The driver moved onto the core, under pool delivery**: `Dioring` runs
  `DurableIoCore` over the Windows backend, with `EventDelivery` as its delivery (DI-D-52's
  Model A) and an optional callback environment in `Setup` binding its pool; the ring trait's
  readiness an associated type (DI-D-52 Q6); `PushRefusal::Ring` replaced by the backend's
  refusal cause; `FileOptions`, `Core`, and the ring code of `push.rs` and `relay.rs` deleted.
  Today's tests and sabotages are the proof, the sabotages re-anchored where the code moved.

- [ ] **DI-3.10.3.5.7** -- **Pumped delivery** (DI-D-52's Model B): no pool threads; the
  consumer's own thread waits on the notification and drains through a `pump` call, over the
  Windows backend and the in-memory one.

- [ ] **DI-3.10.3.5.8** -- **A blocking push -- proposed, awaiting the engineer's ruling**
  (DI-D-52 Q4): a driver policy above refusal that submits, waits on the notification and
  retries; never from inside a callback, and pumping for itself under pumped delivery. Removed if
  the engineer declines it.

- [ ] **DI-3.10.3.6** -- **The split**: `win-durable-ioring` created and registered for release,
  holding the driver, the Windows backend and file target, `ErrorCode`'s reading from an
  `io::Error`, the readiness `Event`, the interrupt clock and the oracle's readiness check;
  `durable-ioring` left with no Windows dependency and added to CI's `portable` job.
  CONTRACT.md's worked examples drive the Windows driver, so they move to `win-durable-ioring`'s
  documentation, compiled there as doctests, and CONTRACT.md links them; its "Vocabulary ahead of
  the code" note is removed. The core's own tests read interrupt time and Windows faults today, so
  they move to a portable mock clock and fault as they move, and `DioringIds`' default time type
  moves with the Windows crate.

- [ ] **DI-3.10.4** -- **The Win32 file object as a target backing** -- the separate concern
  DI-D-48 names: what such a target warrants by buffering class, and whether dioring reads the
  class from the handle (`FileModeInformation`) rather than taking it on declaration.

- [ ] **DI-3.10.5** -- **A raw partition as a target backing** (the engineer, 2026-10-09: "ok"):
  one volume handle with regions in it, one commit per volume, failure sharing found from the
  volume's disks (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`), and optionally the volume lock
  (`FSCTL_LOCK_VOLUME`); implemented to the interface. How
  [D-44](../windows-ioring-sys/DESIGN-NOTES.md#d-44) applies is open under DI-D-48.

- [ ] **DI-3.9.5** -- **[DI-D-44](DESIGN-NOTES.md#di-d-44)**, from `DI-3.2.6`: the provider
  called from the consumer's next call with dioring's lock released; every request built before
  any is made; what a request names, and in what order; when a write is covered; a domain's
  failure observed with `Reach::Domains` of that one domain; and answers no request asked for
  recorded as inconsistencies. Placed after `DI-3.10`, which restates what a request names, so
  the review reads the shapes as they will stand.

- [ ] **DI-3.9.6** -- **The assistant's choices in executing `DI-3.10.3`**, each step adding its
  own as it lands. From `DI-3.10.3.1`: `SuspectWrite::file` and `IoTargetBusy::file` renamed
  `target`, and `IoTargetBusy` displaying "target". From `DI-3.10.3.2`: two `IoFault`s are equal
  when they name the same source and code and share the one report (`Arc` identity), so a
  nullifier agrees with its write's completion only if it relays that very report; the Windows
  path's fault for a short write carries no code and the message "the write completed short: n
  of N bytes"; a write's requested length rides in its sidecar for that check and no longer in
  the core; the inconsistency for a code-less failed write is gone, P8 having made the code
  optional; and the oracle's no-short-write rule is queued to `DI-3.10.3.5`. From `DI-3.10.3.3`:
  `DiagnosticKind` and `Diagnostic` are non-exhaustive enums, `Inconsistency` carrying the
  instance's number, `what` and the formatted detail; a sink is called with dioring's lock held,
  so it must return promptly and never call back into the instance; `Setup.diagnostics` is an
  `Option<Arc<dyn DiagnosticSink>>`, `None` meaning the driver's default; `StderrDiagnostics`
  prefixes each line `dioring:`; and the Windows driver's default is `NoDiagnostics` until
  `DI-3.6`. From `DI-3.10.3.4`: the clock trait is `StampClock`, its reading type bounded by a
  public `Instant` trait (a name `std::time::Instant` shares); the time type is an associated
  type of `Identities` and a second, defaulted parameter of `DioringIds`; and the core's types
  take it as a parameter, the Windows code naming it through a `WinInstant` alias. From
  `DI-3.10.3.5.1`: the backend trait is `IoBackend`; its types live in a public `backend` module
  and the in-memory backend in a public `memory` module, while `IoTarget`, `IoTargetDescription`,
  `WarrantyClass`, `Geometry`, `GeometryError` and `Capabilities` are at the crate root; the
  target module is `io_target`, since the root `.gitignore`'s `target` ignores any directory so
  named; a `Geometry` holds three alignments -- offset, length, buffer address -- each a power of
  two its constructors enforce, with `admits` and `admits_buffer` the one statement of the rule;
  `Capabilities` names write-through only, its fields private; `IoTarget` requires `Send`, and
  `describe` is a provided method; `SubmissionToken` is a public `u64`, a `RegisteredRegion`
  three `u32`s, a completion's result a `u32` count or an `IoFault`, and a commit transfers zero;
  the refusal causes are `SubmitNeeded`, `DrainNeeded`, `SubmitAndDrainNeeded`, `Busy`,
  `UnknownTarget`, `Unsupported`, `Invalid`, `Unusable(IoFault)` and `Unknown(IoFault)`,
  non-exhaustive, with `needs_submit`, `needs_drain` and `is_retryable`, an unknown cause not
  retryable; `submit`, `pop` and `notification` report an `IoFault`; a submission for a target
  detached before it ran completes with a fault; registration is a backend's construction
  concern, `registered_buffers` counting it. The in-memory backend's buffers are `Vec<u8>`; it
  executes at submit; it refuses a drain when queued and unpopped submissions fill the completion
  queue, checking the target, then the submission's shape, then capacity; its notification is a
  count with `wait_past`; its faults name the source `"memory"` with no code; its queue sizes are
  `NonZeroU32`; a commit copies the bytes to `durable_contents`; and it ignores `WriteCaching`.
  From `DI-3.10.3.5.2`: a consumer operation takes the caller's push as a closure and is recorded
  only once the push accepts it, which is how a refused one is undone with nothing to roll back,
  while the core's own submissions -- commits -- come out in `CoreActions` and go back through
  `refused`, re-emitted with the next actions or by `retry`; `CoreActions` is `submit` and a
  `deliver` flag raised on the queue's transition from empty, and is `#[must_use]`; a seal returns
  its answer and its actions as a pair; the core's write takes an `Epoch`, not a `Tag`, handles
  staying the driver's, and a `WriteCaching` rather than `WriteOptions` until gates arrive in
  `DI-3.10.3.5.3`; geometry and registration are the backend's to check, their refusals reaching
  the consumer as a new `PushRefusal::Backend(RefusalCause)`, beside `Ring` until
  `DI-3.10.3.5.6`; a duplicate target hands back its description in a `DuplicateTarget`, and
  removing one returns it; `new` takes a clock and an `Arc<dyn DiagnosticSink>`, with no default;
  a completion with no record, a second completion, and a consumer operation handed back as
  refused are each an inconsistency (DI-D-46); until `DI-3.10.3.5.3`, every target routes to the
  built-in default, `IoTargetBusy::held_for_gate` is zero, and a gate release, a provider request
  or a never-issued operation reaching the core is an inconsistency; the module is `io_core`, as
  `core` would shadow the `core` crate; and the event-to-entry conversion is one method,
  `Event::into_entry`, which the relay now uses too.

- [ ] **DI-3.3** -- **An in-memory backend whose targets and backend fault on a script**
  ([DI-D-50](DESIGN-NOTES.md#di-d-50) P13), public and deterministic, in the portable crate: an
  operation failed, held, or completed late or out of order; a submission refused; a full queue;
  a short count, to test a backend's short-write policy. It answers the problem that flush failure
  cannot be produced on a healthy machine, for consumers' failure paths and not only this crate's,
  in the spirit of [RESPONSE-SPACE.md](../windows-ioring-sys/RESPONSE-SPACE.md), and is the
  basis for unit testing and driving the core (the engineer: "In-memory is a good basis for unit
  testing and driving"). It does not replace DI-4.1.

- [ ] **DI-3.11** -- **The fault seam's merge-or-delete decision**, once `DI-3.3` is proven: the
  `fault-injection` feature's `fail_next_flush` duplicates what the in-memory backend scripts
  ([DI-D-50](DESIGN-NOTES.md#di-d-50)), and stays until then under the duplicate-then-decide rule.

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

- [ ] **DI-3.6** -- **Emit the delay events** DI-2.11 designs, as typed events the core reports to
  its diagnostic sink, and **the Windows sinks** in `win-durable-ioring`: `EtwDiagnostics`, from a
  manifest-based ETW provider that dioring owns and the Windows driver's default, and
  `DebugOutputDiagnostics`, through `OutputDebugStringW` ([DI-D-51](DESIGN-NOTES.md#di-d-51)).
  Gated on `DI-3.10.3`, which creates the sink. Carries what DI-2.11 deliberately left to
  implementation:
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
  arrives. It reports to the instance's diagnostic sink ([DI-D-51](DESIGN-NOTES.md#di-d-51)), so
  its diagnostics go wherever the consumer sent the instance's. Tested alone against fake providers. Gated on the provider
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
  What the published record does say -- Windows' own "the data has been lost" write-behind codes,
  Microsoft assigning the handling to the application, and nothing on retention after a failed
  flush -- was gathered on 2026-10-09 and is in
  [the design session](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md#what-the-published-record-says-research-2026-10-09).

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
