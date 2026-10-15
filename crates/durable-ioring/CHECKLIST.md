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

- [x] **DI-9.8.2** -- The portable CI job's first run is read: the crate's tests run on Linux and macOS. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#di-982)

## DI-M7+ -- Delivery follow-ups

- [x] **DI-7+.1** -- A wait-only handle on the ring's own completion event, offered by a pumped instance as `Dioring::completion_event`. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-7+1)

- [x] **DI-7+.2** -- The engine rings the doorbell, a backend's push only queues, and a failed doorbell or an unreadable ring reaches the consumer once, with no timer. -> [completed 2026-10-13](COMPLETED-CHECKLIST.md#di-7+2)

- [x] **DI-7+.3** -- The Windows backend's ring is an owned one: its pop has no case of a completion the ring held nothing for, and the panic and error rule is recorded. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#di-7+3)

- [x] **DI-7+.4** -- Model A's handler is told when the instance's pop fails, and the in-memory backend refuses what it cannot hold. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#di-7+4)

## DI-M10 -- Target backings

- [x] **DI-10.1** -- A Win32 file target is made from a file this crate opened, so its class, alignment and write-through follow from how it was opened and not from a declaration. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#di-101)

- [x] **DI-10.2** -- A raw volume is a file target made from a volume this crate opened, with its disks found and never `Complete`. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#di-102)

- [x] **DI-10.4** -- A whole physical disk is a file target made from a disk this crate opened by number, its own flush domain and never `Complete`. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#di-104)

`DI-10.3`, whether a handle opened elsewhere can be given to a file target, is parked behind
`DI-M15` as `DI-M15+` below; it kept its ID because [DI-D-60](DESIGN-NOTES.md#di-d-60),
[DI-D-61](DESIGN-NOTES.md#di-d-61) and [DI-D-62](DESIGN-NOTES.md#di-d-62) cite it.

## DI-M13 -- Opening a file: the surface and its home

Why this milestone exists. [DI-D-60](DESIGN-NOTES.md#di-d-60) made a file target from a file this
crate opened and from nothing else, and [`FileOptions`](../win-durable-ioring/src/windows_backend/opened_file.rs)
carries only `read`, `write`, `create`, `create_new` and `truncate`. A consumer whose open needs more
(a share mode, a security descriptor, delete-on-close, an open by file ID) cannot say so, and would
want a handle from elsewhere ([DI-10.3](#di-m15----handles-opened-elsewhere-parked-behind-di-m15)).
Widening what this crate can open is the structural answer: every option a consumer can pass is an
option recorded by construction. The rule throughout is documented APIs only
([copilot-instructions.md](../../.github/copilot-instructions.md)): no `Nt*`, no `Rtl*`.

- [ ] **DI-13.1** -- **The surface, written down before any of it is built.** One decision in
  [DESIGN-NOTES.md](DESIGN-NOTES.md), with a table over every parameter of the documented open
  calls -- `CreateFile2` and its extended parameters, `OpenFileById`, `ReOpenFile` -- saying for each
  whether it is exposed (and as which type) or excluded (and the analytic reason, per the option
  integrity rule: it computes the wrong answer, not that no consumer is in sight). Each row also says
  what the option does to a target's claims: class, alignment, write-through, offsets. Rows ruled by
  the engineer on 2026-10-15, to be recorded in the table with these reasons: **append-only access is
  excluded** (it does not fit this crate's use pattern; as the assistant reads the documentation,
  `FILE_APPEND_DATA` without `FILE_WRITE_DATA` lands a write at end of file whatever offset it names,
  and the core addresses by offset, so it would compute the wrong answer); **handle-level
  `FILE_FLAG_WRITE_THROUGH` is not offered in v1** (what it can warrant is unclear, and the answer is
  a measurement of its effect on a cached file); **inheritable handles are not offered in v1**.
  Neither of the last two schedules work: each is a v1 ruling, reopened by a consumer or a
  measurement and not by this checklist. Exclusions confirmed by the engineer, with their reasons:
  `CreateFileTransacted` (a transacted write is durable at the transaction's commit, so the epoch's
  warranty would mean nothing), `hTemplateFile`, and directory handles by
  `FILE_FLAG_BACKUP_SEMANTICS` (the ring moves file data).

- [ ] **DI-13.2** -- **The unsafe home for opening, and one wrapper over the documented open call.**
  The crate that holds the `unsafe` is the engineer's to name: a new `-sys` crate for opening, or
  [win-volume-sys](../win-volume-sys/README.md) widened, whose name says volumes. Either way the
  wrapper takes typed parameters and no raw flag word, returns an owned handle, and is the only
  place the crate calls `CreateFile2`, so `std::fs::OpenOptions` stops being the opener and its gaps
  (no security attributes on stable, no template, no by-ID) stop being ours. `OpenedFile::open`
  moves onto it with no change a caller can see; `DI-13.1`'s table is its specification.

- [ ] **DI-13.3** -- **Access as named rights, not a read/write pair.** A typed set over the
  documented file access rights the table in `DI-13.1` exposes (data read and write; attributes;
  `DELETE`; the security rights; append-only if ruled in), with no raw `u32`. A target needs a data
  right, which is checked when the file is opened and not when the first operation fails. Tests: each
  right opens and is exercised; a right the file's security refuses reaches the caller as the
  kernel's error and not as a panic.

- [ ] **DI-13.4** -- **Share mode as a typed set**, with today's all-three default kept and an
  exclusive open (no sharing) expressible, since a database that owns its file wants it. A sharing
  violation is a typed error that names which side conflicted where the kernel says. Tests with two
  opens: each combination the table lists is accepted or refused as `CreateFile2` documents.

## DI-M14 -- Opening a file: flags, attributes, security

- [ ] **DI-14.1** -- **Cache hints.** `FILE_FLAG_RANDOM_ACCESS` and `FILE_FLAG_SEQUENTIAL_SCAN`,
  each with what `DI-13.1` ruled it warrants. A hint that only means something to a cached file is
  not offered to an unbuffered one, by type where the buffering mode is in the type and by a typed
  error where it is not. Handle-level `FILE_FLAG_WRITE_THROUGH` is not offered in v1 by the
  engineer's ruling (`DI-13.1`); the per-write flag is unaffected, and is known to fail with error
  509 on a cached handle even one opened with that flag (a kept test).

- [ ] **DI-14.2** -- **Namespace and lifetime flags.** `FILE_FLAG_DELETE_ON_CLOSE`,
  `FILE_FLAG_OPEN_REPARSE_POINT`, `FILE_FLAG_OPEN_NO_RECALL` and `FILE_FLAG_POSIX_SEMANTICS`, plus
  `FILE_FLAG_OVERLAPPED` if the ring turns out to need or refuse it (settled by a test on the ring,
  not by recollection). The remote-device check in `DI-14.5` must see a reparse point's target, not
  the reparse point. Tests: delete-on-close removes the file when the last handle closes; a reparse
  point opened with the flag is the point and without it is the target.

- [ ] **DI-14.3** -- **Creation attributes, security descriptor, inheritance.** The file attributes a
  new file can be created with (hidden, read-only, temporary, encrypted and the others the table
  lists), as a typed set. A security descriptor built from SDDL by a documented conversion call,
  owned and freed by the wrapper, so a caller never holds a raw pointer. Handles are never
  inheritable in v1 (the engineer's ruling, `DI-13.1`): an inheritable handle reaches child processes
  that can write through it, so there is no option to set. Security quality-of-service flags where
  the table keeps them. Tests: a created file carries the attributes and the descriptor asked for,
  read back with a documented query; the handle is not inheritable, read back with
  `GetHandleInformation`.

- [ ] **DI-14.4** -- **Combinations are refused where the kernel refuses them.** Every pair the table
  marks as invalid or silently ignored is tested on this machine and is either refused with a typed
  error before the file is created, or documented as ignored with the test that shows it. A
  combination refused only after a file was created is the worse outcome, so the refusal is ahead of
  the open wherever the table lets it be.

- [ ] **DI-14.5** -- **The remote-device check is made on the opened handle, not on the path.**
  [DI-D-60](DESIGN-NOTES.md#di-d-60) records a window between canonicalising the path and opening it,
  in which a link can be swapped. A documented query of the handle closes it, and it is also what an
  open by ID needs, since it has no path. The assistant must say which documented handle queries it
  checked and which it chose. A file created on a remote device before the check can refuse it
  leaves a file behind, which is why the path check stays for a create and the handle check is the
  authority for every open.

## DI-M15 -- Opening by identity, and reopening

- [ ] **DI-15.1** -- **A file's identity, read from an opened file.** A `FileId` type over the
  documented identity query (the 128-bit form that also covers 64-bit file systems), and the volume it
  belongs to. Which of the identifier kinds `FILE_ID_DESCRIPTOR` accepts are offered (the file ID,
  the extended 128-bit ID, the legacy object ID) is a row in `DI-13.1`'s table. Tests: the ID of an
  opened file is stable across two opens and differs between two files.

- [ ] **DI-15.2** -- **`OpenedFile::open_by_id`.** An open by `FileId` through `OpenFileById`, with a
  volume hint that must be a file this crate opened or a volume it opened, so the hint is
  provenance-checked like everything else. It takes the full typed set from `DI-M13` and `DI-M14`. The
  remote check runs on the hint's device (`DI-14.5`). Tests: round trip of a file opened by path and
  again by ID; an ID from a directory listing (`FileIdExtdDirectoryInfo`) opens by ID; a stale ID is
  the kernel's error.

- [ ] **DI-15.3** -- **Opening relative to a directory handle: confirm there is no documented route,
  then record what stands in for it.** The assistant knows of no documented Win32 call that opens a
  name relative to a directory handle; the only one it knows is `NtCreateFile` with a root directory,
  which is out under the repository's rule. That is a reading to verify against the documentation and
  not a finding. If it holds, the decision records it and names the substitute: list the directory,
  take the child's ID, open it by ID (`DI-15.2`). If a documented route exists, it is an item here.

- [ ] **DI-15.4** -- **`OpenedFile::reopen`, through `ReOpenFile`.** A new `OpenedFile<B2>` from an
  owned one, changing access, sharing and flags: cached to unbuffered and back, a read-only handle for
  a scrubber beside the writer. The target's alignment is recomputed for the new mode, and going
  unbuffered runs the remote check. `ReOpenFile` documents that the new access and sharing cannot
  conflict with a previous open that is still open; each conflict the table lists is a test, and a
  sharing violation reaches the caller as the typed error from `DI-13.4`, not a panic. Documented as
  well: the object must have been created by `CreateFile`, which an `OpenedFile` is and an
  `OpenedVolume` or `OpenedDisk` may not be, so whether those reopen is a test, not an assumption.

## DI-M15+ -- Handles opened elsewhere (parked behind DI-M15)

- [ ] **DI-10.3** -- **Whether a handle opened elsewhere can be given to a file target, and under what attestation, asked of what `DI-M13` to `DI-M15` leave.** Disallowed in this version by the engineer's ruling ([DI-D-60](DESIGN-NOTES.md#di-d-60)): the open mode of a handle cannot be read with a documented call, so the only routes are a consumer's declaration, which can be wrong, or an `unsafe` attestation in the style of `WaitableHandle`'s seam for handles from elsewhere. The blocker is that ruling and not the lack of a consumer. It is parked here because most reasons to want a foreign handle are opens this crate could not express, which the milestones above remove, and what remains is the handle a broker or another library hands over. Decide which route, if either, and what class such a handle may claim: at most `BestEffort` unless the consumer attests more. The same mechanism is how a raw volume or a whole disk could claim `Complete` ([DI-D-61](DESIGN-NOTES.md#di-d-61), [DI-D-62](DESIGN-NOTES.md#di-d-62)): a handle cannot show that nothing else writes the disks, so only an attestation by a consumer that knows could. A wrong attestation is silent, which for a durability layer is the worst failure; that is the argument for `unsafe` over a builder method.

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
