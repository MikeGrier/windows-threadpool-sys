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

## Where the open-surface work goes next (read this first when picking the work back up)

The open surface of a file ([DI-M13](#di-m13----opening-a-file-the-surface-and-its-home) to
[DI-M15](#di-m15----opening-by-identity-and-reopening)) cannot be done in this crate alone: it
changes four others. This is the order across all of them. **No status is kept here**, because each
item's own checkbox is the status and a copy would go stale; this list is only the order and the
links. Each item below also ends with a `Next:` line pointing at the step after it.

**Step 0, added 2026-10-15, before step 1** (the engineer ruled that volume operations belong in the
volume crate and that the error codes become one shared type; see
[WV-D-5](../win-volume-sys/DESIGN-NOTES.md#wv-d-5) and
[WE-D-1](../win-error-codes/DESIGN-NOTES.md#we-d-1)):

- 0.1. [WE-1.1 to WE-1.3](../win-error-codes/CHECKLIST.md) -- the new `win-error-codes` crate.
- 0.2. In any order, each needing only `WE-1.1`:
  [NR-4.1](../windows-namespace-request-sys/CHECKLIST.md) (request crate adopts it),
  [ERR-1](../windows-file-enumeration-sys/CHECKLIST.md) (enumeration crate adopts it), and
  [WV-2.1 to WV-2.5](../win-volume-sys/CHECKLIST.md) (the volume crate takes in the storage-locality
  entries, in that order).
- 0.3. [NR-2.8](../windows-namespace-request-sys/CHECKLIST.md) -- the request crate deletes the
  originals. **It must land before step 4's release**, or the entries are published from the crate
  they do not belong in. `NR-4.1` should land before it too, so that release carries the
  `io::Error` fix.

Steps 1 to 3 do not depend on step 0 and may be done first if the order suits; step 4 does.

1. [NR-3.3](../windows-namespace-request-sys/CHECKLIST.md) -- request crate: the captured root and the
   graft, as one internal type. Additive; nothing breaks.
2. [NR-3.4](../windows-namespace-request-sys/CHECKLIST.md),
   [NR-3.5](../windows-namespace-request-sys/CHECKLIST.md) and
   [NR-3.6](../windows-namespace-request-sys/CHECKLIST.md) -- request crate: **one coupled breaking
   commit.** `OpenFile` and `WatchDirectory` take a `Wtf16String`, an interior NUL is refused,
   `prepare` is retired. `win-durable-ioring` and `windows-platform-probes` call `prepare` through
   path dependencies, so **the same commit adapts both just enough to build**, and the request
   crate's own tests and docs; a commit that leaves the workspace unbuilt is the worse outcome.
3. [NR-3.1](../windows-namespace-request-sys/CHECKLIST.md) and
   [NR-3.2](../windows-namespace-request-sys/CHECKLIST.md) -- request crate: the SDDL constructor and
   the `ReOpenFile` entry. Additive, independent of steps 1 and 2; may be done first.
4. **Release the request crate** (the release-please pull request, then publish). A breaking change,
   so a new minor. This is a release action and not a checklist item.
5. `DI-13.1` is done: its table is [DI-D-64](DESIGN-NOTES.md#di-d-64), and `DI-13.3` onward build on
   it.
6. After step 4: [PATH-1](../windows-file-enumeration-sys/CHECKLIST.md), the enumeration crate
   consumes the new contract and deletes its older copy (the workspace's `M26+.3`, and
   [REVIEW-1](../windows-file-enumeration-sys/CHECKLIST.md) is re-read); `DI-13.5` and `DI-13.6`, this
   crate takes the release that carries the SDDL constructor and the `ReOpenFile` entry.
7. Then, here: `DI-13.7`, which is five items `DI-13.7.1` to `DI-13.7.5` (none needs another crate), `DI-M14`,
   `DI-M15`. `DI-10.5` needs none of them and may be done at any point, and `DI-10.6` is parked.

If you remember only one thing: **the request crate comes first, and its breaking commit moves three
crates at once.**

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

- [x] **DI-9.7.11** -- DECIDED (c): `ErrorCode`'s `from_win32` and `win32` move out of the portable core into `win-durable-ioring`, built on `HResult`. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-9711)

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

- [x] **DI-10.5** -- A consumer who knows a device is theirs alone can attest it through `new_attested_exclusive`, `from_volume_attested_exclusive` and `from_disk_attested_exclusive`, and the target is then `Complete`. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-105)

`DI-10.3`, whether a handle opened elsewhere can be given to a file target, is decided by
[DI-D-68](DESIGN-NOTES.md#di-d-68) and is stubbed below; what it leaves is `DI-10.6`, parked under `DI-M15+`.

## DI-M13 -- Opening a file: the surface and its home

Why this milestone exists. [DI-D-60](DESIGN-NOTES.md#di-d-60) made a file target from a file this
crate opened and from nothing else, and [`FileOptions`](../win-durable-ioring/src/windows_backend/opened_file.rs)
carries only `read`, `write`, `create`, `create_new` and `truncate`. A consumer whose open needs more
(a share mode, a security descriptor, delete-on-close, an open by file ID) cannot say so, and would
want a handle from elsewhere ([DI-D-68](DESIGN-NOTES.md#di-d-68), [DI-10.6](#di-m15----handles-opened-elsewhere-parked-behind-di-m15)).
Widening what this crate can open is the structural answer: every option a consumer can pass is an
option recorded by construction. The rule throughout is documented APIs only
([copilot-instructions.md](../../.github/copilot-instructions.md)): no `Nt*`, no `Rtl*`.

- [x] **DI-13.1** -- The open surface, written down as a table over every parameter of the open calls, with the engineer's rulings (a) to (g), in [DI-D-64](DESIGN-NOTES.md#di-d-64). -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-131)

- [x] **DI-13.2** -- A file is opened through `windows-namespace-request-sys`, so this crate has no `unsafe` of its own for it and `std::fs::OpenOptions` is no longer the opener. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#di-132)

- [x] **DI-13.3** -- Access is a typed set of named rights, `FileRights`, with no raw `u32`; a file target needs a data right, checked when it is opened. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-133)

- [x] **DI-13.8** -- Data access is the type parameter `A` of `OpenedFile`, `OpenedDisk` and `OpenedVolume`, and `FileRights` keeps only the rest. <a id="di-138"></a> -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-138)

- [x] **DI-13.9** -- `IoTarget` states its data access, and the core refuses what the access does not cover. <a id="di-139"></a> -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-139)

- [x] **DI-13.10** -- A typed `Key<A>` carries the access to the ring's submit methods. <a id="di-1310"></a> -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-1310)

- [x] **DI-13.4** -- Share mode as a typed set, with a sharing violation as a typed error. <a id="di-134"></a> -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-134)

- [x] **DI-13.11** -- A share mode for `OpenedVolume` and `OpenedDisk`, through one device open. <a id="di-1311"></a> -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-1311)

- [x] **DI-13.12** -- The exclusive-volume and exclusive-disk tests run elevated, and the lock measured against the share mode. <a id="di-1312"></a> -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-1312)

- [x] **DI-13.5** -- `SecurityDescriptor` from SDDL, taken from the request crate's release. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-135)

- [x] **DI-13.6** -- A `ReOpenFile` entry, taken from the request crate's release. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-136)

- [x] **DI-13.7** -- `OpenedFile::open` reflects Win32 and not `std`, and `DI-D-63` is amended. <a id="di-137-rework"></a> -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#di-137)

## DI-M14 -- Opening a file: flags, attributes, security

- [x] **DI-14.1** -- Cache hints: `FILE_FLAG_RANDOM_ACCESS` and `FILE_FLAG_SEQUENTIAL_SCAN` are builder methods on `FileOptions`, passed to a cached and an unbuffered file alike. -> [completed 2026-10-16](COMPLETED-CHECKLIST.md#di-141)

- [x] **DI-14.2** -- `FILE_FLAG_DELETE_ON_CLOSE`, `FILE_FLAG_OPEN_NO_RECALL` and `FILE_FLAG_POSIX_SEMANTICS` are builder methods on `FileOptions`. <a id="di-142-flags"></a> -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#di-142)

- [x] **DI-14.3** -- Creation attributes (`FileAttributes`), a security descriptor on the request, and no inheritance. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#di-143)

- [x] **DI-14.4** -- The open's arguments, read against the documented contract: no condition `OpenedFile` can express is invalid, so no check is built. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-144)

- [x] **DI-14.5** -- The remote-device check is made on the opened handle, and nowhere else. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-145)

- [x] **DI-14.6.1** -- What an open handle reports about its own kind is measured over a disk file, a directory, `NUL`, a console, a pipe, a volume and a physical disk. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-1461)

- [x] **DI-14.6.2** -- A cached open of a handle that is not a disk file is accepted; an unbuffered one is refused as `UnknownLocation`, this crate's own narrowing, and no longer as the final-path call's error. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-1462)

- [x] **DI-14.6.3** -- The serial-port measurement is withdrawn: nothing decided rests on it, and what was observed is recorded. -> [withdrawn 2026-10-18](COMPLETED-CHECKLIST.md#di-1463)

- [x] **DI-14.6.4** -- The open's documentation states that where a name leads is the caller's responsibility. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-1464)

- [x] **DI-14.12** -- A caller asks what kind of handle an opened file is, of an observer, `HandleKindQuery`, and the opener does not return it. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-1412)

- [x] **DI-14.7** -- The ring on a handle opened with `FILE_FLAG_OVERLAPPED`, run against the same handle without it, on a cached and an unbuffered file. -> [completed 2026-10-17](COMPLETED-CHECKLIST.md#di-147)

- [x] **DI-14.8** -- Safe read-backs of what an open was given, in the two crates the Learn-section rule names: `win-authorization-sys` and `win-handles-sys`. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-148)

- [x] **DI-14.9** -- `FILE_FLAG_OVERLAPPED` is a builder method on `FileOptions`, ruled by the engineer, 2026-10-18. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-149)

- [x] **DI-14.10** -- An unbuffered open of a link to a share, opened as the link, is accepted, ruled by the engineer, 2026-10-18. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-1410)

- [x] **DI-14.11** -- A volume opened without sharing writes stays allowed: the measurement contradicts the page's requirement, so it is documented and not refused. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-1411)

## DI-M15 -- Opening by identity, and reopening

- [x] **DI-15.1** -- A file's identity, `FileId`, is read from an opened file or any shared handle, and the `lpFileId` row cites the `FILE_ID_DESCRIPTOR` page. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-151)

- [x] **DI-15.2** -- An open by identifier, `OpenedFile::open_by_id`, takes any of the three identifier kinds, its own `ByIdOptions` and any handle as the hint, and checks a `FileId` against the file opened. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#di-152)

- [ ] **DI-15.4** -- **`OpenedFile::reopen`, through `ReOpenFile`** (the entry is `DI-13.6`). A new `OpenedFile<B2, A2>` from an
  owned one **or from any handle** ([DI-D-68](DESIGN-NOTES.md#di-d-68): the route for a handle opened elsewhere, since the new
  handle's mode is this crate's choice; the residue it cannot convert is `DI-10.6`), changing access (a change of `A`, [DI-D-65](DESIGN-NOTES.md#di-d-65); whether the kernel
  lets a reopen widen the access is a test here), sharing and flags: cached to unbuffered and back, a read-only handle for
  a scrubber beside the writer. The target's alignment is recomputed for the new mode, and the checks
  are the open's, on the reopened handle ([DI-14.5](COMPLETED-CHECKLIST.md#di-145)): the remote check
  and, unbuffered, `UnknownLocation` ([DI-14.6.2](COMPLETED-CHECKLIST.md#di-1462)), which a reopen
  from any handle can meet, since the original may be a pipe or `NUL`. `ReOpenFile` documents that the
  new access and sharing cannot conflict with a previous open that is still open; each conflict the
  table lists is a test, and a sharing violation reaches the caller as the typed error from `DI-13.4`,
  not a panic: `SharingViolation` names what was opened by `OpenedBy` ([DI-D-70](DESIGN-NOTES.md#di-d-70)),
  which gains a form for what a reopen names. Documented as well: the object must have been created by `CreateFile`; an
  `OpenedVolume` and an `OpenedDisk` are opened through `CreateFileW` too, so whether a volume or a disk
  reopens is a test, not an assumption. The request crate's tests already show a reopen widening read to
  write on a file; this layer tests it again on its own types. Also tests, from
  [DI-D-64](DESIGN-NOTES.md#di-d-64)'s unverified list: whether a handle that came from `OpenFileById`
  (`DI-15.2`) reopens, and whether the reopened handle is inheritable.

- [ ] **DI-15.5** -- **Decide whether an open by identifier that reaches a directory is refused.** Observed
  ([DI-D-70](DESIGN-NOTES.md#di-d-70)): `OpenFileById` opened a directory without `FILE_FLAG_BACKUP_SEMANTICS`, for
  every data access and in either buffering mode, though its page says the flag is required to obtain a handle to a
  directory, and an open of the same directory by path is refused by the kernel. So an `OpenedFile`, and a
  `Win32FileTarget` made from one, can now be a directory, which no open before could make. The engineer's to rule:
  refuse it on the opened handle, where the other checks are made ([DI-14.5](COMPLETED-CHECKLIST.md#di-145)); leave
  it to the caller, as [DI-D-67](DESIGN-NOTES.md#di-d-67) leaves where a name leads; or something else. The test
  `a_directory_was_observed_to_open_by_identity_where_by_path_it_is_refused` states today's behaviour and follows the
  ruling.

## DI-M15+ -- Handles opened elsewhere (parked behind DI-M15)

- [x] **DI-10.3** -- Whether a handle opened elsewhere can be given to a file target is decided: not in this version, and a device's exclusivity can be attested instead. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#di-103)

- [ ] **DI-10.6** -- **A handle opened elsewhere that `ReOpenFile` cannot convert: restrictive sharing, an access the caller was never itself granted, and an object that was not created by `CreateFile`.** Parked, with the blocker named: an open handle's mode is reported only by an undocumented call, which this repository does not make, so such a handle's class is a consumer's declaration, and the engineer has not ruled on an `unsafe` attestation for one ([DI-D-68](DESIGN-NOTES.md#di-d-68)). Everything else a handle from elsewhere is wanted for is [DI-15.4](#di-154), taking any handle. When picked up: decide whether a declaration is accepted at all, in what form, and that the class is at most `BestEffort` unless the consumer attests more through the mechanism of `DI-10.5`.

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

- [ ] **DI-6+.11** -- **Revisit `CREATE_ALWAYS` and `TRUNCATE_EXISTING`, which v1 leaves out** (the engineer,
  2026-10-16, "just to simplify the matrix", to be revisited at a later date). Parked, neither added nor
  dropped. What an answer needs is in [DI-D-64](DESIGN-NOTES.md#di-d-64): `TRUNCATE_EXISTING` was
  accepted only with the literal `GENERIC_WRITE`, and `CREATE_ALWAYS` emptied a file under every access
  mask including a read-only one, so offering either means deciding what a handle typed not to write may
  be opened to do. It is answerable once the three dispositions of [DI-13.7.3](#di-137-rework) are in use.
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
    [DI-15.5](#di-m15----opening-by-identity-and-reopening) is to rule on, and which decides whether the substitute
    reaches directories.
