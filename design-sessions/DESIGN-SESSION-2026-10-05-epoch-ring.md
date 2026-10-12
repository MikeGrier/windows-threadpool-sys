# Design session -- an epoch-driven ring as its own crate (2026-10-05)

**Summary.** The engineer proposed a new crate: an `IoRing`-like type, layered on
`windows-ioring-sys`, that carries the epoch-driven durability model explored in
[examples/epoch_log/](../crates/windows-ioring-sys/examples/epoch_log/) so that a database or
filesystem gets a ring API with durability requirements scheduled into the ring's work. The
direction below was **agreed in direction by the engineer, with details still to be worked
through.** No decision IDs have been assigned and nothing here is binding yet: every item under
"Working positions" is a working position, not a decision, and no checklist work has been
scheduled from this session so far. This file is a checkpoint taken mid-session so the
discussion survives an interruption.

Branch: `mikegrier/public-epoch-ring`, level with `main` when the session opened.

## The proposal, in the engineer's words

> We built some notions here in the example of how to implement a ring using an epoch notion to
> drive durability intervals for lack of a better term to force whatever durability primitives
> were available across whatever storage was appropriate for a log and other storage.
>
> I want to do something both less and more. I want to build an IoRing-like type in its own
> crate that uses the same epoch driven durability concepts that were explored here layered on
> top of the ring in windows-ioring-sys so that a database or filesystem would have a ring api
> that they could use easily to have access to the underlying operating system's IoRing
> capabilities and be able to schedule durability requirements into the ring's work in a
> natural fashion. Epochs seem like a very natural approach to the problem.

"Less": not the sample's log policy (record format, replay, checkpoint, reclaim). "More": a
general ring API a storage engine can adopt, rather than one worked composition.

## How it relates to what is already recorded

- **It does not contradict [D-54](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-54).** That
  decision keeps durability *groups* and the `Epoch` concept out of `windows-ioring-sys` and
  points them at a separate crate. A separate crate is what is proposed. Likewise
  [D-26](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-26) and
  [D-8](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-8) are honoured: the policy lands in a
  layer above the mechanism, not inside it.
- **It is the C-3 crate** from
  [DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md),
  queued as `M33+.5` in [CHECKLIST-io-domains.md](../CHECKLIST-io-domains.md) -- **with one
  change of shape.** `M33+.5` says the durability layer "contains a domain and submits through
  it". This proposal layers directly on `IoRing` instead.

## Working positions (agreed in direction; not decisions)

1. **Build on `IoRing` directly, not on the domain runtime.** The domain runtime (`M33+`) is
   gated on M32 and on the topology planner; coupling durability to it would park this work
   behind questions it does not depend on. Layering on one ring also yields C-3's constraint by
   construction -- one epoch sequence per epoch ring -- and a domain can later *contain* an
   epoch ring. **This amends `M33+.5`'s stated shape and must be recorded there**, including the
   containment direction, once settled.
2. **The `epoch_log` sample stays as it is.** Duplicate-then-decide (PLATFORM INTEGRITY rule 1):
   the crate is a new path beside the sample. Whether the sample is later rebuilt on the crate is
   a merge-or-delete decision for when the crate is proven, and must be tracked so it is not
   forgotten.
3. **Write the crate's contract first**, as
   [contract.rs](../crates/windows-ioring-sys/examples/epoch_log/contract.rs) was: what "epoch E
   is durable" means across many files, what failure does to an epoch, and what
   release-after-durable promises.
4. **Durability belongs in the completion stream.** "Epoch N durable" (or failed) arrives as an
   entry interleaved with operation completions, rather than only as a side query -- the shape an
   io_uring-style consumer already expects.
5. **Scope.** The crate owns epochs, per-epoch dirty-file tracking, the commit strategies,
   release-after-durable, and failure semantics. It does not own record formats, replay,
   checkpointing, or reclamation -- those remain sample or application concerns.

## What the single-file sample never had to answer

Raised by the assistant; agreed in direction. Each shapes the API.

### 1. Multi-file epochs

The sample commits one file. A database or filesystem writes a WAL, data files and metadata in
the same epoch. The barrier is ring-wide but a flush is per file, and
[D-47](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-47-detail) means a drained operation
holds back nothing pushed after it. So committing N files is either:

- **a chain of N covering flushes**, each carrying the drain flag, which serialises N device
  flushes; or
- **host sequencing**: observe the epoch's writes complete, then push N unordered flushes, which
  may proceed concurrently.

In the sample the three strategies were not distinguishable on the development machine. With
many files the difference is structural rather than a workload accident, which strengthens the
case under OPTION INTEGRITY for exposing the strategies rather than picking one. It also adds
bookkeeping the sample never needed: the set of files each epoch dirtied.

### 2. Release-after-durable (the write-ahead rule)

A database needs more than "epoch N is durable"; it needs "do not issue this data-page write
until the log record covering it is durable". Journalling filesystems need the same for a
commit block. Under D-47 the ring cannot enforce it, because nothing after a barrier is held.
So the crate offers something like "submit this after epoch N is durable": held in userspace and
released when the durable watermark reaches N. The assistant's view, not yet discussed in
detail: **this is the feature that makes the crate more than the sample with a nicer API**, and
it is where "scheduling durability requirements into the ring's work" concretely lives.

### 3. Failure semantics -- discussed; see "Failure semantics, round 2" below for the current position

- **A covering flush orders execution; it does not aggregate results.**
  [checkpoint.rs](../crates/windows-ioring-sys/examples/epoch_log/checkpoint.rs) learned this;
  [commit.rs](../crates/windows-ioring-sys/examples/epoch_log/commit.rs) does not apply it. An
  epoch is durable only if every operation in it succeeded *and* its flushes succeeded.
- **A failed flush may not be recoverable by a later one.** `commit.rs` argues a failed commit
  "is not permanent" because a later covering flush settles it. That is the reasoning PostgreSQL
  relied on before its 2018 fsync failure ("fsyncgate"): after a failed flush the OS may have
  discarded the dirty data, and a later flush then succeeds over data that is gone. With
  `NO_BUFFERING` there are no cached dirty pages, so the argument may hold there; for buffered
  handles Windows specifies nothing either way.
- **Superseded by round 2 below.** The first working position here was that a failed epoch is
  sticky by default, any relaxation to be earned by a known handle property. Round 2 replaces
  stickiness with recovery from retained data. The sample's reasoning is
  probably a latent defect in its own right and should be queued separately once this is
  settled.

### 4. Completion routing

The epoch layer must observe every completion to advance its watermark. Natural for a Model B
drain. Under Model A, `EventDelivery` owns the ring and its callback, so the epoch layer would
have to sit inside that callback. Both models are first-class
([D-3](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-3)), so this has to be designed in rather
than retrofitted.

### 5. Alternating rings stretch "one epoch per ring"

Keeping `AlternatingRings` (OPTION INTEGRITY) means one epoch sequence can span two `IoRing`s.
So the unit is the **epoch ring**, not the `IoRing`; C-3's per-domain constraint survives as
long as the epoch ring is the domain-equivalent unit. A client spanning two epoch rings still
gets no single durability point and needs two commits and an explicit join.

## The crate's name: `durable-ioring` (settled by the engineer)

The package is `durable-ioring` (hyphenated, matching every package in the workspace; the Rust
crate identifier is `durable_ioring`). No `-sys` suffix: in this repository `-sys` marks the layer
that makes the unsafe Win32 calls, and this crate is expected to contain no unsafe code.

`epoch-ring` was considered by both parties and rejected by the engineer on this ground:

> I think that the top line feature is the durability; the epoch is just how we are achieving it.

The assistant had raised the opposite concern -- that "durable" names an outcome the crate cannot
guarantee, since durability rests on the device honouring the flush -- and conceded: the name
should state the capability the crate owns (Design Autonomy), and the over-claim risk belongs to
the contract, which must say plainly what the crate assumes rather than guarantees.

**No unsafe code, enforced rather than assumed (working position).** `#![forbid(unsafe_code)]`
from the first commit. That constrains the API: the sample needs `unsafe` only because it pushes
through raw handles, so this crate takes files in owning forms (`SharedFile`, `RegisteredFile`,
`flush_owned`) and never as `RawHandle` -- which per-epoch dirty-file tracking wants anyway, for
file identity. If a Win32 call is ever needed that no lower crate wraps safely, the fix goes in the
lower layer (mono-repo bug policy), not into an `unsafe` block here.

## Failure semantics, round 1: fail-stop (withdrawn)

The assistant first proposed **fail-stop at epoch-ring granularity**: the first failed epoch
puts the ring into a terminal state, `durable_through` never passes it, gated operations are
refused, and the caller recovers by closing the ring and running its own crash recovery. The
failure classes tabled then still stand as a classification: a failed, short, or cancelled write
fails its epoch; a refused push never joined one; reads are not epoch members; a never-completing
operation leaves its epoch unresolved and the crate invents no timeout
([D-67](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-67)).

The third argument offered for it -- "this is how storage engines already behave; fail-stop hands
them the state their crash recovery handles" -- was challenged by the engineer:

> on your point number 3, that seems to imply that this is an insufficiently rich or robust data
> structure to use to build a filesystem or database on top of. this is precisely my goal
>
> i am not pushing back, i want to advance the design

The assistant agreed the challenge is correct, not merely a preference: point 3 assumed a layer
above would absorb failure, which is wrong for a crate whose purpose is to *be* the substrate a
storage engine builds on. `ERROR_DISK_FULL` alone refutes fail-stop as the model -- an ordinary,
recoverable event for a database, which fail-stop would make fatal.

## Failure semantics, round 2: custody, suspect sets, and recovery (working position)

**The insight.** PostgreSQL's 2018 fsync failure was fatal because the application had already
surrendered custody of the dirty data to the kernel page cache, which discarded it; there was
nothing left to rewrite. This crate controls when a buffer is returned. If it retains a write's
buffer until that write's **epoch is durable** (rather than until the write's completion is
popped), the data at risk after a flush failure is still in hand, and the failure becomes
**recoverable**.

1. **Two failure kinds, each with a precise scope.**
   - *Operation failure* (error, disk full, short write): the scope is known -- that operation's
     range. The rest of its epoch is unaffected once its flush succeeds.
   - *Flush failure*: the scope is uncertain -- everything written since the last successful
     flush may be lost. The ring tracks that set exactly; call it the **suspect set**.
2. **The suspect set's scope is a flush equivalence class, not a file.** A failed device cache
   flush may lose other files' data in the same cache, and a later successful flush of one of
   them would succeed over data that is gone -- the 2018 failure across files. Default scope:
   the whole epoch ring (conservative, and exactly known). A caller-declared flush regime
   narrows it. This makes the `M33+.5` "declare a flush regime" musing load-bearing rather than
   a nicety, and supports declaring over discovering, since the class cannot be soundly derived
   from a device number.
3. **Epochs gain recovery states; the watermark stays monotonic.**
   `Open -> Committing -> Durable`, or `Committing -> Failed(suspect set) -> Recovering ->
   Durable`. `durable_through` stalls at a failed epoch during recovery and then advances: a
   hole is healed, never skipped. Release-after-durable needs no new concept; gated operations
   wait longer, and a WAL prefix stays a prefix.
4. **The ring supplies recovery mechanism; the engine decides.** A failed epoch is reported in
   the completion stream with its suspect set or failed operation, and the engine chooses:
   *rewrite in place* (the ring resubmits retained writes and re-commits), *relocate* (the engine
   rewrites elsewhere and updates its own metadata), or *abandon* (an explicit, precise loss
   event). Fail-stop survives as one engine choice, not a substrate-imposed outcome. Whether the
   crate also offers a default retry policy is deferred.
5. **Retention is a per-write choice.** Retaining until durable lengthens buffer lifetime from
   completion to epoch durability, which costs arena slots under load. A write that is not
   retained cannot be rewritten after a flush failure, and its loss is reported precisely
   instead. Both are exposed (OPTION INTEGRITY).

**Uncertain, and what would settle it.**

- That a rewrite plus a successful flush re-establishes durability after a failed flush, for an
  honest device. Believed, not measured; this is the spike to build
  ([D-44](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-44)), now aimed at *recovery* as well
  as detection. Injecting flush failure on demand is the hard part (a detached VHD or an
  error-injecting filter are the candidates).
- Buffered handles: retention makes them recoverable in principle, but whether a lazy-write
  failure is ever reported to the ring is unknown. If it can be silent, buffered handles cannot
  carry this guarantee and the contract must say so.

### Scenarios: why writes must be tracked after their completion is popped

Requested by the engineer: ordinary error sequences showing why writes whose completion has been
popped -- which `windows-ioring-sys` correctly forgets at that point -- must still be tracked.
**Scenarios accumulate: a new one is added beside the earlier ones and never replaces them**
(the engineer's instruction, 2026-10-05).

#### Scenario 1 -- a virtualised guest over a host page cache

The first construction. The engineer then asked for something less exotic, which produced
Scenario 2; this one is kept because it shows the loss happening outright rather than
possibly.

Setup: a Windows guest running a storage engine, its virtual disk backed by a host page cache
(for example a KVM host with `cache=writeback` over NFS or iSCSI), so a guest flush reaches the
host as an `fsync` of the image. Epoch 7 writes pages A, B, C; epoch 8 writes D, E.

| # | Event | What `windows-ioring-sys` knows | Where A, B, C are |
|---|---|---|---|
| 1 | push writes A, B, C (epoch 7) | three in flight | -- |
| 2 | completions arrive successful and are popped | **nothing**: in-flight set empty, buffers returned and reused | host page cache, dirty |
| 3 | push epoch 7's covering flush | one flush in flight | host begins writeback |
| 4 | transient network fault; host writeback fails | -- | host marks pages clean after the failed writeback (the 2018 fsync failure, on the host) |
| 5 | epoch 7's flush completes with an I/O error | flush failed | nowhere |
| 6 | network recovers; push D, E and epoch 8's covering flush | three in flight | -- |
| 7 | all succeed | nothing | D, E on media |
| 8 | the sample's logic: `durable_through = 8`, `is_durable(7)` true | -- | **A, B, C are not durable and never will be** |

No crash and no power loss: reading A after step 8 returns stale contents from media. How real
this is depends on the host's configuration -- a write-through host, or one that retains and
re-reports the error, would not lose the data -- but the guest cannot tell which kind of host
it is on, which is the point.

**Clarified on the engineer's question ("you are describing a host fault?").** Two parts, and
only one is a fault:

- **The trigger is an infrastructure fault** -- the transient network or storage-path failure
  between the host and its backing store (step 4). Ordinary, external to both guest and host
  software.
- **The loss is host software working as designed, not a host fault.** Marking pages clean after
  a failed writeback and reporting the error once is the host kernel's deliberate error
  semantics (the behaviour PostgreSQL met in 2018), not a bug.
- **No layer lied.** The host *reported* the failure, and the guest saw it at step 5. This is
  therefore not the contract's "device dishonours the flush" assumption. The loss is entirely in
  the guest's inference at step 8 -- that a later successful flush repairs an earlier failed one.
  That inference is the thing dioring exists to get right, which is what makes this scenario
  useful despite being exotic.

**What a failed flush puts at risk** (on the engineer's question "a failed flush means that all
writes may have to be repeated?"). Not all writes -- the **suspect window**, and within it,
*must* rather than *may*:

- **Bounded below by the last successful flush.** Writes covered by an earlier successful flush
  are on stable media; a later failure does not reach back to them (an honest device assumed, as
  everywhere).
- **Superseded -- this upper bound was later found unsound; see "What overlapping means" below.**
  **Bounded above by the failed flush's completion, not its push.** The guest cannot know when the
  host's writeback failed relative to its own writes, only when the failure was reported. So every
  write *completed* before the failed flush completed is suspect -- including writes pushed
  *after* the flush, which [D-47](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-47-detail) lets
  complete first. A write completing after the failure was reported is covered by the next
  successful flush in the ordinary way.
- **Across the flush regime, defaulting to the whole ring.** In Scenario 1 the guest's whole
  virtual disk is one host file, so every guest file on it shares the regime -- the conservative
  default is exactly right there.
- **"Must", for a guarantee.** The guest cannot tell which suspect writes were lost, so to
  re-establish durability it treats all of them as lost: rewrite them (retained), regenerate them
  (engine), or declare them lost (abandon). "May" describes the physical outcome; the obligation
  is "must".

**A replay hazard this exposes (open).** Rewriting a suspect write is idempotent only if nothing
newer has since written the same range. If a later write (say epoch 8's D) covers the same offset
as suspect A, replaying A **clobbers newer data**. So the suspect set must be resolved by range
with newest-wins before replay. That needs an order between overlapping writes -- and the ring
gives none within an epoch, and under D-47 none across epochs either unless the engine waited for
the earlier epoch's commit before issuing the later write (which the sample did, and a pipelined
engine will not). Who owns overlap ordering -- dioring refusing overlapping writes in the unflushed
window, dioring recording submission order, or the engine -- is not yet discussed. It also
questions the sample contract's "ordering across an epoch boundary is guaranteed", which holds
there only because the sample serialises epochs.

**Is any of this documented? (the engineer: "this seems seismic").** Checked during the session:

- **Linux: yes, and it was seismic there.** PostgreSQL assumed for about two decades that retrying
  `fsync` after a failure was safe; the 2018 discovery ("fsyncgate") changed PostgreSQL to PANIC
  on `fsync` failure. The systematic study is Rebello et al., *Can Applications Recover from fsync
  Failures?*, USENIX ATC 2020 ([paper](https://www.usenix.org/system/files/atc20-rebello.pdf)):
  ext4, XFS and Btrfs **all mark the in-memory pages clean after an fsync failure**, so a retried
  fsync does not rewrite them, and the applications studied (including PostgreSQL, LMDB, LevelDB,
  SQLite and Redis) mishandled it in various ways.
- **Windows: not documented either way.**
  [FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)
  documents only success and `GetLastError` on failure -- nothing about whether dirty data
  survives a failed flush, or whether a later flush's success says anything about an earlier
  failure. The "Delayed Write Failed ... the data has been lost" notification is the cache manager
  reporting a lazy-write loss, which is evidence the cache *can* discard dirty data, but it is not
  a specification of flush semantics. A second search for Windows-specific documentation timed out
  and was not retried; treat this bullet as "nothing found", not "confirmed absent".
- **Consequence:** dioring cannot cite Windows documentation for its suspect-window rule, and
  must not bind to observed behaviour either (PLATFORM INTEGRITY rule 2). The rule is justified as
  the only one that is correct under every behaviour the platform is *permitted* to have -- the
  same posture as [RESPONSE-SPACE.md](../crates/windows-ioring-sys/RESPONSE-SPACE.md) -- and the
  spike already proposed is what would show which behaviour Windows actually has.
- **The replay overlap hazard is not new.** Storage stacks generally give no ordering between
  concurrently outstanding overlapping writes, and journalling designs handle it explicitly. What
  is specific here is that D-47 removes the ordering a reader might assume the epoch barrier
  provides.

#### Scenario 2 -- a USB drive knocked loose (the mundane case)

Requested as the most ordinary configuration and failure available: non-exotic hardware,
configuration, and failure.

**Configuration.** A Windows 11 laptop, default settings. A storage engine keeps its data file
on an ordinary external USB SSD, NTFS, default removal policy ("Quick removal"). The file is
opened `NO_BUFFERING | OVERLAPPED` and preallocated, exactly as the sample does. One ring;
epochs committed by a covering flush as in `commit.rs`.

**Failure.** The USB cable is knocked loose; the user plugs it back in; the program keeps running.

| # | Event | What `windows-ioring-sys` holds | Where the data is |
|---|---|---|---|
| 1 | epoch 40 commits; `durable_through = 40` | -- | epochs <= 40 on media |
| 2 | the engine splits a B-tree node: writes parent A and children B, C, all in epoch 41 | three in flight | -- |
| 3 | all three complete successfully and are popped | **nothing**: in-flight set empty, buffers back in the engine's pool and reused | accepted by the bridge/drive; on media or in a volatile cache |
| 4 | push epoch 41's covering flush | one flush in flight | -- |
| 5 | cable knocked out: surprise removal, the flush fails with a device error, the handle is dead | flush failed; nothing else | the drive lost power; A, B, C are each independently on media or not |
| 6 | ("Quick removal" asks Windows to disable the device write cache; whether a given bridge or drive honours that is device-specific, and the host cannot find out) | -- | unknown and unknowable |
| 7 | drive re-inserted, volume mounts; the engine reopens `data.db` and continues the epoch sequence: epoch 42 writes D, E and its flush succeeds | -- | D, E on media |
| 8 | `commit.rs` logic: `durable_through = 42`, so `is_durable(41)` is true | -- | if B landed and A, C did not, the B-tree on disk is corrupt |
| 9 | at its next checkpoint the engine trims its WAL to the durable watermark, past epoch 41 | -- | **the only copy that could repair the split is deleted** |

Step 9 is the irreversible one. Through step 8 the WAL still holds the split and crash recovery
could repair it; the false watermark is what licenses discarding that copy.

**What step 5 needed, and when it was lost.** Which writes since the last durable epoch may not
be on media (A, B, C by file and offset), and -- to heal without crash recovery -- their
contents. Both were dropped at step 3, correctly for `windows-ioring-sys`, since each operation
was over. But step 3 is exactly where the exposure begins: a completion means the device took
the bytes, not that they will survive.

**The same sequence under round 2.** At step 5 epoch 41 goes `Failed` with suspect set
{A, B, C} and `durable_through` stays at 40. At step 7 the engine chooses to rewrite (A, B, C
retained) or to regenerate from its WAL (not retained); either way epoch 41 is re-committed,
becomes durable, and the watermark advances to 41 and then 42. At step 9 the WAL trim cannot pass
41 until 41 is really durable.

**A detail this settles (working position): a failed epoch survives the file being reopened.**
The watermark is the engine's trust boundary, and a new handle to the same file does not reset
it.

#### Scenario 2, analysed: how the database behaves with and without dioring

Asked by the engineer ("dioring" is the engineer's short form of durable-ioring). Three ways
to write the engine, all running Scenario 2:

- **A. Without dioring, retrying the flush** (the sample's `commit.rs` logic; PostgreSQL before
  2018). The error is logged and a later flush is allowed to cover it. The watermark passes 41,
  the WAL is trimmed past 41, and if A or C was lost the B-tree is permanently and silently
  corrupt -- found later as an inconsistent index or a checksum failure, after the evidence is
  gone.
- **B. Without dioring, fail-stop** (PostgreSQL after 2018, `data_sync_retry = off`). The flush
  error at step 5 kills the process and drops every connection. Clients whose commit records were
  in epoch 41's WAL were never acknowledged, so **their commit outcome is unknown to them**. After
  the drive returns, a restart runs crash recovery from the last checkpoint and replays the split.
  Correct and lossless; the cost is an outage (restart plus redo since checkpoint), dropped
  connections, and uncertain outcomes for in-flight committers.
- **C. With dioring.** Epoch 41 goes `Failed({A, B, C})`, the watermark holds at 40, and the
  process stays up. Transactions committing in epoch 41 **simply wait**: their acknowledgement is
  gated on 41 becoming durable. After the reopen the engine rewrites A, B, C from retained buffers,
  or redoes *only those three pages* from WAL; 41 re-commits, the waiters are acknowledged, and
  work continues. If the drive never returns, the engine abandons -- reaching B's end state by
  choice and with a precise loss report.

**What the comparison establishes (assistant's reading, not challenged):**

1. dioring is **not more correct than B**; a careful engine avoids the corruption by fail-stop.
   dioring makes the correct behaviour the *default*, where the natural spelling (A) is the bug
   PostgreSQL shipped for about two decades.
2. Over B it wins on **availability and precision**: no restart, and recovery of three pages
   rather than of everything since the last checkpoint.
3. It changes the **client-visible outcome** from "connection lost, outcome unknown" to "commit
   acknowledged later". For a database this is arguably the largest difference.
4. An engine *could* build C itself -- direct I/O plus a buffer pool that keeps
   written-but-unflushed pages dirty is the same bookkeeping, and some engines do. dioring's
   claim is to supply that discipline once, correctly, as a reusable layer, which is C-3's
   rationale for the crate.

**Design point surfaced (open): file identity must outlive handles.** In C the handle dies at
step 5 and the engine reopens at step 7. For the suspect set to stay meaningful it must name a
**logical file** that a new handle can be rebound to, not a handle. The earlier working position
takes files as `SharedFile`, which *is* a handle -- so dioring needs a handle-independent file
identity and a rebind operation after reopen. Load-bearing; must be designed in.

**Scoping that followed (the engineer):**

**Withdrawn -- the assistant misread the remark; the correction and the restored positions follow
this block.**

> i suspect that anything with a log on media with surprise removal is a poor fit, but that is
> not our concern

Recorded consequences (working positions):

- **Removable media is not a target workload.** Scenario 2 was chosen for mundanity, not as a
  use case. The failure shape it exhibits -- a flush that fails followed by a later flush that
  succeeds over data that may be gone -- also arises on fixed storage (transient device errors
  across a controller reset, SAN path failover, thin-provisioned exhaustion during writeback,
  Scenario 1's host cache), so the conclusions above do not depend on supporting removable media.
- **Out of scope for recovery quality, not for correctness.** On such media dioring may only ever
  abandon, but it must still never over-report: a poor fit must behave correctly, not gracefully.
- **The handle-independent file identity above is downgraded** from load-bearing to open: it was
  forced specifically by handle death, which without removable media is confined to rarer cases
  (forced dismount, a locked BitLocker volume, a failed SMB reconnect). Minimum requirement: suspect
  sets stay correct for the life of the handle. Rebind becomes a later feature if one of those
  cases proves to matter. Whether network shares are in scope at all is not yet discussed.

**Correction (the engineer):**

> no, i actually meant "not our concern" in the opposite direction. it is not our place to judge
> how people use our platform

The remark was a statement of the platform's posture, not a scoping-out: the level-platform rule
and [D-no-client-prescriptions](../crates/windows-platform-probes/DESIGN-NOTES.md#d-no-client-prescriptions)
applied to storage choice. Corrected working positions, replacing the withdrawn block above:

- **Removable media is in scope** like any other storage a consumer chooses. Whether a log
  belongs there is the consumer's judgement; dioring's documentation does not call it a poor fit,
  and at most states what was observed on such media.
- **Recovery quality there matters as much as anywhere**, not merely correctness.
- **Handle-independent file identity is restored to load-bearing.** Surprise removal is a
  supported case and kills the handle, so a suspect set must survive the reopen and rebinding to
  a new handle is designed in from the start.
- By the same principle network shares are presumably in scope, which makes handle death more
  common and rebind more necessary. Not yet discussed.

**Supported, but not a design driver (the engineer):**

> i do think it is not a good example for us to design around though

Both hold at once: removable media is supported without judgement, and Scenario 2 does not get
to shape the design. That reweights what rests on it:

- **Not dependent on Scenario 2** (each also holds under Scenario 1 and the fixed-storage
  failures): tracking every write by identity until durable, the watermark stalling at a failed
  epoch, per-write retention, and the engine's rewrite / relocate / abandon choice. These stand.
- **Dependent only on Scenario 2: handle-independent file identity with rebind.** Moved from
  load-bearing to **open, with a non-foreclosure constraint**: the design must not weld file
  identity to the handle in a way that would prevent a suspect set outliving it, but rebind's
  shape is not derived from this scenario. (This supersedes the "restored to load-bearing" bullet
  in the correction above.)
- **Gap identified:** the design lacks a fixed-storage scenario in which the flush fails
  transiently and the handle survives -- the case dioring's recovery path is chiefly for.
  Candidates offered: an internal NVMe controller reset (System event 129 from `stornvme`), or an
  iSCSI/SAN path flap under MPIO failover. Not yet drafted; when it is, it must say which steps
  are documented behaviour and which are assumptions (class-driver flush retry after a reset, and
  whether the device's volatile cache survives one, are both stack-dependent).

#### What the scenarios establish

Identity and contents differ sharply in cost. **Identity and range** (file, offset, length,
epoch) is a few words per write and is enough to report the loss precisely -- and a database can
usually regenerate the pages from its WAL, so for the primary consumer identity may suffice.
**Contents** cost a buffer per write held until durable, and are what self-healing rewrite needs.

**Refinement of round 2, point 5 (working position):** *every* write is tracked by identity
until its epoch is durable, unconditionally; **data retention** is the per-write option layered
on top. A non-retained write still appears in the suspect set by range; it simply cannot be
rewritten by the ring.

**Where the gap is.** `windows-ioring-sys` forgetting at the pop is correct: it owns what one
operation means ([D-54](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-54)), and the operation
is over. What is not over is the write's **membership in an epoch**, which ends at durability.
That second lifetime -- completion to durability -- is the thing this crate exists to own.

**Consequence for the sample.** `commit.rs`'s "a failed commit is not permanent" is half right
under this model: a later flush heals the hole only if the suspect data is rewritten first.
Without the rewrite it is wrong. To be queued once this crate's contract is written.

## Two consumer classes, and retention moves up a layer (round 3)

The engineer's framing:

> First I think that there will be two general kinds of consumers for dioring. The first will be
> naive. They want to use ioring and they want durability. neither the native api nor
> windows-ioring-sys solve their problems, so the dioring can help them out.
>
> Second will be developers that understand logs and durability constraints and the issues here.
>
> The second class do not want us to implement some arbitrary buffering of writes until
> durability has been seen to succeed, agreed? They probably have their own logs and can either do
> their own replays or otherwise deal with things - our job would at most be to clearly delineate
> the inventory of writes which were not known to be flushed, and even that may not be required of
> them. If a database is maintaining a sequential log and then flushes, it can rewrite the log
> pages.
>
> I am wondering if the first class gain *some* utility out of dioring and we can build yet
> another layer on top in a future crate to assist them further with the kind of buffering of
> writes until a safe flush has been observed.

**Agreed (assistant): retention is policy, so it leaves dioring.** This supersedes round 2's
"retention is a per-write choice" (point 5) and the refinement that layered retention on top of
identity tracking *inside* dioring. Retention, automatic rewrite, and newest-wins replay belong
to a future crate above dioring. That also dissolves the arena-slot pressure retention would
have put on registered buffers. The future crate is a deliberate layer, not a deferral for want
of a consumer, and must be queued as work when checklists are written so it is not lost.

**What stays in dioring, for both classes (working position):**

1. **The truthful watermark.** It never passes a failed epoch. Not optional: this is the step-8
   defect in Scenarios 1 and 2, and an expert needs it as much as anyone.
2. **A resolution protocol, policy-free.** A stalled watermark must be releasable by the consumer:
   the failed epoch is either *healed* (the consumer has re-issued what it needed in a later
   epoch, and that epoch is durable) or *abandoned* (declared lost). dioring supplies the
   transition and enforces that healing requires a later durable epoch; the consumer decides
   which. The future retention crate is then a client of this protocol rather than a
   reimplementation of it.
3. **The inventory of the suspect window**, as mechanism -- a fact about what was issued, not a
   policy about what to do. Kept in dioring, against the engineer's "even that may not be
   required", for two reasons put to the engineer:
   - *Experts need it more than the framing suggests.* A sequential log can rewrite its own log
     pages, but a database's background writer or checkpointer also wrote data pages in the same
     window and same flush regime, and those are equally suspect. Knowing them is exactly what
     PostgreSQL lacked in 2018. An engine that keeps its own complete record can ignore the
     inventory; one that does not, needs it.
   - *The upper crate needs it to compose.* Without it, the retention crate would have to
     intercept and re-track every write dioring already tracks -- a second copy of one fact.
   - The inventory therefore carries a **submission sequence** per write, so the upper crate can
     resolve overlapping suspect writes newest-wins. That moves the replay overlap hazard into the
     layer that replays, while dioring supplies the order it needs.
4. **What the naive class gets from dioring alone:** truthful epoch durability, failure detection,
   and a precise loss report. Without retention, a failure for them amounts to fail-stop with a
   precise error -- correct, if not graceful. The retention crate is what makes it graceful.

Whether the inventory should be opt-out for experts who will never read it (it costs a few words
per write until durable) is not yet discussed; the assistant's lean is to keep it unconditional
until a measurement says the cost matters, since the per-epoch failure aggregation already has to
track every write until its epoch resolves.

## The real consumer: user-mode filesystems

The engineer, on round 3:

> I feel kind of settled now actually how does this fit with your concerns? I think that this puts
> dioring into a good position for what I really need which is a ring shape on which to build
> filesystems, literally. I have a series of user mode filesystem implementations (ext4, fat16,
> fat32, ntfs, etc) that are based on windows ioring and have hit this durability problem.

The assistant's reconciliation of its earlier concerns against that consumer:

- **Failure semantics: settled by round 3**, and the filesystem set spans both consumer classes.
  ext4 (jbd2) and NTFS (LFS) are journalled: expert class, able to heal from their own logs.
  **FAT16/FAT32 have no log**: after a flush failure they cannot regenerate suspect writes, so on
  failure they are the naive class -- they need either their own retained copies (a FAT driver
  usually caches the FAT and directory sectors anyway) or the future retention crate.
- **The truthful watermark maps onto journal-tail advance.** jbd2 moves its journal tail once
  checkpointed metadata is durable at its home location; NTFS advances its restart area likewise.
  Moving that tail on a false watermark is exactly Scenario 2's step 9 (WAL trim) in filesystem
  form. The concern is confirmed, not new.
- **Release-after-durable is core, not an extra, for all three designs.** ext4 commits a
  transaction's commit block only after its journal blocks (and, in `data=ordered`, its data
  blocks) are durable; NTFS obeys the WAL rule by LSN; FAT's crash consistency is careful write
  ordering (data, then FAT, then directory entry), which on a device with a volatile cache needs a
  durability point between steps. All three are "issue X only after Y is durable".
- **Overlapping writes become central, not an edge case.** A filesystem rewrites the same sectors
  constantly -- FAT sectors, bitmaps, inode tables, the superblock -- so overlapping writes in the
  unflushed window are the norm. Two concerns follow, the first independent of any failure: (a)
  two overlapping writes in flight at once land in unspecified order even with no failure, so
  something must serialise writes per block; (b) replay after a failure needs newest-wins
  resolution, which round 3's per-write submission sequence supplies. Who serialises in-flight
  overlap -- the filesystem's buffer cache, or dioring by refusing an overlapping write while an
  earlier one to the same range is in flight -- is open, and now load-bearing.
- **Multi-file epochs probably shrink.** A filesystem instance usually targets one block device
  or one image file, so one flush target per instance. Multi-file epochs remain for databases;
  whether they are needed for the filesystem case depends on the targets (asked, below).
- **Removable media returns as a question, for the engineer to weigh.** FAT's natural habitat is
  removable media, so Scenario 2 is FAT's ordinary operating environment. The engineer ruled it
  out as a design driver; the assistant flags that the FAT implementations may reopen that, and
  with it handle rebind, without presuming the answer.
- **A filesystem on an image file is a two-level stack**, like Scenario 1: the user-mode
  filesystem above, host NTFS below. Whether the image is opened `NO_BUFFERING`, and what the
  host does on a failed flush, decide how much of Scenario 1 applies.
- **Unaffected and still open:** completion routing (Model A vs B), alternating rings and the
  epoch-ring unit, whether `Epoch` is public vocabulary (a journal's transaction ID or an LSN is a
  natural client-side analogue), the `M33+.5` amendment, and the Windows flush-failure spike.

Asked of the engineer, not yet answered: what the filesystems target (raw volumes, partitions,
or image files on NTFS, and opened with which flags), and where those implementations live.

### The stack (the engineer's answer)

> well they actually sit on a driver (also ioring based) on vhdx which also sits on ioring

So: filesystem (ext4 / FAT / NTFS) -> block driver (IoRing-based) -> VHDX implementation
(IoRing-based) -> the VHDX file on the host. The assistant's reading, as directional positions
(not decisions):

- **dioring is a contract that layers both consume and provide, not only a type.** Every layer
  in the stack presents a ring shape upward and consumes one downward. If the filesystem is to sit
  uniformly on "dioring over a file" or "block driver over VHDX over dioring", the durability
  contract (epochs or watermark, release-after-durable, failure plus inventory, the resolution
  protocol) must be something a layer can *implement*, with dioring-over-`windows-ioring-sys` as
  one implementation. This is the strongest consequence of the stack and is unexamined.
- **Scenario 1's host is now our own layer.** Its fault was a lower layer reporting a failure once
  and then letting a later success imply repair. In this stack the VHDX layer and the block driver
  are ours, so their failure semantics are *ours to define*: each must hold its watermark at a
  failed epoch and report upward, never let a later success paper over an earlier failure. Only
  the bottom (host filesystem, Windows, the device) is outside our control.
- **Failure and inventory must translate upward.** A failed epoch at the VHDX layer's target is
  reported in host-file offsets; the layer above needs it in virtual-disk LBAs, and the
  filesystem in its own blocks. Each layer maps the inventory into its own terms as it
  propagates. Composition, not pass-through.
- **The VHDX layer is itself an expert-class consumer.** VHDX keeps its own metadata log, and a
  write to an unallocated block of a dynamic disk needs the payload durable before the BAT update
  that points at it -- release-after-durable inside the middle of the stack. So a top-level
  commit may cost more than one sequential flush at the bottom. That cost compounds by layer, is
  workload-dependent, and is measured rather than argued (OPTION INTEGRITY).
- **The contract must not bake in D-47.** The one-sided barrier is a property of the *Windows*
  ring. A layer implemented in software may offer stronger ordering. The contract states a
  minimum; an implementation may exceed it.
- **Overlapping writes recur at every layer**, and are densest in the middle (BAT and log sectors
  in VHDX).

Asked of the engineer, not yet answered: are the block driver and VHDX layers user-mode and in
one process; does "IoRing-based" mean each owns a Windows `IoRing`, or that each presents a
ring-shaped API with only the bottom on Windows' ring; how the VHDX file is opened (flags); and
where these components live.

**Answered in part:** the filesystem, driver and VHDX implementations are not available to this
repository. The design therefore proceeds against the stack as the engineer describes it, and
any characterisation of ext4, NTFS, FAT or VHDX behaviour in this file is the assistant's general
understanding of those formats, not a description of the engineer's implementations. The other
three questions remain open, answerable at whatever level of abstraction the engineer can share.

### Open source for everyone, with an internal motivating consumer

> I'm implementing this set of components as open source for everyone but really I want to use
> them for my internal work

Consequences drawn by the assistant (working positions):

- **The internal stack validates the design; it does not specify it.** dioring is a public
  platform crate, so every decision recorded here must be justifiable in public terms. That is a
  useful discipline as well as a confidentiality requirement: a rationale that only holds given
  undisclosed internals is one no public consumer can evaluate.
- **The public crate is not narrowed to the internal need** (PRIME DIRECTIVE, PLATFORM INTEGRITY
  rule 3). In particular "multi-file epochs probably shrink" above applies to the filesystem
  case only; multi-file epochs stay in scope for public consumers such as databases.
- **"A contract layers implement" gains a second reason.** The internal layers would implement
  the contract privately; the public needs at least one implementation beyond
  dioring-over-`windows-ioring-sys` to show the contract is genuinely implementable. The natural
  candidate is a **fault-injecting implementation**: deterministic failed flushes, short writes
  and failed epochs on demand. That also answers the earlier problem that flush failure cannot be
  induced on a healthy machine -- for *consumers'* tests, not only dioring's own -- in the same
  spirit as `windows-ioring-sys`' response-space resolver
  ([RESPONSE-SPACE.md](../crates/windows-ioring-sys/RESPONSE-SPACE.md)). It does not replace the
  spike, which is about what real Windows does.

### Cross-platform today, a ring-based research OS later

> Also I said that they are over IoRing but actually they currently in Rust over a trait that is
> an abstraction over both IoRing and iouring so that they work on both Windows and Linux so I
> will be applying the same discipline. However the intent is also for the FSes to be the basis
> for kernel mode filesystems in a research operating system I'm working on also where the entire
> IO system is ring based, so this durability system is educational about how I want to approach
> structuring the storage and cache there.

The assistant's reading (directional; none of it decided):

- **The contract-as-trait position is reinforced, and the contract should be platform-neutral.**
  The consumers already sit on a trait over two platforms, so dioring's contract should name no
  Windows type. This workspace is Windows-only by rule (every item `cfg(windows)`, CI on Windows
  only), so *where* a platform-neutral contract lives -- here, or beside the engineer's existing
  trait -- is a scope decision for the engineer, not one to take in passing.
- **The contract's minimum is the weaker platform; implementations may exceed it.** This
  generalises the earlier "do not bake in D-47". Windows' drain flag is one-sided (measured,
  D-47). io_uring's `IOSQE_IO_DRAIN` is documented as two-sided -- the drained request does not
  start until earlier ones complete, *and* later ones do not start until it completes -- and
  io_uring also has `IOSQE_IO_LINK` chaining and a per-write sync flag (`RWF_DSYNC`, which can
  become FUA where the device supports it), none of which Windows' ring has. The contract states
  what every backend can honour; a backend that can do better says so.
- **On Linux the suspect-window rule is documented necessity, not conservatism.** The 2020 study
  above establishes that ext4, XFS and Btrfs mark pages clean after a failed fsync. On Windows the
  same rule is justified only by the permitted response space. Same rule, different evidence.
- **In the research OS, the retention layer is the page cache -- and the lesson is a cache
  design decision.** The layering reached in round 3 maps onto a kernel directly: the block layer
  provides truthful completion and failure with an inventory; the page cache is the retention
  layer; the filesystem resolves (heal or abandon). Linux's fsyncgate behaviour is the page cache
  choosing to mark pages clean on failed writeback; the structure here argues for the opposite --
  keep them dirty and surface the failure to the filesystem.
- **But Linux's choice was not careless, and the reason must be carried.** Keeping dirty pages
  across a failure on a device that never returns (a yanked USB stick) pins memory without bound;
  marking them clean is partly an out-of-memory defence. So the *abandon* path is what bounds
  retention, and someone with authority must be able to take it. In dioring that is the consumer
  (and the mechanism invents no timeout, D-67); in a retention layer or a kernel page cache, a
  bound on retained-but-unresolvable data is policy that layer must own. This is the one place
  the research OS faces a trade-off dioring can defer.

**Settled by the engineer: dioring is strictly Windows-only.**

> I tell you these things to solicit opinions but this implementation is still strictly Windows
> only

So the platform-neutral-contract question above is closed: dioring lives in this workspace under
its Windows-only rule, with no cross-platform code and no portability layer. The cross-platform
and research-OS material stays in this record as context for the engineer's other work, not as
requirements. What survives of it in dioring (assistant's view) costs nothing and adds no
platform: write the contract's semantics in terms of durability itself rather than in terms of
Windows' incidental behaviour -- D-47's one-sidedness is the floor dioring's implementation must
cope with, not a property the contract should promise to consumers.

## Next design steps (proposed by the assistant on request, 2026-10-05)

Ordered by dependency. Not yet agreed; the engineer sets the agenda.

1. **Give the crate a home and record what is settled.** `crates/durable-ioring/` with its own
   DESIGN-NOTES.md (Tier 1) and CHECKLIST.md / PLANS.md, plus a root PLANS.md row. Promote the
   settled items (name, no unsafe, Windows-only, the layering of round 3) to decisions; amend
   `M33+.5` in [CHECKLIST-io-domains.md](../CHECKLIST-io-domains.md) to the new shape; queue the
   implied work so it is not orphaned -- the sample's `commit.rs` correction (ioring checklist),
   the retention crate as a parked milestone, and the flush-failure spike.
2. **Settle the three questions the contract cannot be written without.**
   - *Overlapping writes:* who serialises overlapping in-flight writes -- the consumer, or dioring
     refusing an overlap while an earlier write to the range is in flight. Load-bearing for the
     filesystem consumer, and it decides whether dioring tracks in-flight ranges (a data-structure
     cost) or only per-write identity.
   - *Public vocabulary:* `Epoch` or an opaque ordered durability point; how a push reports the
     point that covers it; how release-after-durable names what it waits for.
   - *Resolution protocol detail:* what "healed" requires (dioring cannot know what the consumer
     considers sufficient, so heal is a consumer assertion gated on a later durable epoch), what
     "abandoned" does to gated operations, and how both appear in the completion stream.
3. **Write the contract, prose first**, as the sample's `contract.rs` was: guarantees, non-
   guarantees, requirements of the handle, assumptions, the suspect-window bounds, failure states,
   and release-after-durable -- in terms of durability, with D-47 as an implementation floor
   rather than a promise.
4. **Shape the API over `windows-ioring-sys`.** The completion-stream type (operation completion,
   epoch durable, epoch failed); composition with `IoRing<T, X>` and dioring's own sidecar;
   registered buffers; Model A versus Model B routing; whether the contract is a trait from the
   start or a concrete type until a second implementation (the fault-injecting one) exists to
   justify extracting it.
5. **Plan the first implementations as milestones:** dioring over `windows-ioring-sys`, and the
   fault-injecting implementation for consumer tests.

In parallel, not blocking: the Windows flush-failure spike (D-44), which tells us what Windows
actually does but is not needed to write the contract, since the contract is justified on the
permitted response space.

Deliberately left coarse (RESOLUTION GRADIENT): multi-file commit strategies and their
measurement, alternating rings, caller-declared flush regimes, and file rebind beyond its
non-foreclosure constraint.

## The engineer's answers to step 2, and step 1 carried out

> 1: please just do this
>
> 2: my thoughts on the questions:
>
> Overlapping writes: not our problem I would say, only an identity per write.
>
> Public vocabulary: I prefer opaque, does it need to be ordered? The implementation knows, but
> otherwise don't they just behave as groupings?
>
> Healed: like you say, we don't know. Can we put this in the control of the consumer?

And, immediately after: "note that when I say 'can we ...' this is a question not a directive".
So the second and third answers contain **questions**, recorded as open items rather than
decisions.

- **Overlapping writes -- decided:** the consumer's responsibility; an identity per write only.
  Recorded as [DI-D-6](../crates/durable-ioring/DESIGN-NOTES.md#di-d-6).
- **Opaque -- decided; ordered -- a question.** Recorded as
  [DI-D-8](../crates/durable-ioring/DESIGN-NOTES.md#di-d-8) (opaque) and `DI-1.1` (ordering). The
  assistant's answer: an order is not needed for correctness if dioring reports each point's
  outcome, but points are **not independent** -- a write of the point after a failed one can
  complete inside the failed point's suspect window, so a later point's successful flush does not
  make it durable; durability is prefix-closed, and the contract must say so whatever the type
  exposes. Consumers usually know their own creation order, so comparability can be decided at the
  API step; comparing points from two instances should yield no answer, which would put C-3's
  per-domain constraint in the type.
- **Consumer-controlled healing -- a question.** Recorded as `DI-1.2`. The assistant's answer: yes,
  with one mechanism rule kept in dioring -- a resolution takes effect at the next durable point
  opened after it is declared, so re-issued writes are covered by a later successful flush and
  dioring never judges sufficiency. Open: the fate of operations gated on an abandoned point, the
  completion-stream representation, and the unbounded stall when a consumer never resolves (no
  timeout in mechanism, [D-67](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-67)).

**Step 1, as carried out:** [crates/durable-ioring/](../crates/durable-ioring/COMPONENT.md) created
with COMPONENT.md, DESIGN-NOTES.md (`DI-D-1`..`DI-D-8`), CHECKLIST.md and PLANS.md; a root PLANS.md
row; `M33+.5` marked transferred in [CHECKLIST-io-domains.md](../CHECKLIST-io-domains.md); the
sample's `commit.rs` defect queued as `M29.1` in
[windows-ioring-sys' CHECKLIST.md](../crates/windows-ioring-sys/CHECKLIST.md); and a forward pointer
added to [D-54](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-54). **One conflict surfaced while
doing it:** the workspace decision that new crates take the `win-` prefix
([DESIGN-NOTES.md](../DESIGN-NOTES.md#new-crates-take-the-win-prefix)) predates and arguably covers
this crate's name. Recorded as an open tension in `DI-D-2` and queued as `DI-1.3` for the engineer;
the directory was created under the chosen name because it is unpublished and cheap to rename.

**Resolved by the engineer:**

> The "win-" naming rule applies for crates that provide more-or-less direct layering of safe
> abstractions on the Windows APIs. We can debate whether the windows-ioring-sys abstraction is
> "heavier" than a direct safe abstraction but that is the intent.
>
> Since durable-ioring is a completely new facility, it does not fit the mold. Please update the
> rule to indicate this intent.

The workspace decision was amended in place with a scope marker under its heading
([DESIGN-NOTES.md](../DESIGN-NOTES.md#new-crates-take-the-win-prefix)), given a Tier 2 entry
([DESIGN-RATIONALE.md](../DESIGN-RATIONALE.md#why-the-win-prefix-is-scoped-to-direct-layers)), and
the horizon migration `M-inf.3` in [CHECKLIST.md](../CHECKLIST.md) gained a classification step,
since some existing `windows-*` crates are facilities rather than direct layers. `DI-D-2` now states
the reason the name carries no prefix, and `DI-1.3` is archived.

## Working through DI-1.1: are durability points ordered?

**Disjoint lifetimes need no relationship (engineer's opening; agreed).**

> First of all, clearly time passes. Once a durability point has resolved and another starts, there
> is a relationship there that I hope we don't have to specify, correct?

The assistant agreed, with one statement that does need writing and is not a relationship: **a
point reported durable stays durable.** The suspect window's lower bound is the last successful
flush, so no later failure reaches back to it (an honest device assumed, as everywhere; media
failure is out of scope). That is a property of a single point -- finality -- and it is what makes
"A resolved, then B started" need nothing further: B's writes did not exist while A was at risk,
and A's outcome cannot change afterwards.

**So DI-1.1 narrows to points whose lifetimes overlap** -- B opened while A is unresolved. In a ring
that pipelines its commits (keeps writing while a commit is outstanding) that is the ordinary case,
not an edge, and it is exactly where the prefix property bites: B's writes can complete inside A's
suspect window.

**One relationship DI-1.2 will create regardless.** If a failed A is *healed*, it resolves only when
a point opened after the heal is declared becomes durable -- so a healed A's resolution depends on a
later point by construction. An *abandoned* A resolves immediately and creates no such dependency.

### What "overlapping" means, process lifetime, and other writers

The engineer:

> Well now we have the usual memory ordering problem of "what do you mean by overlapping
> lifetimes?" where storage can be a distributed system. So again, since this is about precision
> and durability, can we characterize these objects and this durability with regards to process
> lifetime?
>
> The durability objects we're talking about only live within a process but their effects are long
> lived. Can we agree that we are fundamentally *not* going to talk about any cross-process
> interactions here? Or is there some way to define this to head off whatever problems there may
> be?
>
> It's common nowadays to have single-writer disciplines for a database but this doesn't have to be
> true. VAX Rdb/VMS maintained a high quality high performance multi-reader multi-writer database.
> I haven't thought through what problems it may have caused for itself with regards to logging and
> durability (I know a little of how it managed its transactions running in this mode) but single
> writer is not the only mode of operation.
>
> I may seem to be operating counter to my interests but I want to be sure not to foreclose a
> needed design concern.

The assistant's answer (working positions, not agreed):

1. **Order is defined by the instance's own observations, never by time.** A ring has one
   submitter and one drainer at a time, so a dioring instance sees a total order of its own events:
   each push, and each completion it pops. "Before", "after" and "overlapping" are defined only over
   that sequence -- no wall clock, no device time, nothing about when anything happened in the
   storage below. B overlaps A if B is opened before A's resolution in that sequence. This is the
   memory-model move: the only happens-before edges claimed are the instance's own program order,
   plus one causal edge -- anything the instance pushes after it has *observed* an event happened
   after that event.
2. **Defining it that way corrects the suspect window's upper bound.** It was stated earlier as
   "every write that *completed* before the failed flush *completed*". That is not sound in
   observation order: a write the lower layer accepted before the failure can have its completion
   posted *after* the flush's failure is posted, so completion order does not bound what the failure
   could have touched. The only edge the instance can rely on is causal: a write **pushed after the
   failure was observed** was accepted after the failure. So: **suspect = every write pushed before
   the failure was observed that is not covered by an earlier successful covering flush.** This
   widens the window to include writes in flight when the failure arrived. It supersedes the earlier
   bound in "What a failed flush puts at risk" above, which is left as written.
3. **Within one process lifetime, durable facts outlive the process; nothing else does.** A point
   reported durable is final, and its effect is the long-lived part. Unresolved points, suspect
   inventories and gated operations die with the process; after a crash the next process knows
   nothing of them, and recovery is the consumer's own (its log, its replay). The contract claims
   nothing across a process boundary except that what it reported durable was durable.
4. **Cross-process interaction cannot simply be declared out of scope, because a flush regime can
   be shared.** Two writers -- two processes, or two instances in one process -- writing the same
   file share a cache and a flush regime. If the lower layer reports a failure only to the writer
   whose flush hit it, the *other* writer's writes become suspect and its instance sees nothing: a
   later successful flush there would report data durable that is gone. That is Scenario 1 across
   writers rather than across layers, and no instance can detect it locally. (Whether Windows
   reports a write-back failure to every handle on a file, or to one, is undocumented; Linux reports
   it once to each open file description. Added to the spike's questions.)
5. **So: do not model cross-process coordination, but state the assumption and keep one mechanism
   open.** The contract states that its guarantees hold only if every failure affecting the
   instance's flush regime is reported to that instance. For consumers where that is not true --
   multi-writer designs -- the resolution protocol gets one cheap addition: **importing an external
   failure**, by which the consumer tells an instance "treat everything pushed before now as
   suspect", so a failure learned through the consumer's own coordination (a lock manager, a shared
   log) can be propagated into each writer's instance. Exporting needs nothing new: a durable point
   is final, so it is a fact a writer can publish. Multi-writer designs coordinate through their own
   protocols; dioring neither provides nor forecloses one.
6. **On Rdb/VMS specifically**, the assistant does not know its logging internals in enough detail
   to say what it relied on, and says so. The general shape of shared-disk multi-writer databases --
   coordination through a distributed lock manager, with ordering between writers established by
   that protocol rather than by the I/O layer -- is what points 4 and 5 are designed to leave room
   for.

### Another writer's failed flush, while this instance is quiet

The engineer:

> I want to explore something you pointed out. Another process may flush and that flush may fail,
> even while we are quiet. The failure of that flush may have impact on writes that we have in
> flight that are not yet durable, correct?

The assistant: yes, and the exposure is wider than "in flight" and wider than "another process".

1. **Which of our writes are exposed: every write accepted below and not yet covered by our own
   successful flush** -- including writes whose completions we have already popped and are idle
   on. "Quiet" changes nothing: the exposure lasts from acceptance until our covering flush
   succeeds, whatever we are doing meanwhile. These are exactly the writes Scenario 2 showed
   `windows-ioring-sys` correctly forgets at the pop.
2. **What shares the risk is whatever shares the cache, and with `NO_BUFFERING` that is the
   device.** A buffered handle shares the system file cache with every other handle on the file. An
   unbuffered handle still shares the device's volatile write cache with every process writing to
   that device, whatever file it writes -- and another process's flush of *its* file is a device
   cache flush. If that flush fails because the device lost cache contents (a reset, a power event
   on an external enclosure), our unflushed writes in that cache are gone too, and nothing reports
   it to us. The flush regime is therefore at least the device, and possibly larger (a Storage
   Space, or a host beneath a virtual disk) -- which is the flush-equivalence concept recorded at
   `M33+.5`.
3. **The failing agent need not be any process's flush.** On a buffered handle the system's own
   write-back writes dirty pages on its own schedule; when that fails ("Delayed Write Failed"), no
   application flush failed, and whether the error reaches any later flush is undocumented. So the
   hazard exists for a single writer in a single process.
4. **What this does to the contract.** The assumption proposed in the previous section -- that every
   failure affecting the instance's flush regime is reported to the instance -- is not an edge
   condition for multi-writer designs; on a shared device it is an assumption about every other
   writer, and on a buffered handle about the system's write-back. dioring cannot detect any of it
   locally. So the guarantee is truthful **relative to what the instance was told**: dioring reports
   a point durable when every covering flush it issued succeeded and no failure in its regime was
   reported to it -- by the ring, or by the consumer through the import. The contract must say that
   in those words, not imply more.
5. **What can narrow it, all outside dioring's mechanism:** the external-failure import (DI-1.2) is
   the channel for anything the consumer learns -- from its own coordination, or by watching the
   system's storage error reporting (disk and write-back failure events), which is a consumer policy
   and not dioring's. The spike gains a question: does another handle's failed flush on the same
   device lose this handle's unflushed `NO_BUFFERING` writes, observably.

### If points are ordered, ordered by what?

> back to di-1.1, if durability points are ordered, what are they ordered by?

The assistant's answer (proposed, not agreed):

1. **The fundamental sequence is the instance's push order of writes and flushes.** Everything
   about durability on this ring is decided there: a covering flush covers what was pushed before it
   (D-47 makes that the only thing it covers), and the suspect window is bounded by pushes relative
   to an observed failure. Points do not have an order of their own; they inherit one from that
   sequence.
2. **Candidate orders, and why two are wrong:**
   - *Open order* (when a point is created) -- wrong in general. It coincides with the right answer
     only if one point is open at a time, and says nothing about durability if several are open.
   - *Resolution order* (when an outcome is observed) -- wrong as a definition. Covering flushes do
     complete in push order (the surviving half of D-47: a drained operation waits for everything
     before it), but host-sequenced commits use unordered flushes that can complete in any order.
     It is a consequence to report in, not the thing that defines.
   - **Seal order** -- the position in the push sequence of the flush that commits the point.
     This is the one durability actually follows: the prefix property is a statement about seal
     order, and "durable through X" means "every point sealed at or before X".
3. **Consequence: a point has no position until it is sealed.** While open it is a set of writes
   still growing; only its commit gives it a place in the sequence. If points are comparable, they
   are comparable only once sealed.
4. **This surfaces a design question the epoch model hid: one open point at a time, or several?**
   The sample's epochs allow exactly one -- every write joins the currently open epoch -- so open
   order and seal order coincide and the distinction never showed. Opaque points invite a consumer to
   hold several concurrently (one per transaction, say). That is cheap to support over the single
   sequence -- a point is durable at the first successful commit pushed after its last write, with no
   failure in between -- but it makes open order meaningless, and it means two points can share one
   commit. Not decided; the engineer's call.

### The engineer's restatement, corrected

> here's how I would characterize what you told me, please correct me, I know you will! :-)
>
> Ordering by start is incorrect because it's meaningless. Ordering by the completion/seal time is
> not useful.
>
> So the remaining question is whether it's possible to have multiple concurrent epochs on the same
> ring concurrently. I would say "yes", I thought this was the innovation that epochs brought to the
> ring. But I guess that the utility was also that the epoch was a number and one could depend on
> the notion that ensuring that epoch "n" was durable meant that for all m < n, m was also durable.

The assistant's corrections:

1. **Start order: right**, with the nuance that it is meaningless only when several points are
   open; with one open at a time it coincides with seal order.
2. **"Completion/seal" merges two different things, and seal order is the useful one.** Completion
   (resolution) order is what is *not* a definition. Seal order -- the push position of the commit
   -- is what the proposal recommends, and it is precisely what yields "n durable implies every
   m < n durable".
3. **"Concurrent epochs" has two meanings, and the sample already had one of them.**
   - *Several sealed-but-unresolved points in flight together* -- epoch *n* committing while
     *n+1* fills. That is pipelined group commit; the sample's design permits it (its measurement
     harness keeps appending while a commit is outstanding), and seal order handles it with nothing
     new.
   - *Several open points accepting writes at once* -- the new question. The sample never allowed
     it: every write joins the single open epoch.
4. **The two utilities are compatible: number points at seal.** With several open at once, a point
   receives its sequence position when it is sealed, so "n durable implies every m < n durable"
   holds over sealed points exactly as before. A point opened earlier but sealed later sorts later,
   correctly: its writes may still have been arriving. The cost is that the consumer learns a
   point's position only when it seals it -- but release-after-durable can still name an unsealed
   point by its opaque handle, meaning "after this point is sealed and durable". An opaque ordered
   point is then the epoch number in disguise, assigned at seal.

### Does a consumer have to open a point at all? (a what-if, not a proposal)

**A what-if discussion only, per the engineer: "this was not a proposal, it was just a what-if
discussion". Nothing is pending on it and it is not an option under consideration; it is kept as
the record of the exploration.**

> This is just a question - does a consumer really have to formally "open" an epoch? if it were
> just a newtype on a u64, couldn't they just use another higher number? This was to some degree my
> old model of thinking of how it worked

The assistant's answer:

1. **Opening can be implicit; sealing cannot.** A point can come into being at its first tagged
   write. What cannot be implicit is the moment it stops accepting writes, because "n is durable"
   is meaningless while writes tagged n may still arrive. Every model needs a seal; the question is
   only whether it is explicit.
2. **The engineer's old model makes the seal implicit too: using a higher number seals every lower
   one.** Writes carry a consumer-chosen number that never decreases; the first write tagged above n
   seals n and everything below it; a commit request seals the current number when there is nothing
   more to write. Gaps are harmless. A write tagged below the current number is refused. This is
   the sample's model with consumer-supplied labels instead of dioring-assigned ones, and it admits
   **exactly one open point at a time** -- the monotone rule is what makes "n durable implies every
   m < n durable" hold over the consumer's own numbers.
3. **What it buys:** the consumer's existing sequence -- a journal transaction ID, a log sequence
   number -- *is* the durability point, with no mapping table. It is also consistent with
   [DI-D-8](../crates/durable-ioring/DESIGN-NOTES.md#di-d-8)'s intent: dioring's mechanism stays
   hidden; the visible identity is the consumer's own value rather than dioring's.
4. **What it costs:** no concurrent open points -- if the consumer could leave 5 open while sealing
   7, reporting 7 durable would imply 5 durable, so dioring would have to hold 7's report behind 5;
   and a bare `u64` compares across instances, losing the type-level guard on C-3's per-domain
   constraint.
5. **The two models nest; they are not rivals.** The monotone, consumer-numbered model is the
   explicit-handle model restricted to one open point with consumer labels, and is easy to build on
   it (map the consumer's number to the handle). The reverse is not possible: the monotone model
   cannot express concurrent open points. So the general mechanism is handles sealed in order, and
   the monotone model is a natural convenience over it -- possibly the one most consumers use.

#### The what-if, continued: interleaved tags and "commit n"

> and it was only the beginning of it
>
> so my thought was that the client could tag writes with epochs as they saw fit, intermixing
> higher and lower, and then at some point they would say "commit `n`" which would be our cue to
> make `n` and lower durable.

The assistant's earlier answer above had assumed a monotone rule the engineer never proposed. On the
actual what-if -- exploration, nothing proposed:

- **Tags do not change what the device does, only what dioring reports.** A covering flush covers
  everything pushed before it whatever its tag, so writes tagged above *n* pushed before the commit
  are covered incidentally and simply not reported -- reporting less than reality, which is safe.
  The tag is pure bookkeeping over the ring's physics.
- **"Commit n" seals a prefix of the number space, so the earlier head-of-line objection
  disappears.** Sealing 7 while leaving 5 open is impossible by definition: committing 7 commits
  5. "n durable implies every m < n durable" holds over the consumer's own numbers by construction,
  and epochs above *n* stay open -- several open points come free.
- **Familiar prior art:** this is the shape of a log force -- "make the log durable up to LSN n" --
  applied to every write rather than to a log alone.
- **Tags give host sequencing a real advantage.** A covering flush waits for everything outstanding
  on the ring, including writes tagged above *n*. Host sequencing can wait only for the writes
  tagged at or below *n* and then push unordered flushes, so a commit need not wait on later,
  unrelated work. A difference between the strategies the single-epoch sample could not show.
- **Questions the what-if raises, not answered:** what a write tagged at or below an
  already-committed *n* means (refused, or a new obligation needing another commit); whether
  `commit(5)` after `commit(7)` is a no-op; and that a failure's suspect set is defined by push
  order, not tag, so it can include writes of still-open epochs above *n*, which the inventory would
  report with their tags.

The engineer's answers to those questions (still within the what-if):

> Once `n` is committed, a new write tagged a <= `n` is an api violation.
>
> I would say that `make_durable(5)` after `make_durable(7)` is a no-op with a particular return
> code akin to `already_durable`.
>
> It's fine to include writes that are from a future epoch, they just aren't guaranteed to be
> durable yet

The assistant's notes on them:

- **A late write at or below a committed `n` is a violation: agreed.** How it surfaces is worth
  choosing deliberately: refusing the push with a distinct error *before anything is reserved* --
  nothing reaches the kernel and the buffer is returned -- matches how `windows-ioring-sys` treats
  every legality check ([D-28](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-28)). The violation
  is triggered by the commit *request*, not by durability.
- **`make_durable(5)` after `make_durable(7)`: a no-op, but "already durable" is only one of three
  truthful answers.** If 7 is committed but its flush is still outstanding, 5 is already *requested*
  and not yet durable; if 7's commit failed, 5 is in the failed state. So the no-op should report
  5's current state -- durable, pending, or failed -- rather than a single code that would be false
  in two of the three cases.
- **Future-epoch writes in a suspect set: agreed**, and one consequence: once a later epoch is
  committed, its durability requires its own suspect writes to be resolved first, which is the
  prefix property again and needs nothing new.
- **Naming, in passing:** the what-if moved from `commit(n)` to `make_durable(n)`, which names the
  outcome rather than the action -- consistent with the crate's naming argument, with the same
  caveat that the call requests durability and the completion reports whether it was achieved.

The engineer accepted all three notes, and on naming:

> Re: naming: I wanted to move more towards a pure storage idiom rather than a database
> (prepare/commit) idiom. I'm open to whatever.
>
> What do you think? I think that this relaxes the API shape and makes the epoch *usefully* ordered

The assistant's assessment (an opinion on the what-if, not a decision):

- **Agreed: it relaxes the shape and makes the order useful.** The consumer's own number is the
  order; `make_durable(n)` is the seal, and because it seals a prefix, seal order *is* numeric order
  -- so the earlier "order by seal" analysis is satisfied by construction. No open or seal
  ceremony, no handles, several open epochs for free, and a journal ID or LSN can be the tag
  directly.
- **It matches the ring's physics better than independent groups did.** A covering flush covers
  everything pushed before it, so "make B durable but not A" was never physically available; the
  prefix is the physical truth and this model states it. The only thing given up is host
  sequencing's ability to exclude lower-tagged writes from a wait -- and the consumer controls the
  tags, so it can avoid that by construction.
- **It composes cleanly with healing.** Since a write at or below a committed `n` is a violation,
  rewriting a failed epoch's data must carry a *higher* tag -- which is exactly DI-1.2's proposed
  rule that a resolution takes effect at a later durable point. The two rules agree without either
  being bent.
- **It would amend [DI-D-8](../crates/durable-ioring/DESIGN-NOTES.md#di-d-8).** DI-D-8 says points
  are opaque. Here the visible identity is the consumer's own ordered number; what stays hidden is
  dioring's mechanism (which flushes, which strategy). That preserves DI-D-8's intent but not its
  wording, so adoption means restating it, not silently departing from it.
- **Smaller costs to carry:** a bare number compares across instances, losing a type-level guard
  on C-3's per-domain rule (documentable, or the durability report can carry the instance's
  identity); sparse tags mean an ordered map of open epochs inside dioring; and a consumer whose
  native sequence wraps (jbd2's 32-bit transaction IDs do) must map it onto a non-wrapping one.
- **Naming -- "through" carries the semantics.** A storage idiom fits better than prepare/commit.
  The assistant's suggestion: `make_durable_through(n)` for the request and `durable_through()` (and
  a `Durable { through }` completion) for the report, because "through" says *n and everything
  below*, which is the single most important property and the one `make_durable(n)` alone could be
  misread to omit. The sample already uses `durable_through` for the same watermark. Alternatives
  in the same idiom: `sync_through(n)` (shorter, closer to `fsync`, but names the mechanism) and
  `persist_through(n)`. Open.

#### Adopted

> let's go to this model. I agree with make_durable_through().

Recorded as [DI-D-9](../crates/durable-ioring/DESIGN-NOTES.md#di-d-9), with
[DI-D-8](../crates/durable-ioring/DESIGN-NOTES.md#di-d-8) amended (the label of a point is the
consumer's; the mechanism stays opaque) and `DI-1.1` archived as resolved. The explicit-handle model
explored above is superseded.

### Reads

> Reads can still go through this ring, right? We spend a lot of time talking about writes but
> clients can still use the ring for both reads and writes I hope

The assistant: yes, first-class, untagged, and not epoch members -- with three interactions worth
designing for, and one question the operation set raises:

1. **Reads lengthen covering commits.** A covering flush waits on everything outstanding on the
   ring, reads included -- a cost, not a correctness issue. Host sequencing does not wait on them.
2. **A read sees completed writes, durable or not.** Durability is about surviving loss, not
   visibility. After a failure that lost cache contents, reads of a suspect range may return stale
   data, so the suspect inventory is also the list of ranges whose reads cannot be trusted until
   resolved.
3. **A read overlapping an in-flight write lands in unspecified order**, the same as two
   overlapping writes, and is the consumer's to order under
   [DI-D-6](../crates/durable-ioring/DESIGN-NOTES.md#di-d-6).
4. **The question it raises: which other operations are exposed.** If a consumer can push its own
   flush, that flush's failure is a failure the suspect window must hear about; if it can cancel a
   tagged write, the cancellation fails that write's epoch. Queued as `DI-2.6`.

### No interfering operations

> I don't believe that the consumer should be able to put operations onto the ring that interfere
> with our meta-control of it

Recorded as [DI-D-10](../crates/durable-ioring/DESIGN-NOTES.md#di-d-10). The assistant's additions:

- **It has to be enforced by the types**, the way `windows-ioring-sys` enforced the equivalent for
  `EventDelivery` (D-43, `RingScope`): no public method may return a borrow through which a raw
  ring operation can be pushed, including the raw-SQE seam. A rule stated only in documentation is
  the kind this workspace has repeatedly found unenforced.
- **No escape hatch, even for experts, and that is consistent with OPTION INTEGRITY**: the rule
  allows foreclosing an option that produces a demonstrably wrong answer, and an interfering
  operation produces a false durability report.
- **It leaves a narrower question: which operations dioring offers in mediated form.** Cancel is a
  real consumer need, and a cancelled write's outcome is indeterminate, so it would join the
  suspect set. Registration needs a story if dioring registers anything itself. Both moved into
  `DI-2.6`.

The engineer's response:

> 1: agreed, type based enforcement
> 2: no escape hatch, agreed
> 3: The thing is that even cancel doesn't have a guaranteed response time. I'm somewhat of the mind
> to not provide it in v1. I don't understand the registration point, maybe this is something we
> work through when it happens?

The assistant's notes:

- **Cancel: the response-time argument is the right one, and it is a real reason rather than "no
  consumer".** Cancel is a request; the operation may still complete normally, later, so it gives
  neither shutdown nor a deadline the bound they want. Shutdown is already served by stopping
  submission and draining, which the ring's rundown requires anyway. Recorded as the engineer's lean,
  with the item parked as `DI-6+.4` rather than dropped, and with one constraint on v1: do not
  foreclose adding it.
- **Registration, restated more plainly.** Registered buffers and files are ring-wide state. Under
  DI-D-10 a consumer can only reach that state through dioring, so if dioring offers registration it
  is a mediated operation, and if dioring also registers anything for its own purposes the two must
  coexist. If dioring registers nothing of its own, the question shrinks to "does dioring pass
  registration through". Agreed to work it through when the API shape needs it; moved into `DI-2.2`
  so it is tracked.

### Healing, as a trace of calls and events

> I don't understand the healing point. Can you give me the series of API calls / events that are
> visible that the consumer is calling or consuming (events) please

The assistant's worked trace (names illustrative, none decided; `->` is a consumer call, `<-` an
event in the completion stream):

```text
-> write(tag 41, A) ; write(tag 41, B) ; write(tag 41, C)
-> make_durable_through(41)                 // seals everything <= 41
<- WriteDone(A) ; WriteDone(B) ; WriteDone(C)
<- Failed { failure: F1, through: 41, suspect: [A@41, B@41, C@41] }
                                            // durable_through() stays 40
-> write(tag 41, A')                        // REFUSED: 41 is sealed (DI-D-9 rule a)
-> write(tag 42, A') ; write(tag 42, B') ; write(tag 42, C')   // the rewrite, under a higher tag
-> heal(F1)                                 // consumer asserts: "I have re-issued what I need"
-> make_durable_through(42)
<- WriteDone(A') ; WriteDone(B') ; WriteDone(C')
<- Durable { through: 42 }                  // F1 resolved; 41 and 42 now reported durable
```

**Why `heal(F1)` exists.** Without it, a successful commit of 42 would let the watermark pass 41,
which asserts 41's data is durable -- false if the consumer never rewrote it. dioring cannot know
whether the consumer rewrote anything (it tracks identities, not contents), so the consumer has to
say so. Without the heal the trace ends differently:

```text
-> make_durable_through(42)
<- WriteDone(...)
<- Blocked { through: 42, by: F1 }          // 42's own flush succeeded, but the watermark
                                            // cannot pass the unresolved failure at 41
```

**Why the heal must come before the healing seal.** The rule is that a heal takes effect at the
first durable seal made *after* it. If the consumer heals after 42 is already sealed, dioring
cannot tell whether the rewrites were pushed before 42's flush (covered) or after it (not), so it
waits for the next seal. The natural order -- rewrite, heal, make durable -- satisfies the rule
without the consumer thinking about it.

**Abandoning instead:**

```text
-> abandon(F1)
<- Abandoned { failure: F1, suspect: [A@41, B@41, C@41] }
```

What `durable_through()` reports after that is open question 5: whether the watermark may pass an
abandoned epoch.

### Healing accepted; reopening files

> ok this seems reasonable!
>
> During some of our discussions there was talk about having to possibly re-open files. Is that
> still in scope?

The healing mechanism -- a consumer-declared heal, taking effect at the first durable seal after
it, with rewrites carried under a higher tag -- is recorded in `DI-1.2` as agreed in principle.
The open sub-questions there stand.

On reopening, the assistant's answer:

- **In scope, parked past v1, with one v1 obligation.** Removable media is supported without
  judgement but was ruled out as a design driver, so rebinding a file identity to a new handle is
  `DI-6+.3`, parked. What v1 owes it is `DI-2.2`'s non-foreclosure constraint: file identity must not
  be welded to the handle.
- **Rebinding turns out to be a convenience, not a correctness need.** The `Failed` event hands the
  consumer the suspect inventory. A consumer whose handle died can therefore reopen the file, start a
  new instance over the new handle, rewrite the suspect ranges there, and abandon the failure in the
  old instance -- recovering fully without dioring ever rebinding anything. Rebinding would only let
  the heal happen in place, in one instance.

### Rebinding withdrawn; what a bad handle means

> yes the rebinding seems like a nice capability but it's very heavyweight, not appropriate for
> this layer. I'm not sure what exactly we have to do when file handles go bad. I don't think there
> are any situations where this happens any longer except for the media disconnects.

Recorded as [DI-D-11](../crates/durable-ioring/DESIGN-NOTES.md#di-d-11); `DI-6+.3` withdrawn. The
assistant's answers:

- **What dioring has to do when a handle goes bad: nothing special.** Operations on it complete
  with errors; a failed write takes the ordinary failure path; the consumer resolves. No dead-handle
  state, no recovery machinery. The one obligation is that the `Failed` inventory names files in the
  consumer's terms, not by dioring's handle, so it stays usable after the handle is gone -- which
  replaces the earlier "do not weld identity to the handle" constraint with something concrete.
- **Media disconnects are not the only cause.** A forced volume dismount (administrative tools,
  `chkdsk` taking the volume), a BitLocker data volume being locked, a virtual disk being detached --
  directly relevant to a stack with a VHDX layer -- a network share whose reconnect fails, and loss of
  an iSCSI or SAN path can all leave handles invalid. Stated with moderate confidence about the exact
  error each produces; it does not matter here, because every one reaches dioring as an error and
  none needs handling of its own.

### Closing DI-1.2, and failures as objects

> well those all "feel" like media disconnects to me, although I can see where an admin dismount and
> a bitmount volume lock clearly aren't. I don't want to split hairs. What do we need to do to close
> on this.

The bad-handle topic was already closed by DI-D-11. For DI-1.2 the assistant offered a seven-point
closing slate, recorded in the checklist item. The engineer then asked:

> to be clear, the failures here are durability failures. Should these get unique opaque handles?
> And they should be queryable / enumerable?

The assistant: yes to both, with reasons.

- **One kind of object, several causes.** A failed write, a failed flush and an imported external
  failure all prevent durability; they differ in the scope of the suspect set (known for a failed
  write, uncertain for a failed flush). One `DurabilityFailure` with the cause as a field.
- **Unique opaque handles, because resolution targets a failure.** `heal` and `abandon` need
  something to name, and `Blocked` events need something to point at. A `Copy` identity that owns
  nothing and is never reused within the instance -- the same shape `windows-ioring-sys` already
  uses for `OperationId`. Using one instance's failure handle on another should be refused.
- **Enumerable and queryable, because an event is delivered once.** If the consumer's code drops
  or mishandles a `Failed` event, the watermark stalls with no visible reason; enumerating
  unresolved failures is how anything -- the consumer, a diagnostic, a component that starts later
  -- finds out why. Each is queryable for its state and its suspect inventory.
- **The inventory is frozen at observation**, since the suspect set is "pushed before the failure
  was observed" and cannot grow afterwards -- which makes it safe to hand out. The borrow question
  in `windows-ioring-sys`' design instructions will apply to whatever returns it.
- **After resolution, a bounded memory.** A resolved handle still answers "resolved, and how", but
  its inventory is released, with a bounded history of finished identities, as the ring crate's
  contract oracle already does.

### Only durability failures are failure objects

> you seemed to want to include general write failures into the mix, not just specific durability
> failures. I think it should only be durability failures. concerns?

Correct reading of the earlier proposal, which had a failed write as one cause of a
`DurabilityFailure`. The assistant agreed with the narrowing, with one obligation and two residual
concerns:

- **The obligation: say exactly what "durable through n" covers.** If a failed write is just an
  operation error on its own completion, then a successful commit of 41 must not be read as "every
  write I tagged 41 is durable". It means: every write tagged at or below 41 *that completed
  successfully*, up to the byte count its completion reported. That is the sample's checkpoint
  lesson (a covering flush orders execution, it does not aggregate results) moved into the
  contract's wording, where an expert reads it and a naive consumer can still miss it.
- **Mitigation for the naive class, suggested only:** the `Durable` event could carry a count of
  failed or short writes in its range -- a reminder, not a failure object, and cheap.
- **Residual concern, judged covered by an existing assumption:** a device error on a write can be a
  symptom of a fault -- a reset -- that also discarded *other*, successful writes from the device's
  cache, after which a later flush succeeds over the loss. Treating write errors as durability
  failures would not actually fix that, since the successful writes' loss is reported nowhere; it is
  the device losing cached data without failing a flush, which the contract already lists as an
  assumption (the device honours the flush). So the narrowing gives up nothing real.

## DI-1.2 closed and the contract written (DI-M1 complete)

The ten DI-1.2 questions were settled one by one -- most with refinements recorded in the archived
item ([COMPLETED-CHECKLIST.md](../crates/durable-ioring/COMPLETED-CHECKLIST.md#di-12)) -- and
recorded as [DI-D-12](../crates/durable-ioring/DESIGN-NOTES.md#di-d-12) (failures and their
resolution) and [DI-D-13](../crates/durable-ioring/DESIGN-NOTES.md#di-d-13) (the completion queue).
The engineer then asked for DI-1.4, and the contract was written as
[CONTRACT.md](../crates/durable-ioring/CONTRACT.md), against the settled decisions only, with what
remains open listed under "Not yet specified". That completes DI-M1; DI-M2, the API shape, is next.

## DI-2.1 drafted

On the engineer's instruction ("implement di-2.1"), the tag and completion-queue types were drafted
as [API.md](../crates/durable-ioring/API.md), with the type sketch compile-checked standalone under
the pinned toolchain. Proposed, not approved; its choices and the two open points it proposes to
settle are listed in the document itself rather than restated here.

Later the same day the engineer had the epoch type made a generic parameter ("my c++ roots are making
me wonder if the epoch type could be a generic parameter instead of just u64"), the prose simplified,
and "watermark" replaced by "high-water mark" throughout; then approved API.md ("api.md looks good").
Recorded as [DI-D-15](../crates/durable-ioring/DESIGN-NOTES.md#di-d-15).

## DI-2.2: what the ring crate cannot give dioring safely, and a `win-` crate to give it

The engineer answered DI-2.2's five points: dioring takes both owned buffers and registered spans;
the consumer's sidecar is a generic type parameter; file handling was left to the assistant; buffer
registration is mediated by dioring ("see the queue planning work"); and a file may be removed once
nothing is in flight against it.

Drafting the sketch against the real `windows-ioring-sys` surface (the draft compiles against it,
not standalone) found two things that crate's safe surface does not provide:

1. **File registration is `unsafe`, with no safe counterpart.** `Batch::register_files` requires
   that the handles stay valid for the ring's whole life, because Win32 has no unregister, and the
   sys ring does not hold them. dioring is `#![forbid(unsafe_code)]`
   ([DI-D-4](../crates/durable-ioring/DESIGN-NOTES.md#di-d-4)), so as things stand it cannot
   register files at all.
2. **A refused push drops its buffer and sidecar.** `Batch::finish_owned` drops the payload on
   any `Build*` failure. The common failure is a full submission queue, which no caller can test
   for in advance: the sys crate exposes no free-slot count, and the build is the test. So the
   approved API.md promise that a refused push returns its buffer cannot be kept for a `Ring`
   refusal.

There is also a third, structural point. A ring accepts one file registration and one buffer
registration in its life. A file added after construction therefore cannot be registered, so
`add_file` at run time can only use a `SharedFile`.

The engineer's direction: "or sketch out for a new win- crate that provides the api you need". By
the [root naming rule](../DESIGN-NOTES.md#new-crates-take-the-win-prefix), a safe layer more or
less directly over a Windows API is a `win-` crate. It carries no `-sys`, because owning the
handles is a decision taken on the caller's behalf. The working name is `win-ioring`.

The sketch was compile-checked against the real sys crate. Its shape:

- **Need 1, owned registration.** `Ring<T, X>` owns the `IoRing<T, X>` and holds a `SharedFile`
  clone of every registered handle for the ring's life. Ownership discharges the sys crate's
  safety obligation, so the one `unsafe` call lives inside `win-ioring` and its API is safe.
  Registration happens only in the constructor, from a `Registrations { files, buffers }`, so a
  second registration cannot be expressed. Construction blocks until both registrations are
  claimed. If rundown fails, the held handles must be leaked, by the same rule as sys
  [D-73](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-73).
- **Need 2, refusal returns the payload.** `PushRefused<T, X> { error, buffer: Option<T>, extra }`.
  There are two places to fix it:
  - **Option A**, in `windows-ioring-sys`: `finish_owned` returns what it now drops. This breaks
    every owned push's error type, and `win-ioring` then needs nothing for it.
  - **Option B**, in `win-ioring` only: the sys ring is lent a non-owning `Lent { ptr, len }`
    buffer and carries only a slot index as its sidecar, while `win-ioring` keeps the real buffer
    and sidecar in its own slab. That is a second inventory beside the sys ring's. It must
    duplicate D-73's leak-on-failed-rundown rule, and it must re-wrap every push the consumer
    uses. The write path was sketched to show this.

The assistant's recommendation:

- Need 1 belongs in `win-ioring`. It is small and clean, and it is a real gap.
- Need 2 is a defect in the sys crate's owned push and should be fixed there (Option A), under the
  fix-the-layer policy. Option B is the duplicate-then-decide path, workable but costly: it is a
  second copy of an ownership rule the sys crate deliberately made single
  ([D-55](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-55)).

This is the engineer's call; it is not decided.

The engineer then asked whether both could be fixes to `windows-ioring-sys`. They can, and the
case for a separate crate was weaker than the assistant presented it:

- **Need 1 fits the sys crate's existing pattern.** The ring already keeps registration-time state
  for its whole life: the handle and buffer-info arrays the kernel reads late (D-32), held in
  `late_read` and leaked on a failed rundown by the D-73 rule. A safe
  `register_shared_files(Vec<SharedFile>)` would keep the `SharedFile`s beside them, under the
  same rule. The `# Safety` note's "there is no safe counterpart" argues only that the
  per-operation inventory cannot express a ring-lifetime hold. A ring-lifetime field can.
- **The assistant's reason for dropping `-sys` was wrong.** `SharedFile` is already the sys
  crate's own ownership type, and the ring already holds clones of it per operation. Holding them
  for the ring's life adds no new policy.

The engineer then directed: "plan to make these changes to windows-ioring-sys". `win-ioring` is
therefore not pursued. The work is planned as `M30.1` (a refused owned push hands back its payload)
and `M30.2` (safe file registration from `SharedFile`s) in
[the ring crate's checklist](../crates/windows-ioring-sys/CHECKLIST.md). `DI-2.2` carries a
cross-component prerequisite on both.

The engineer confirmed the assistant's reading of "see the queue planning work": dioring accepts
buffers that are already allocated and placed, and never allocates or places one itself. Placement
belongs to the topology planner's residency work. The five settled points are listed on `DI-2.2`
in [the checklist](../crates/durable-ioring/CHECKLIST.md).

`M30.1` and `M30.2` were then implemented in `windows-ioring-sys` (decisions D-80 and D-81 there).
The engineer directed that, until this whole set of changes is about to release, dioring consumes
the prerelease ring crate by relative path. That is recorded as
[DI-D-16](../crates/durable-ioring/DESIGN-NOTES.md#di-d-16), with the pin before release scheduled
as `DI-3+.5`.

With M30 landed, the engineer agreed DI-2.2 could be completed and told the assistant to proceed.
The draft is [API.md](../crates/durable-ioring/API.md) sections 7-10. Two points differ from what
the assistant had described just before drafting:

- **dioring owns the buffer registration**, rather than handing the consumer a `RegisteredBuffers`.
  A gated operation is issued later, from inside `pop` or a seal, where no consumer borrow exists,
  so dioring must reach the registration itself. The consumer reaches the bytes through accessors
  the ring crate already guards. The registered type became its own parameter `R`, so that owned
  writes keep write-only buffer types.
- **`remove_file` is refused while the file has a write no successful flush has covered**, not
  only while something is in flight. Once the file is gone dioring cannot flush it, so an epoch
  holding such a write could never become durable.

The engineer approved both: "its fine for dioring to hold the buffers", and the removal rule
"fine, if possible ensure there is a descriptive err". The refusal became `FileBusy`, which reports
every hold on the file at once (operations in flight, operations held for a gate and the lowest
gate, uncovered writes and the epoch to seal through, the unresolved failures involved), and whose
`Display` says what clears each. DI-2.2 closed as
[DI-D-17](../crates/durable-ioring/DESIGN-NOTES.md#di-d-17).

## DI-2.3: completion routing

The engineer asked for Model A and Model B to be explained. He then gave them labels at their
source: Model A is **threadless dispatch**, Model B the **dedicated domain thread**. He stated the
rule for terminology: live material may be updated, but history is not rewritten. The follow-up to
label first uses across the workspace is root `M38.1`.

The assistant laid out DI-2.3's points: an I/O-free core with a front end per model; Model A
delivering through a consumer callback; locking with the consumer's callback run unlocked; checking
that `EventDelivery` serializes its callbacks; signalling the ring's completion event for entries the
kernel never announced; and Model B waiting as the ring crate does. The engineer: "this all is
reasonable". The accepted points are listed on `DI-2.3` in
[the checklist](../crates/durable-ioring/CHECKLIST.md).

He added: "i wish we had an alternative to locking here but i guess we have too much correlated state
to consider a lock free data structure or something like queued updates but we would still need to
lock and integrate the queue too often."

The assistant's response:

- **The lock under Model A is the ring's before it is dioring's.** `IoRing`'s submission queue is
  not thread-safe, and `EventDelivery` already serializes every submitter behind a mutex. So Model A
  has a lock whatever dioring does. The live question is one lock or two, and one is better: it
  removes the lock-ordering problem. `EventDelivery` locks only the `IoRing` today, so sharing that
  lock means extending it to carry a caller's state, or dioring building Model A itself.
- **Model B is the lock-free answer.** One thread owns the ring and dioring's state, so nothing is
  locked. That is the dedicated domain thread's reason for existing; Model A trades it for not
  owning a thread.
- **Queued updates are possible but change the API.** Consumer calls could become messages on a
  lock-free multi-producer queue (`windows-waitable-queues` has them), drained by the serialized
  delivery callback as the sole mutator -- flat combining, in effect. Three costs. Synchronous
  refusals (`Sealed`, `UnknownFile`, `GateAbandoned`) need the state at call time, so they would
  either arrive later as entries or need a published read-only snapshot; because `E` is generic, a
  plain atomic will not hold the seal point. A write reaches the kernel only at the next drain,
  which adds latency. And each enqueue needs a wake, meaning a signal of the completion event.
  Recorded as an option, not foreclosed.

The engineer then asked: "and what if it wasnt? what if an event type was passed in and the
equivalent of 'e_pending' were returned and the event signalled when done?" -- that is, if
`EventDelivery` did not serialize its callbacks.

Reading `EventDelivery` answered the first half: it does not serialize them. Its callback drains,
re-arms, and drains again, so a second callback can run on another pool thread while the first is
still in its second drain. Delivery order across threads is therefore not completion order, and
DI-D-13's guarantee cannot rest on the pool. The assistant proposed that dioring order delivery
itself, with a single deliverer draining dioring's own queue (DI-2.3 point 8).

On the event: it is a second channel for the same fact, so it inherits the same ordering
obligation. It is valuable because it lets one specific waiter block on one commit -- a synchronous
commit call, a group-commit waiter, or a Model B thread waiting on several handles -- without
consuming the queue. The open points are listed as DI-2.3 point 9.

The engineer then clarified the question behind it: "i guess the question is how you are using
'serialized' here. if you mean that you always had planned to deliver them one by one (yes, a strict
definition of serialized, but i anticipated something much lesser), then i am happy. my event
proposal was to break up what i anticipated your original proposal to be, so lets just ignore it."
The assistant confirmed the strict meaning: consumer callbacks never run concurrently, and entries
are handed over one at a time in queue order. Point 8 was accepted and point 9 withdrawn.

On the one open question, whether dioring's state should share the ring's lock, the engineer:
"do not try to share locks. bad coupling." dioring keeps its own lock. The order is dioring's lock,
then the ring's, and `EventDelivery`'s documented rule (it calls back with the ring lock released)
means nothing inverts it. This supersedes the assistant's earlier "one lock is better" argument in
this session, which treated the lock-ordering cost as the deciding factor and missed the coupling
between layers that sharing would create.

The engineer sharpened it: "a single lock is better but taking advantage of an implementation
artifact in this way is terrible coupling. to fix it, we would need to formally make the ioring a
synchronization provider, which is clearly not in its scope. when you see it that way, using it
without 'permission' surely isnt better than getting it properly supported."

Applying that test to the assistant's own point 10 found the same defect there. The lock order
relied on `EventDelivery` calling back with the ring lock released, and point 8 relied on callbacks
being able to overlap. Neither is stated on `EventDelivery`'s public surface; both appear only in
comments on its private `drain` and callback body. Unlike lock sharing, these two properties are
squarely in scope: whether a callback may re-enter, and whether two may run at once, are what any
callback API owes its users. They are queued for specification in the ring crate as `M31.1`, and
DI-2.3 point 11 carries the prerequisite.

The engineer asked whether the design could move past that dependency, given the implementation
already conforms and the tests that keep it so are queued. The assistant: yes -- the prerequisite
sat on the wrong item. A design decision needs only that the contract be the intended one; what
actually depends on it is code, first in `DI-3+.2`. On "do both", DI-2.3 closed as
[DI-D-18](../crates/durable-ioring/DESIGN-NOTES.md#di-d-18), and the prerequisite moved to
`DI-3+.2`, with the ring crate's `M31.1` handoff pointing there.

## DI-2.4: multi-file epochs

The engineer said the two strategies sounded like the same thing said two ways. The assistant
explained that they differ in who enforces the ordering. The **chain of covering flushes** uses
the kernel's drain barrier: each flush waits for everything queued before it on the ring, including
other files, later epochs and the previous flush, so the N flushes run back to back, but no host
bookkeeping or round trip is needed. **Host sequencing** has dioring watch completions and issue
ordinary flushes once the writes tagged at or below n have completed: the flushes can run in
parallel and later work is ignored, at the cost of one round trip. Which wins depends on the file
count, how busy the ring is and the device, so both are exposed and measured (DI-6+.1).

The engineer asked what host sequencing's tracking costs. The assistant corrected its own earlier
framing: the tracking is **not** a differentiator, because dioring keeps it under either strategy.
The suspect set (DI-D-12) needs every write not yet covered by a successful flush; `Durable`'s
ordering (DI-D-18) needs to know when every write at or below n has completed; and `remove_file`'s
`FileBusy` needs per-file uncovered writes. What host sequencing adds on top is small: per file, the
lowest epoch with a write still in flight, consulted when a seal is waiting on that file. Seals are
prefix-ordered, so each file needs only its lowest pending seal. The real cost of host sequencing
is the round trip, which is latency rather than bookkeeping.

The engineer: "to me, it seems clear that the chain is the only real choice. otherwise if there are
multiple independent devices, consumers will be forced to maintain independent rings to prevent
co-flushing." The assistant pushed back: as it reads the mechanisms, the coupling runs the other
way. The chain's barrier is ring-wide, so a flush for a file on one device waits for every operation
queued earlier on the ring, including writes to an unrelated, slower device. The N flushes of a
multi-device epoch also run back to back. That is the strategy that would push a consumer to one
ring per device. Host sequencing waits per file, only on the writes the seal covers, and its
flushes to different devices run independently. Not decided; the assistant asked whether
"co-flushing" meant something it had missed.

The engineer: "hmmm i just realized that because of the ordered coupling, a consumer that wants
independent durability to different devices has to by construction use multiple rings. that is
unanticipated but a clear outcome." The assistant agreed and placed the cause. An instance has one
high-water mark, and `make_durable_through(n)` seals every file at or below n, so durability is a
prefix of the whole instance under either strategy. Independent durability per device therefore
means one instance, and so one ring, per device. This extends DI-D-14's separate-instances argument
to devices. Lanes (DI-D-14, recorded and not proposed) are the feature that would relax it. It also
narrows DI-2.4 to sealing one durability domain that spans several files. Within that domain the
chain's cross-device stall is a coupling the consumer chose, but its back-to-back flushes and its
wait on later epochs remain, so both strategies stay. Recorded on DI-2.4.

## Durability lineages (raised, not decided)

The engineer: "this is an extremely undesirable consequence of this design. i wonder if we could
have a notion of 'durability lineages', where the rules apply as we have defined them within a
lineage, but multiple lineages can coexist on a ring, and reads do not participate in them at all,
unless they have to because files and lineages have a coupling?"

This is the feature DI-D-14 recorded as "lanes", not proposed and not foreclosed, now with a
concrete need. The assistant's analysis:

- **A lineage is everything the contract so far gave an instance:** an epoch space, a seal point,
  a high-water mark and a failure set. Every rule applies unchanged within a lineage. Several
  lineages share one ring, one completion queue and one delivery.
- **The coupling that matters is file to lineage, through the flush.** A flush acts on a file. If
  two lineages wrote one file, a flush for either covers both, and a failed flush puts both at
  risk, so failure scope would cross lineages. The clean rule is that **a file belongs to exactly
  one lineage**, fixed when the file is given to dioring. A write then takes its lineage from its
  file, so the write call does not change. One lineage may still span several files and devices,
  as a database's log and data would.
- **Reads take no part,** as the engineer proposed, except through a gate: a gate names an epoch,
  so with lineages it names a lineage's epoch. That is exactly the "unless they have to" case.
- **The strategy question changes.** The chain's barrier is ring-wide, so a lineage's covering
  flush waits for other lineages' writes. Lineages on one ring are independent only under host
  sequencing. The chain stays correct, but it gives back the coupling lineages exist to remove;
  it fits a lineage alone on its ring.
- **Ordering and delivery:** `Durable` names its lineage and follows the completions of that
  lineage's writes at or below n. One single deliverer still serves all lineages. A slow consumer
  callback for one lineage delays the others' entries -- shared delivery, not shared durability.
- **What it reopens:** approved decisions that say "instance" where they now mean "lineage" --
  DI-D-9 (the seal), DI-D-12 (failures, imports), DI-D-14 (epoch scope, one author), and API.md
  (lineage on `make_durable_through`, `durable_through`, `epoch_state`, `Durable`, `Blocked`,
  `Failed`, and on file setup). An instance with one implicit lineage would keep the simple case
  as it is today.
- **A side effect:** independent clients could share a ring as separate lineages, instead of the
  separate instances DI-D-14 requires today.

The engineer: "i think we need this. to have a single dioring and just be careful about your
durability. we just teach them to have separate lineages to have independent counters and they
should be seta normal mode of operation needs to be". The assistant read the last clause as
"lineages should be set up as a normal mode of operation". The direction was recorded as
[DI-D-19](../crates/durable-ioring/DESIGN-NOTES.md#di-d-19): lineages are adopted, and they are the
normal way to get independent durability -- one dioring per ring, with a lineage per independent
counter. The specifics became `DI-2.9`, placed ahead of DI-2.4 because they change what DI-2.4
chooses between. DI-D-14 carries an amendment marker.

The engineer asked: "does a file have to be on only one lineage or can the lineage be a default?"
The assistant: it can be a default. Exactly one lineage per file was proposed only to keep each
failure inside one lineage, and relaxing it makes nothing incorrect. What sharing costs is failure
scope: a flush acts on the whole file, so a failed flush on a shared file puts at risk the uncovered
writes of every lineage that wrote there, and the failure belongs to each of them. DI-D-12 already
lets a failure span several epochs, so this extends the same relation to lineages. A flush for one
lineage also makes another's completed writes durable, but does not credit them; each lineage still
advances only by its own seal. The lean is a default lineage per file, with a write allowed to name
another explicitly, so the coupling is visible and the consumer's choice. Recorded on DI-2.9
point 1.

The engineer asked whether Rust has a convention for an optional parameter that is a reference to
a set of "advanced" parameters. The assistant: no optional parameters, so three patterns -- an
options value with `Default`, passed by value and built with setters (this repository's
`PushOptions`); a plain method beside a `_with` form; or a builder for the whole operation -- and
advised against `Option<&Advanced>`. It proposed the first two together. On "do it", API.md gained
`write_with`, `read_with` and the registered-span `_with` forms taking `WriteOptions` and
`ReadOptions`, the gate moved into them, and the sketch still compiles against the ring crate.
Recorded as [DI-D-20](../crates/durable-ioring/DESIGN-NOTES.md#di-d-20).

On DI-2.9 point 2 the engineer settled lineage identity: "dioring minted handle. apis on dioring to
enumerate. when minting, consumer may supply a utf8 description." Recorded on the item, with the
assistant's working position for the details (a `Copy` handle carrying its instance, so a foreign
one is refused; an optional description returned by an enumeration of owned values). Whether
lineages can be minted after construction, and whether an empty one can be retired, stay open.

The engineer settled both: "when: at any time. empty lineage retirement: yes." Point 2 closed. The
assistant recorded a working position for "empty" (no file bound to it, nothing in flight or held,
no uncovered write, no unresolved failure), a descriptive refusal like `FileBusy`, and retired
handles never reused.

The engineer corrected the assistant's list of what remained open: "i thought we answered the file
lineage as default in the affirmative." He had, when he told the assistant to proceed with the options
design whose `WriteOptions` carries the override; the assistant had kept it open by mistake. Point 1
closed as the default with an explicit override. On point 3: "the default lineage: yes, it has a
presence, can be found via enumeration." Point 3 closed. The assistant's working position for the
details: the enumeration marks the default, and the default cannot be retired, since files given no
lineage and the plain API forms name it.

The engineer then proposed: "it looks like essentially in api.md, all of the places that call for
epoch to be a parameter, a new struct which has both 'lineage' and 'epoch' contained in it is passed.
i would tend to call this new type 'epoch', and rename the current 'epoch' to 'epoch id'." The
assistant agreed and named three consequences. `Epoch` can only be `PartialOrd`, because epochs of
different lineages are unrelated. Gates can cross lineages, so the log-and-data example becomes a
join dioring performs inside one instance. And the proposal supersedes point 1, which had just
closed: if every write names its lineage, the file's default lineage and its override have nothing
left to do. It offered (a), dropping the file-to-lineage binding, or (b), keeping it as a check.
The engineer chose (a): "epoch id is better in cases where it is not, logically, a number." API.md
gained section 12 and the sketch was amended and compile-checked. DI-2.9 points 1 and 4 record the
outcome; point 7 carries the term sweep through CONTRACT.md and the decisions it re-reads.

Point 5, failures and imports per lineage, approved: "point 5: ok!" A failed flush is a failure in
every lineage with an uncovered write to the file; an import names one lineage or all
(`import_failure(ImportScope)`); a failure's lineages are read from its suspect set rather than
stored; and one resolution applies in every lineage the failure belongs to.

Point 6, ordering per lineage, approved: "point 6: ok!" `Durable` follows its own lineage's
completions; one deliverer serves all lineages; nothing more is promised between lineages.

On point 7 the engineer was less sure: "i think when a client wants no interaction, another
instance is a safe bet." The assistant agreed and corrected its own guidance in DI-D-19, which had
said lineages replace more instances. Lineages separate durability counters, but the lineages of one
instance still share its submission-queue capacity, its completion queue and deliverer, dioring's
lock, the chain's ring-wide barrier, the failure scope of a file they both write, its lifecycle, and
under Model B its thread. Lineages are for independent counters among parties content to share a
ring; another instance is for a client that wants no interaction. Both first-class. DI-D-19,
DI-D-14's marker and point 7's instructions were amended to match.

On "then go ahead given that", the assistant carried out point 7: CONTRACT.md's guarantee sentence,
terms, guarantees 2, 5, 6 and 9 and requirements became per lineage and use "epoch id", DI-D-9 and
DI-D-12 gained amendment markers, and DI-D-14's title was swept. Reading DI-D-12 surfaced a conflict
the assistant had introduced in point 5: it had said a failed flush is a failure in every lineage
with an uncovered write *to that file*, but DI-D-12(b) and the contract make the suspect set every
uncovered write in the *instance*, because dioring cannot see which writes share a device's cache.
As specified, then, one failed flush reaches every lineage with an uncovered write anywhere, and
lineages on separate devices share failures. The assistant left the scope unchanged, corrected
API.md to defer to the contract, and opened DI-2.9 point 8 with three options: keep the
instance-wide default, narrow to the file, or let the consumer declare flush regimes (the parked
DI-6+.2).

The engineer chose: "caller declared fault regimes", then corrected himself: "sorry meant flush".
Recorded as [DI-D-21](../crates/durable-ioring/DESIGN-NOTES.md#di-d-21), caller-declared flush
regimes. The contract's *flush regime* is the physical set of writes one flush failure can affect,
which no one observes; a *declared* flush regime is the consumer's statement of it, which dioring
scopes failures by, and the consumer warrants each declaration contains the physical regime.
Undeclared, the whole instance is one regime. The parked DI-6+.2 graduated to `DI-2.10`, with seven
points as a working position.

On "let us close it", DI-2.9 closed: DI-D-19's final sentence now states the settled specifics, and
the item is archived verbatim.

## Flush domains, and where their keys come from

The engineer asked for a CreateFile that also reports the NVMe device's NUMA node and the volume root
of the flush and fault domains, "not as part of the dioring crate", perhaps in the namespace-request
crate. The assistant noted that crate's D-3 (one entry per Win32 call) and its `-sys` promise of no
policy: the individual calls belong there, the traversal and its interpretation above it. It also
noted that the volume root only approximates both domains: a file flush pushes a whole device's
cache, and what a failure can affect is the physical disk set, which volumes on one disk share and a
striped or Storage Spaces volume spans.

The engineer then set the goal: "identify the common identifier(s) for the flush domain(s) and their
topologies so that as a capability provider, the dioring crate has the needed parameters for the
caller to specify the flush domains for the file", keeping "flush domain" as an abstraction point if
useful. The assistant argued it is: the unit is not always a device -- a disk, a controller cache, a
virtual disk (recursively, its backing file's domains, which covers the engineer's own VHDX layer),
or an SMB share. It corrected DI-2.10's "one regime per file" to **a set of flush domains per file**,
with failures reaching files whose sets intersect, since a striped volume shares fate with each disk
it spans. The keys are opaque and supplied by the consumer, because they name something physical that
several instances must name alike. The identifiers the helper derives: a disk's
`StorageDeviceUniqueIdProperty` (stable, unlike the disk number), the volume GUID path as the hop to
the disks, a virtual disk's backing-file dependency, server and share for SMB, and the NUMA node as
a per-disk attribute rather than part of the identity.

The engineer: "sounds good. so the naive use is just logically a single instance-wide flush domain."
Confirmed: a file declared with no domains intersects every file, so declaring nothing is one domain
for the whole instance. DI-2.10 was reworked around domain sets, DI-D-21 amended, and the Windows
side queued: per-call entries as `NR-2` in `windows-namespace-request-sys`, and the traversal as root
`M39`, whose home is an open decision.

Working through DI-2.10: point 1 was already settled by the engineer's "naive use" remark. On point 2
he chose "the bytes, we will 'intern' it ourselves into a small pointer-sized value for use" -- the
exact identifier bytes, interned by dioring, over the assistant's hashed `u128`. That gives exact
equality, so no collisions, with word-sized comparison after declaration.

Point 3: "specified at registration only". The assistant read "registration" as the file being given
to dioring by either path (`Setup::files` or `add_file`), not only the kernel registration made at
construction, and recorded that reading. The set is then fixed; changing it means removing and
re-adding the file, which `remove_file`'s existing refusal makes safe.

Point 4, the suspect set by intersection with unknown files matching everything: "correct".
Point 5, `ImportScope` as `All`, `Lineage` or `Domains`, without combinations: "looks right".
Point 6, the consumer's warranty that declarations are complete: "true, needs clear documentation".
Written into CONTRACT.md's requirements, with what a missing domain costs, what is always safe,
and an example. Point 7, the API and contract amendments: "good". API.md gained section 13
(`FlushDomain`, `FileOptions`, `FileSetup`, `add_file_with`, `ImportScope::Domains`) and the
sketch was compile-checked; CONTRACT.md's failures section now states the intersection rule.
The engineer: "close 2.10". DI-D-21 gained its settled specifics, and DI-2.10 was archived.

## DI-2.4 revisited after lineages and flush domains

The engineer asked what is left to resolve in DI-2.4. The assistant's list, each with a first
view: where the strategy is chosen (per seal through the options pattern, since the drain flag is
one-sided per D-47 and a chain in one lineage delays only that lineage); the chain's shape (every
flush drain-flagged, so the N flushes serialise); one flush per file regardless of declared
domains; crediting a successful flush's coverage to every lineage and seal it covers; seals that
must wait for gated writes, and the deadlock a gate cycle makes; and the per-epoch file set,
which falls out of the uncovered-write tracking.

The engineer asked whether offering both strategies had been decided. It had not: the record shows
it as the assistant's framing under OPTION INTEGRITY, with the engineer's own stated view having
been "the chain is the only real choice", and the strategies listed as deliberately coarse. The
assistant withdrew "already settled".

The engineer, finding the names uninformative, stated the expectation instead: "if i make a
durability epoch which is separable from another durability chain's flush domains, they will not
overlap on make durable events." The assistant's reading: a seal of one lineage neither waits on
nor is reached by another lineage's work when their files' declared domains are disjoint. The
chain cannot meet that, because its drain flag waits on everything queued earlier on the ring;
host sequencing waits only on the sealing lineage's own writes at or below n, and flushes only
its files. Questions put back: whether the expectation is a guarantee in CONTRACT.md, and so
whether the chain is dropped or kept as an opt-in that waives it for one seal.

On the residual sharing (submission-queue capacity, the deliverer, the lock), the engineer:
"delays are inevitable with shared resources. we should plan for etw logging for delays." Queued
as DI-2.11 (design: which delays, identity in events, cost when disabled, provider mechanics, and
which layer owns each delay) and DI-3+.6 (emit them). No crate in the workspace provides ETW
events yet; the existing ETW work consumes kernel traces.

The assistant had read the expectation as a candidate guarantee for CONTRACT.md. The engineer
corrected it: "they may overlap. i was stating what would happen for a careful consumer who kept
everything very orderly." Restated by the assistant: the question is only whether dioring's own
flush mechanism may add coupling between lineages a careful consumer keeps separable -- overlap
from shared resources stays, and ETW (DI-2.11) shows it.

Offered (a) completion-gated only, (b) completion-gated by default with ring barriers as a
per-seal opt-in, or (c) leave it coarse until measured, the assistant leaning (b). The engineer:
"completion-gated", then "only". Recorded as DI-D-22. Ring-barrier flushes are not offered; the
barrier stays available in `windows-ioring-sys` to anyone using the ring directly. DI-D-19 no
longer lists a ring-wide barrier among what lineages share, DI-6+.1 drops the strategy comparison,
and DI-2.4's remaining points (one flush per file, coverage credit, gated writes and gate cycles,
the per-epoch file set) are written onto the item.

The engineer: "as perf optimizations we may want to note if specialized flush domains are in use at
all, or which are in a given ring so that we may apply a different synchronization strategy
dynamically." The assistant read it as compatible with DI-D-22 once "only" is read as excluding a
consumer-selected strategy, since the mechanism is dioring's (DI-D-8), and wrote that into DI-D-22.
Where the barrier adds no coupling of its own -- no domains declared anywhere, or everything
outstanding intersecting the sealing lineage's domains -- dioring could use it to put the flush in
the same submission as the writes, saving the round trip (a log append and commit on one file). It
still waits on unrelated earlier work and serialises N flushes, so the choice is workload-dependent.
Parked as DI-6+.5, gated on DI-3+.2, with ETW recording which mechanism each seal used.

The engineer: "at least "naive" meaning no files with any flush domains set will be common." Noted
on DI-6+.5: in the naive case one flag decides that the barrier is admissible, so it is the first
case to build and measure, and the choice there is purely one of performance.

On DI-2.4 point 2, the engineer: "why not one per domain? i was thinking that perhaps the flush
domains should come with functors for them to do their flushing, but now its feeling like we should
have flush domain registration apis." The assistant gave the two reasons behind one flush per file.
First, a file flush has two halves: writing back that file's own cached data and metadata, which is
per file, and the device sync. Second, that one file's sync also commits another file's writes is
observed rather than specified -- the ring crate's `FlushMode` documentation notes that nothing in
Win32 even says the modes sync -- so coalescing on it binds to incidental behaviour, and coalescing
on a consumer's domain declaration would make over-declaring unsafe. Registration answers the second
reason: a registered domain carrying its own flush target makes the domain flush specified by the
consumer and warranted like the declaration. The first half stays per file, as `NoSync`
write-backs, then one syncing flush per domain once they complete. The assistant preferred a
ring-issuable target (a handle dioring flushes through the ring) to a functor, which would run
arbitrary and probably blocking code on dioring's thread, outside the ring's completion and failure
paths.

The engineer extended it: "well and there is more. there should be a "begin flush session", then
all the flush domains get notified that they will be asked to flush, then finally a notification to
all that the session should flush. this would allow flush domains which have different names but
which have coupled behaviors to minimize the physical flush count." Recorded as DI-2.12, with DI-2.4
point 2 waiting on it. The assistant reconciled it with its preference for ring-issued flushes by
having the callbacks plan rather than act: on "flush", each domain returns the ring operations to
issue, or none because a coupled domain's operation covers it. Open: one session per seal or
merged (merging only seals whose domains intersect, so separable lineages stay uncoupled), where
the per-file write-backs sit, the new warranties, and the callback rules.

The engineer, on who performs the I/O: "well I thought for some domains the flush might occur
entirely out of band, as for some exotic hardware. When you are saying "in the ring", do you mean in
the windows-ioring-sys ring, which includes synthesized members, or the actual win32 IoRing
processed by the kernel?" The assistant: the kernel's IoRing. It also corrected the premise:
`windows-ioring-sys` has no synthesized members in production. `Completion::synthetic` is
`#[cfg(test)] pub(crate)` by design, because fabricating a completion breaks the argument that a
completion's existence means the kernel is done with the operation, which `claim_if`'s buffer
release relies on; and the external consumer's "synthetic ring" (2026-08-28) was found to be the
root of their concurrency problem. Out-of-band flushes are legitimate, so the working position
widens: a domain answers "flush" with ring operations, "covered by another domain", or out of band
through a completion token dioring hands it, whose outcome enters dioring's core directly rather
than through the ring's completion queue. Open: waking the deliverer on a token's completion, and
dropped or never-completed tokens.

The engineer: "your analysis depends on the current implementation of windows-ioring-sys. If you
want to make a dependency on something ensure it is contractually obligated." Correct: the
assistant had cited a `#[cfg(test)]` gate, and the ring crate's public surface does not promise
that completions are never fabricated -- that is an internal soundness argument. Re-argued on what
is contractual. The token route is dioring's own choice: an out-of-band outcome is a domain flush,
not a kernel operation, so its meaning is dioring's to define, and the ring offers no specified way
to post one anyway. What dioring does depend on -- that each popped completion reports an operation
it pushed, finished by the kernel -- is now queued for the ring crate to state publicly as `M31.2`,
pinned by a `compile_fail` doctest, and added to DI-3+.2's prerequisite with `M31.1`.

The engineer: "is this a real requirement? why?" The assistant checked and withdrew it: the
property is already contract. `try_pop`'s rustdoc hands a payload back by that call alone, and
D-79, stated in `try_pop`'s panics section, makes a completion for any identity not in flight a
defect that panics; what a result means is what popping a completion is, and only the caller's
own fault-injection call rewrites one, to failure only. `M31.2` restated those and its doctest
would have pinned an implementation detail. DI-3+.2 now cites `try_pop` and D-79 as what it relies
on. The engineer: "yes. I don't think we have a specific need for the items in any of these cases
to use the ioring to interact with the kernel. The "filesystem interaction" could be entirely a
fiction that ended up being manifested as a series of udp interactions; still ends up interacting
with the kernel of course and possibly an ioring but not at all in the way you are expecting." So
DI-2.12 was rewritten: dioring assumes nothing about how a domain flushes; each enlisted domain
completes a token by whatever means; dioring's own per-file sync through the ring is one domain
implementation among others, serving undeclared and unregistered domains; a session tells a domain
which files and writes it covers, and per-file write-back is the domain's call rather than a fixed
phase; a write is durable when every domain in its file's set has succeeded; and a domain's success
is a consumer warranty alongside DI-D-21's.

The engineer set the terms for feedback: "so my points are *intended* to reduce coupling and
hopefully make things simpler. When they actually make things more complex, this is where I want
strong feedback so we can hone the solution. It's often very attractive to couple the software
stack top to bottom but then you get a sloppy mess that just needs to be completely rewritten and
you don't even know what really was the defective part."

The assistant's feedback on DI-2.12 as it stood: the goal -- dioring not knowing how durability is
achieved -- reduces coupling, but the shape chosen adds it back. Per-domain participant objects and
a three-phase session make dioring a commit coordinator with a protocol surface that grows with
the number of domains, and coupled domains can only dedupe by sharing state with each other behind
dioring's back. The notify phase exists only because each domain sees only itself. Counter-proposal:
one consumer-supplied durability provider per instance, one asynchronous call per seal carrying the
whole set (domains, their files, the writes), and an outcome per domain. The provider sees
everything at once, so dedupe is internal and needs no phases or shared state; domains go back to
being DI-D-21's failure-scope keys, and any per-domain attributes live in the provider rather than
in a registration API on dioring; dioring's per-file sync through the ring is the default provider,
so the naive consumer never meets it. What stays irreducible is the out-of-band completion, a second
wake source under Model A. And a boundary: if the writes themselves stop being ring I/O, that is not
dioring.

The engineer liked it -- "it's what I expect dioring to be on top of the wioring (a name I just
synthesized for our windows-ioring-sys ring)" -- and asked whether "the writes involved" meant ring
positions or the provider issuing writes. Neither: by the time of the call dioring has issued every
covered write and observed its completion, which is the call's precondition; ring positions are
gone and are dioring's private use of the wioring (DI-D-10). The provider makes completed writes
durable and never carries their data; it cannot push to dioring's wioring, only to its own.

The engineer: "given that the windows apis don't allow for flushes on the ioring I think that the
limitations are fine. (this is true, right?)" Not true: IoRing has `IORING_OP_FLUSH`
(`BuildIoRingFlushFile`, with flush modes), the wioring exposes it, and DI-D-22's completion-gated
flushes are ring flushes. What IoRing lacks is a range flush, device commands and network
operations, so exotic and UDP providers cannot be ring-based anyway. The limitation stands on
DI-D-10's grounds instead. Asked whether extents or identities are needed: dioring already mints a
per-write identity (`OpId`, carried in `SuspectWrite`), and after issuing a write dioring itself
uses its extent for nothing but the report. Proposed: the provider call carries domains, files and
`OpId`s only -- the provider is consumer code and can map an identity to whatever it keeps, and the
retention layer keys by identity too. Whether `SuspectWrite` keeps `offset` and `len` is a separate
question put to the engineer.

The engineer: "drop the extents. The consumer may track them themselves via the OpId. Noted that
the IoRing may carry flushes." `SuspectWrite` lost `offset` and `len` in API.md, CONTRACT.md's
failure section says the extent is not reported, and DI-D-6 ("an identity per write and nothing
more") records it as applied. DI-2.12 was restructured around the durability provider, with the
earlier session protocol kept here as history; DI-6+.5's dynamic barrier applies to the default
provider only.

The engineer, on DI-2.12 point 1: "I agree we have a default, I hope we can do a little better
than per-file sync. Do we only have the file handle or do we have a file name also?" Handle only:
`FileSetup` and `add_file_with` take a `FileKey` (the consumer's `u64`) and a `SharedFile` (an
`Arc<OwnedHandle>`). The assistant argued a name would not help -- a handle identifies the volume
more reliably than a name, which hard links, renames, mount points, junctions and redirectors make
ambiguous -- and listed improvements on documented ground, checked against Microsoft's
`FlushFileBuffers` page: skip files opened with `FILE_FLAG_NO_BUFFERING` and
`FILE_FLAG_WRITE_THROUGH`, which the page says flush metadata with each write; one flush per volume
through a volume handle, which the page documents but which needs administrative privileges and
flushes every open file on the volume, other processes' included; a cheaper flush mode per file;
and coverage credit. Two facts left unverified: a documented way to query a handle's open mode, and
whether IoRing's flush accepts a volume handle.

The engineer: "oof I didn't realize that the volume flush required admin privileges. write through
and no buffering are good for the filesystem cache but don't issue the flush at the device level,
and fua is largely ignored nowadays, with volume flush, or really any flush, taking its place. I
think you are right and we should record these points as optimizations we should strongly
consider. I think that for the case of for example a database where they were careful to use the
write-through / no buffering on the file handles, they can provide us with the metadata to inform
us of this and we can work to ensure one flush per physical device since they have guaranteed the
cache manager's profile is handled." The assistant accepted the correction -- skipping such files
would lose the device sync -- and recorded the plan with one adjustment: that a syncing flush on one
file commits the device cache behind the other files is not documented by Windows, so it becomes
part of the consumer's warranty for the flush domain rather than an assumption of dioring's, and
"physical device" is expressed as the flush domain the consumer already declares.

The engineer: "I agree that the scope of the flush is not 100% clear but I don't want to resolve it
now. this is a performance feature that we will tackle during development and if we can't make it
safe, we will omit it. It is not required for shipping." The optimizations moved out of DI-2.12 into
a parked item, DI-6+.6, gated on DI-3+.2, with the safety question named as what decides inclusion.

The engineer: "point 2, this is a design point on flush domains now right?" Yes: with the provider,
dioring's core issues no flushes of its own, so per-file or per-domain is the provider's choice under
its warranty; the default provider syncs each file and DI-6+.6 holds its per-domain variant. The
concern behind the point -- dioring must not coalesce on declared domains by itself, or
over-declaring stops being safe -- holds because only a provider ever does. DI-2.4 point 2 closed as
moved to DI-2.12.

On point 3 the assistant framed the question -- does a provider call name exactly the sealing
lineage's writes, or every completed, not-yet-durable write on those files so its success can be
credited to later seals -- leaning to the second. The engineer: "well this is a case where we have
to talk about contractual behavior vs. optimizatioons. contractually, what is guaranteed is that the
series of writes in the lineage. If additional writes are written, that's not incorrect, that's a
performance issue." Settled that way: the contract is the minimum, and CONTRACT.md's "Nothing about
open epochs" became "Nothing about writes outside the seal", covering other lineages' writes too.
Widening and crediting was parked as DI-6+.7, with the rule that only writes a call named are ever
credited.

On point 4 the engineer: "I think we have to detect it. The alternative leads to a lot of extra work
around enumeration, modification and cancellation of lineages that I don't really want to have to
do. We may need to implementation cancellation just in general but only because somewhere along the
line the holder of a lineage may decide not to finish it for whatever reason." Recorded as decided.
The assistant proposed the mechanism: a gated write `L:a` gated on `M:b` is the dependency "L
reaching a needs M reaching b", a cycle exists when M reaching b already needs L reaching a or
beyond (durability being a prefix), and the push that would close one is refused before anything
is reserved, naming the cycle. It noted the limits: only gates dioring sees are covered, and a
lineage whose holder walks away strands other lineages' gates on it, which detection cannot see.
That became DI-2.13, hedged as the engineer hedged it.

On point 5 the engineer: "yes this seems correct is there something subtle here?" -- then, after the
assistant misread it as about point 4: "sorry this is regarding 5", and "point four had aleady been
closed, I was approving five". Point 5 approved. The subtlety the assistant raised: CONTRACT.md's
"Covered" is physical (a flush issued so it cannot complete before the write), which the provider
no longer lets dioring know; proposed instead, a write is covered when named in a provider call made
after its completion was observed, and covered successfully when every domain of its file reports
success, so one definition drives the seal's file set, suspect sets, `FileBusy` and crediting. Queued
as DI-2.12 point 9. Also recorded on point 4 for the implementation: a heal takes effect at the first
seal after it, so reaching a failed epoch can depend on reaching a later one.

The engineer: "close di-2.4 please". DI-D-23 records the gate-cycle decision (DI-D-22 already held
the strategy), and DI-2.4 was archived verbatim with anchor `di-24`.

On DI-2.11 the engineer, keeping detail for later: "for locks, instead of simply taking the blocking
lock we would establish a pattern where we would always do the "try-lock" (attempt to take the lock
without blocking) and then if that were to fail, capture a timestamp (interrupt time for
efficiency), take a blocking lock, and after acquiring the lock, log the etw event with the delta in
timestamp. So the fast path stays fast, the contended path logs an event." The assistant agreed with
the shape and gave five notes: a precise timestamp is affordable on a path about to block, whereas
interrupt time advances per clock tick and would read most short waits as zero; emit after
releasing, so the event does not lengthen the hold it measures; one wrapper type, a name per lock;
poisoning is a separate failure of try-lock; and the shape fits the other waits too. The existing
`trace_record!` debug trace stays separate from ETW.

The engineer: "all agreed, we can consider the time base later." Recorded on DI-2.11 as agreed, with
the time base as its own open point 6.

On DI-2.5 the engineer: "I would prefer a trait. What challenges are posed by this?" The assistant
listed them: identity types that only dioring can mint (`Lineage`, `OpId`, `FailureId`,
`FailureToken`), `dyn` versus generic, optional capabilities such as registered buffers, the front
ends, the contract binding other implementations, and evolution of a public trait. The engineer:
"1: yes associated types / 2: yes generic only. We will make things free functions when needed / 3:
agreed, I assume you can do this / 4: what's the question? / 5: yes". The assistant restated 4: front
ends need `pop` and a readiness signal; written once over the trait, they need the trait to carry
that signal, which, being set by any source of entries, would also answer DI-2.12 point 4's waking
question. Recorded on DI-2.5 with 1, 2, 3 and 5 decided and 4 open.

The engineer: "Regarding point 4, why wouldn't it have such a signal?" The assistant: no good
reason; the alternative was listed for completeness, and the case against is a test double keeping a
signal nobody waits on. The real cost is specifying it: the wake-up rules -- when the signal is set
relative to `pop`, and what a front end does before waiting again -- must be contract, as the
wioring's drain-rearm-drain lesson (M4.2) shows. Point 4 recorded as decided; the form and rules
became DI-2.14, and DI-2.12 point 4's waking question now points there. Point 6 (evolution) marked
as a consequence rather than a decision.

The engineer: "yes please close it". DI-D-24 records the trait decision; DI-2.5 was archived verbatim
with anchor `di-25`; restating API.md as the trait was queued as DI-2.15 and the conformance oracle
as DI-3+.7.

The engineer: "is there anything to talk about with di-2.6?" The assistant compared the wioring's
operations (`Nop`, `Read`, `Write`, `Flush`, `RegisterFiles`, `RegisterBuffers`, `Cancel`) with what
API.md offers and found four points: that no consumer flush is offered, durability coming only
through a seal and the provider; that `WriteOptions` carries only a gate, so the per-write
write-through flag the wioring exposes (its D-25) is hidden, narrowing the platform; whether a
mediated marker entry -- a consumer-posted entry in dioring's queue, the `Nop` analogue -- is wanted;
and that cancel stays parked, distinct from DI-2.13's lineage-level question.

The engineer: "1 and 2: agreed. 3: I assume the use of it would be the next layer up. I don't know I
don't see the use but if it can be a sequencing point that the consumer can depend on that only
sequences i/o submissions (or completions?) rather than durability maybe it has utility? 4: I still
don't think we want to get in the business of cancelling individual I/Os. I believe that entire
larger structures may be abandoned but not surgical I/O cancellation. I may be proven wrong in the
long run but right now I just can't see it." The assistant applied 1 and 2 to API.md --
`WriteOptions::caching(WriteCaching)`, dioring's own enum so other implementations need nothing from
the ring crate, and a statement that dioring offers no flush. On 3: submission order already exists
as `OpId` order, and a completion fence is meaningful, deliverable by dioring from its own
bookkeeping rather than a ring-wide drain, and scopable to a lineage or files; parked as DI-6+.8.
On 4, recorded on DI-2.6; DI-6+.4 stays parked.

The engineer: "close it please". DI-2.6 archived verbatim with anchor `di-26`; no decision row,
since point 1 restates what DI-D-10 implies and point 2 lives in API.md section 11.

On DI-2.7 the engineer: "1: yes. 2: yes. 3: yes. 4: yes. 5: yes. 6: yes. if that's everything can we
close it?" The assistant re-read the item against the day's decisions and found two things before
closing: point 3 is an API addition (`close()` returning held operations, unresolved failures and,
under lineages, a high-water mark per lineage), which API.md does not yet have; and the durability
provider opened a gap no point covers -- provider calls in flight at drop. Proposed as point 8:
dioring adds no blocking, the instance drops its provider so waiting is the provider's own `Drop`, a
late completion is a no-op, and the named writes are simply never reported durable.

The engineer: "1: add close here also. 2: the responsibilities w.r.t. outstanding calls to
durability providers are similar to the responsibilities to around outstanding ring operations. so
in the end we have to await them. perhaps there should be a cancellation notification but I doubt
it's worthwhile. shutting down a lineage does not have to wait because there is not a lifetime issue
and there is not a dependency around lifetime management." Then: "I'm sorry the last point should
have been around durability". The assistant checked the ring's parity first: `IoRing`'s drop runs
`run_down`, which waits while anything is outstanding and leaks rather than frees only if the
kernel refuses -- so awaiting provider calls is the same responsibility, as the engineer said. Added
`close(self) -> Leftovers` (held operations, unresolved failures, every lineage with its final
high-water mark) to API.md, compile-checked; CONTRACT.md gained "Ending an instance" in place of its
"not yet specified" entry; DI-D-25 records it, including that no cancellation notification is
scheduled and that the wait must not hold dioring's lock. The lineage point was recorded on DI-2.13.

On DI-2.11 the engineer: "I have two major concerns for delays: flushes and locks. Locks are in our
control and flushes are up to the flush domains control." The assistant applied the split by
control: dioring instruments its own lock fully; the wioring's lock is the ring crate's to instrument
the same way; flushes are measured at the provider boundary -- call issued and each domain's outcome,
so the event names the slow domain -- with the provider's internals its own to report. It noted one
gap: a seal's wait for covered writes to complete and for gated writes to be released is neither a
lock nor a flush, and asked whether it gets a secondary event. Recorded on DI-2.11 points 1 and 5.

The engineer: "I think it's very unlikely to warrant one but I'm ok with it." Recorded with the
hedge: a secondary seal-phase event, doubted. The ring crate's share -- timing `EventDelivery`'s
mutex with the agreed pattern -- was queued there as `M31+.1`, parked behind DI-2.11.

The engineer, on DI-2.11's remaining points: "2: I think it's going to depend on the event. We don't
have to design this now. 3: nothing. 4: manifest. 5: we are generating events about durability
delays, so we own it. 6: let's see what the timelines look like before we decide". Recorded: 2 and 6
moved to DI-3+.6 with the provider's name and GUID; 3 means no clock read and no event built when no
session listens; 4 a manifest provider, with the consequence noted that events decode by name only
where the manifest is installed, an administrative step an application must take or document. On 5
the assistant read dioring as owning every durability-delay event, measuring what it calls at the
boundary, and asked whether the ring crate's `M31+.1`, queued a turn earlier under the split by
control, stays as that crate's own instrumentation or is withdrawn.

The engineer: "the etw events we publish give a surface for diagnosis of issues; clients don't want
to search around across providers to try to correlate issues." So one surface, dioring's: `M31+.1`
was withdrawn, and the wioring's lock contention appears as a slow call into the ring, timed by
dioring at the boundary. Every DI-2.11 point is now settled or moved to DI-3+.6.

The engineer: "yes" (close it). DI-D-26 records the ETW decisions; DI-2.11 was archived verbatim
with anchor `di-211`.

On DI-2.12 the engineer: "now let's talk about di-2.12. one provider per instance seems too
restrictive". The assistant agreed several providers are natural and put the question as where
routing lives: (a) in dioring's core, domains registered against providers and one call per provider
per seal, or (b) a routing provider dioring ships, implementing the provider trait over providers
registered by domain, leaving the core single-provider. It preferred (b): the same capability
without growing the core, routing as one replaceable component whose faults are its own, and the
natural home for the engineer's earlier "flush domain registration apis". Constraint either way:
coupled domains dedupe only within one provider, so splitting them is correct but costs flushes.
Recorded on DI-2.12 point 1 as open.

The engineer asked for an example of the difference. The assistant used a database with a log on
local NVMe (`nvme0`), data striped behind one RAID controller cache (`raid-a`, `raid-b`, coupled), and
an archive on an SMB share (`share1`): under (a) the instance registers `raid` and `smb` and splits
each seal into three calls, its contract growing to cover registration conflicts and removal; under
(b) a `Router` built up front is the instance's one provider, refusing conflicts at build time and
testable alone. The example exposed a wrinkle in (b): the default provider alone may use dioring's
wioring, so a router falling back to it would need ring access. The clean form keeps the default in
the core, which routes exactly one way -- built-in default or the one consumer provider.

The engineer: "do your clean form of b". Recorded on DI-2.12 point 1: the core holds the built-in
default and at most one consumer provider, which states at construction the domains it serves;
everything else goes to the default. The router dioring ships was queued as DI-3+.8.

The engineer: "can you please just take initial stances on them" (DI-2.12 points 2-9). The assistant's
stances, recorded on each point as not decided: one request per seal with a completion handle per
domain; the provider may hold `SharedFile` clones, hence DI-D-25's wait; a dropped handle is a
failure of its domain, a never-completed call leaves the seal pending with no timeout in dioring,
and completion only sets the readiness signal; a domain's failure reaches files containing that
domain plus unknown files, narrower than DI-D-21's rule and sound because the provider warrants per
domain; a domain's success covers exactly its named `OpId`s; calls are non-blocking, handles complete
from any thread, the provider cannot re-enter, and its panics propagate with dropped handles counted
as abandoned; the router merges nothing; and "covered" is redefined as proposed, in one change.

The engineer: "seems right, let's move on". Points 2-9 recorded as approved; DI-D-27 records the
provider, with amendment markers on DI-D-9 (rule (c)'s "covered") and DI-D-21 (per-domain failure
scope); DI-2.12 archived verbatim with anchor `di-212`; writing it into CONTRACT.md and API.md queued
as DI-2.16.

The engineer: "for di-2.14, is there anything to resolve here?" The assistant checked the wioring's
completion event first: ring-owned, auto-reset, edge-triggered on empty to non-empty, one waiter, a
duplicate handed out (D-19 to D-21), and its documented example sets the duplicate by hand as
"always safe" under rule 2. Stances: the trait's signal has the same form and rules; dioring's
signal is the wioring's completion event itself, set by dioring when a provider completes; and
conformance is a harness check beside the oracle, which cannot see wakes. The one thing to resolve:
with front ends written over the signal, dioring's Model A front end is its own thread-pool wait,
so dioring stops using `EventDelivery`, DI-D-18's binding to its callback contract lapses for
dioring, and ring `M31.1` stops being a prerequisite of DI-3+.2.

The engineer: "relay to a second event". Recorded on DI-2.14 point 5 as decided, over the
assistant's stance. The assistant acknowledged the merit it had underweighted: dioring's signal keeps
semantics dioring defines, and the ring's arm-and-drain discipline stays in the ring crate, where
M4.2 and M26.12 already paid for it; setting the ring's own event by hand had tied dioring's
contract to the ring's. So dioring owns an auto-reset event set by its `EventDelivery` callback and
by provider completions; DI-D-18 and M31.1 stand; dioring's callback assumes no order among
overlapping callbacks; and in both models the ring is drained by pool callbacks, the models differing
only in who delivers.

The engineer approved points 1-3 ("yes"). DI-D-28 records the signal; DI-2.14 archived verbatim with
anchor `di-214`; stating the rules in CONTRACT.md and the signal in API.md added to DI-2.16, and the
harness check to DI-3+.7.

The engineer: "go ahead with di-2.16". The assistant wrote the provider and the signal into
CONTRACT.md (new terms, "How a seal is made durable", per-domain failure reach, the provider
warranty, "The readiness signal") and API.md (sections 15 and 16; in the sketch `DurabilityProvider`,
`FlushRequest`, `DomainCompletion`, two new `Cause` variants, `Setup::provider`, `readiness()`),
compile-checked. The sweep found two older restatements the item had not listed: guarantee 4 and
DI-D-13 promised every request "a prompt answer ... never a silent stall", which DI-D-27's
no-timeout rule contradicts -- reworded, with an amendment marker on DI-D-13; and "No bound on what a
commit waits for" still described the ring barrier DI-D-22 removed. DI-2.16 archived with anchor
`di-216`.

The engineer: "go ahead with di-2.15". Restating the sketch as the trait hit a wrinkle DI-D-24 had not
seen: it kept `Epoch` a shared plain value, but an epoch holds a lineage, so once the identities are
associated types the shared types cannot stay concrete. The assistant bundled the identity types and
the epoch-id type in one trait, `Identities`, implemented by a zero-sized marker (`DioringIds<E>`),
and made every identity-carrying shared type generic over that one parameter; one parameter per
identity would have given `Entry` seven. `DurableRing` became the trait, with `RegisteredBufferRing`
the extension and `Dioring` dioring's implementation; shorthands (`Lin<D>`, `EntryOf<D>`,
`PushResult<D>` and others) and a generic `seal_log` show a consumer written once against the
contract. Compile-checked clean. Recorded as a refinement on DI-D-24; DI-D-15, DI-D-17 and DI-D-18
gained markers (the rename, and DI-D-18's stale "ring's own completion event"). Open, as DI-2.17:
the core trait still names the ring crate's `SharedFile`, so every implementation depends on the
ring crate for it.

The engineer on DI-2.17: "is there a canonical file type that provides a robust set of standard
features we should use? Make a list of what features such a type would have and record it in one of
the design notes. I will assume we don't have one to use so introduce one that's private to the
crate." The assistant checked: `std::fs::File` owns but is not cheap to share; the ring's
`SharedFile` is the shared form but exposes no handle, which would have left a durability provider
unable to flush out of band -- a gap in DI-D-27 nobody had noticed; the namespace crate's
`CapturedHandle` owns a duplicate. So none fits. `DurableFile` was introduced, `pub` because the
public trait names it, its representation (`Arc<OwnedHandle>`) private. Its eleven features are
recorded in DI-D-29. Handing the ring the same handle needs the ring crate's `M31.3`
(`From<Arc<OwnedHandle>> for SharedFile`), queued and added to DI-3+.2's prerequisite; DI-D-4 and
DI-D-27 gained markers. Sketch compile-checked clean; DI-2.17 archived with anchor `di-217`.

On DI-2.13 the engineer: "is seems like almost all of this is straightforward. What do you need
answered?" The assistant agreed: point 3, the hard part, falls out of guarantee 9 and DI-D-23 once
ending abandons the lineage's epochs, and point 4 resolves to "ending cancels no I/O". Six questions
remained, recorded on the item with stances: in v1 (yes); abandon every not-yet-durable epoch, sealed
ones included; ending also retires, the handle refused at once; a failure shared with another
lineage stays unresolved for it; a new `LineageEnded` entry; and the default lineage cannot be ended.

The engineer: "close it and go ahead and end the milestone". DI-D-30 records the six stances as
decided; CONTRACT.md gained "Ending a lineage", a fifth synthesized event, and an empty "Not yet
specified"; API.md gained section 18 (`end_lineage`, `EndLineageError`, `Entry::LineageEnded`),
compile-checked; DI-D-13 and DI-D-15, which counted four events, gained markers. DI-2.13 archived
with anchor `di-213`, and DI-M2 closed: its stubs moved to the archive, the preamble and both PLANS
rows updated. DI-M3+ keeps its `+` IDs; graduating them to DI-M3 is left for when that milestone is
taken up.

## The shared handle moves into its own crate (2026-10-06 and 07)

In conversation, the engineer asked whether the better answer to DI-2.17 was a better "handle" type
rather than a crate-local file type. The assistant agreed: the same `Arc<OwnedHandle>` had been
re-wrapped crate by crate (`SharedFile`, `DurableFile`), each with an incomplete surface. Along the
way: `windows-sys` publishes only the raw `HANDLE` alias; std's `std::os::windows::io` already owns
(`OwnedHandle`) and lends (`BorrowedHandle`) handles, so no workspace-wide owning wrapper is wanted
-- handles closed other than by `CloseHandle` keep their own owners -- and the one gap is shared
ownership. One type, not two; a file-specific second type would be worth it only to prove a file was
opened overlapped. The engineer named the crate ("I prefer win-shared-os-owned-handle") and the type
("how about just SharedHandle"), and had it built on its own branch off `main`. It merged with the
dependabot fix, and release-please released it as 0.1.0 through the `Release-As` footer.

"definitely rebase or merge origin/main and let's continue on": merged (the branch is pushed with
many commits, so a rebase would have forced a push), with one conflict in the root PLANS.md, where
both sides had added a tracker to the head of one list. Then: API.md's sketch uses `SharedHandle`
with one private `ring_file` bridge, compile-checked against both crates; DI-D-29 is revised to name
it, and rewritten as a short row with a readable section, after the engineer found the dense row
unreadable; the ring crate's M31.3 becomes "`SharedFile` adopts `SharedHandle`", as an alias, which
is expected to be source-compatible; DI-3+.1 notes the new dependency; and the new crate's checklist
records publication as done and where adoption is queued.

The engineer: "do the renaming to start with". DI-M3+ graduated to DI-M3 and its items from
`DI-3+.n` to `DI-3.n` in every live document; the archives and this record keep the old IDs, and a
note under the new heading maps one to the other.

## DI-3.2 is split, and two dependencies are brought forward (2026-10-07)

With the merge of `main` (the ring taking `SharedHandle`, M31.3 done) and DI-3.1's scaffold in
place, the engineer asked whether DI-3.2 could be implemented. The answer given: yes, once ring
M31.1 lands -- DI-D-18 binds the front end to a callback contract that today holds only as
implementation -- and once DI-3.2 is split, because as one item it covered the whole contract and
two of its requirements sat on items numbered after it: its worked-example doctests on DI-3.3's
fault injection, and DI-3.7's conformance oracle, to be built "alongside" it.

The engineer: "do the rewrite of the milestone item first, now". DI-3.2 became DI-3.2.1 to
DI-3.2.7 -- types, trait and construction; plain I/O and the Model A front end; seals in one
lineage; failures and resolution; lineages, gates and flush domains; the consumer provider; ending
-- each growing the trait by what it implements, with the M31.1 prerequisite on DI-3.2.2 alone.
The two dependencies were left open for the engineer, each with two options.

The engineer chose "the earlier option" for both, read as the option that lands sooner:

- **The worked-example doctests** land in DI-3.2.4, on a fault seam built there and reachable from
  doctests, rather than waiting for DI-3.3.
- **The conformance oracle** is folded into DI-3.2: DI-3.2.2 builds it, with the readiness
  signal's harness check beside it, and each later step adds its rules. DI-3.7 is kept as a
  checked record pointing there.

## The readiness event needs a safe event, and gets a crate (2026-10-07)

Starting DI-3.2.2 surfaced a gap: DI-D-28's readiness event must be created and set inside a crate
that forbids `unsafe`, and nothing below dioring offered a safe event that could be set.
`windows-threadpool-sys` creates one safely (`WaitableHandle::event`) but only as a wait target to
hand to the pool, with no way to set it; the ring crate and `windows-waitable-queues` each make a
private one.

The engineer asked first why a new type was needed at all, given `windows-sys`, the `windows`
crate and std's `OwnedHandle`. The answer: nothing about the event differs; what is missing is a
type whose invariant lets `SetEvent` be called safely. Both binding crates expose it as an
`unsafe` function over a raw handle, and `OwnedHandle` knows the handle is valid but not that it is
an event, so it has no `set`. The `unsafe` has to live somewhere, and the question was only where.

Asked to place it, and not to keep adding to the thread-pool crate, I proposed `win-sync-sys`
covering Win32's synchronization area, sorted Learn's fourteen groups into kernel objects (the
core), in-process primitives (a pinning problem of their own) and groups belonging elsewhere, and
asked four questions. The engineer's answers:

- the name `win-sync-sys`;
- omit the in-process primitives to start with;
- start with `Event` only;
- make it work well with `windows-threadpool-sys`, changing that crate as part of this work;
- omit `PulseEvent` and document why.

Named objects were not answered and stay open. One thing surfaced while planning that the proposal
had stated loosely: what makes `set` safe is ownership, which `OwnedHandle` already gives; that
the object is an event is what makes it correct. Event-ness becomes a safety property only at the
thread pool, which treats a wait on a mutex as undefined -- so adopting an existing handle as an
`Event` must be `unsafe`. Recorded as [WS-D-4](../crates/win-sync-sys/DESIGN-NOTES.md#ws-d-4),
with the rest in that crate's [DESIGN-NOTES.md](../crates/win-sync-sys/DESIGN-NOTES.md).

## DI-3.2.2 is implemented in part, and split (2026-10-07)

Implementing DI-3.2.2 surfaced that it held two things of different readiness. Plain I/O with the
delivery every front end shares was fully specified by DI-D-18 and DI-D-28, and was built as
`DI-3.2.2.1`. The Model A front end type was decided in behaviour by DI-2.3 but never sketched as an
API -- no name, constructor, choice of which `&mut self` operations it offers as `&self`, or
teardown shape -- so it became `DI-3.2.2.2`, with those questions listed for the engineer rather
than answered in code.

Three things were decided while building the first half, each recorded where it binds rather than
here:

- `readiness()` returns the `win-sync-sys` `Event` rather than an `OwnedHandle`. This was the purpose
  of creating that crate: the readiness check, and any Model A front end, must wait on the signal
  through a thread-pool wait, and only an `Event` can be handed to one without `unsafe`. DI-D-28
  carries the amendment.
- The gate setters are withheld until gates are honoured (`DI-3.2.5`), so a gated operation cannot
  be issued ungated in the meantime.
- The conformance oracle is a public module, `durable_ioring::oracle`, with the readiness check
  beside it.

One measurement came out of the caching option's test: `IoRing`'s write-through flag fails with
Win32 error 509 on any handle opened for cached I/O and succeeds on an unbuffered one. The ring
crate's documentation never said so; it is queued as that crate's `M32.1`.

Asked to suggest a name and constructor for the Model A front end, I proposed `EntryDelivery<D>`,
named as the layer-up counterpart of the ring crate's `EventDelivery`, with
`EntryDelivery::new(ring, on_entry, env)` taking the instance by value, a `FnMut` handler because
delivery is serialised, a non-owning `DeliveryHandle` given to the handler so it can call back in,
and a refusal handing back the ring and the handler. `PoolDelivery` was offered as the alternative
naming the mechanism, and `Fn + Sync` as the alternative that would keep room to relax the
serialisation. The engineer: "write it in". Recorded as DI-D-31. Which operations the owner and the
handle offer, and teardown, stay open under `DI-3.2.2.2`.

The engineer then asked for `DI-3.2.2.2` to be implemented with those two still open. I decided them
on analytic grounds and recorded them as DI-D-32 for confirmation: the handle mirrors the trait's
consumer operations as `&self` methods, because a closure over `&mut D` is the ring crate's D-43
defect at this layer -- it lets the instance be replaced under an armed wait -- and would also hand
out `pop` and `readiness`; and ending quiesces the delivery first, with `into_inner` returning the
instance so it can still be closed. The mirroring has a standing cost -- each later step must extend
the handle -- which `DI-3.2`'s parent item now states.

## DI-3.2.3: seals in one lineage (2026-10-07)

The engineer asked for `DI-3.2.3` to be implemented. Its decisions were already made (DI-D-9's
seal, DI-D-22's coverage by observation, DI-2.3 point 10's lock order); what was left was the
mechanism, which I recorded as DI-D-33 for confirmation. The shape that fell out: one lineage's
durability is a pure state machine under dioring's lock, and the delivery callback, already
recording completions under that lock, also pushes the flushes a completion makes due. D-83 is what
makes the second half legitimate -- the callback may open a scope and submit -- and was landed in
the ring crate first for that reason.

Two things came out of building it that were not in the plan:

- **Dropping the instance from a callback would deadlock.** The callback needs the ring to push
  flushes, but the ring's delivery owns the callback, so the relay can hold only a `Weak`. If a
  pool thread held the last strong reference when the instance was dropped, `EventDelivery` would
  be dropped inside its own callback and wait for itself. Upgrading only under dioring's lock, and
  closing the core under it before the drop, rules that out structurally.
- **A sabotage was caught by a crash rather than a test.** A write let past the seal tripped a debug
  assertion after the ring already held the push, and the unwind through an unsubmitted batch
  aborted. The check now also runs before anything reaches the ring. It is a reminder that an
  assertion's position relative to the ring's batch decides whether a defect fails cleanly.

One question has no answer in the contract: what `epoch_state` reports for an epoch of a lineage the
instance does not have. Every `EpochState` value would be false, so dioring panics, documented on the
method. It is queued for the engineer under `DI-3.2.5`, where a retired lineage raises it again.

## DI-3.2.4: failures in one lineage (2026-10-07)

The engineer asked for `DI-3.2.4` to be implemented. DI-D-12 had settled what a failure is and how
it is resolved; what was left was the mechanism, recorded as DI-D-34 for confirmation. The ring
crate's fault seam turned out to be enough to fail a flush, because it transforms the very
`Completion` dioring's callback receives, so dioring's seam is a thin layer over it.

Decisions taken in the code, each because the contract's sentence admitted more than one reading:

- **"The first seal made after the heal"** is read as the first such seal whose flushes all
  succeed. Read strictly, a heal whose seal failed would never take effect, and its failure would
  stall the mark for good.
- **A pending heal holds the token.** The contract does not say whether a healed-but-not-yet-
  effective failure can have its token taken again; answering no keeps "at most one live token"
  true without a second resolution path.
- **Flush-domain reach was built here, not in `DI-3.2.5`.** The suspect set is defined in this
  step, and an instance-wide interim is not merely imprecise: an abandon over it abandons epochs no
  failure put at risk.

Two things the contract text and the code disagreed on. The many-to-many example's second ending
said the mark "moves past" the abandoned epochs; the contract's own resolution rule says an epoch
passes only once every failure containing it is resolved, and F1's heal is not in effect until a
seal. The example was corrected, not the rule. And the healing trace shows one `Durable` where
dioring appends one per seal; the compiled example asserts the outcome -- durable through 42, 41
durable -- rather than the count.

Building it found a weakness the earlier steps had left: a debug assertion in the durability core
fires on the delivery callback's pool thread, where a panic aborts the whole test process. Two old
sabotages were being scored as caught only because the process crashed. The core now records the
inconsistency and `pop` asserts on it on the consumer's thread.

One question is left for the engineer under `DI-3.2.5`: whether a write to an open epoch that was
already abandoned should be refused, as a gate on one is.

The engineer answered at once: "yes a write to an epoch that has been abandoned should fail".
Recorded as DI-D-35 and implemented as `DI-3.2.4.1`. The one choice left to the code was the
refusal's shape: a new `EpochAbandoned` variant rather than reusing `Sealed`, which would misstate
why an open epoch is refused; and `Sealed` checked first, so a sealed abandoned epoch keeps the
answer it already had.

The engineer then asked what DI-D-32 was asking them to ratify, finding the row too dense to tell.
Restated as two questions -- mirrored `&self` methods on the handle rather than a closure lending
`&mut` to the instance, at the cost of keeping the handle in step with the trait; and ending by
stopping delivery first, with `into_inner` returning the instance and its undelivered entries --
the engineer confirmed both: "yes mirrored", and of the second, "I don't see how else it could
work". The row's enumeration of mirrored methods, stale after two steps had added to it, was
replaced by the rule it follows and a pointer to the code.

DI-D-33 was restated the same way, as four choices: the delivery callback pushes the flushes a
completion makes due; a seal waits for its writes in flight and then flushes each file once; one
`Durable` per seal rather than one coalesced entry; and an I/O-free core. The engineer confirmed
all four ("all is true"), and asked whether "each file" needed to map to flush domains or durability
providers. It does not for the built-in default: a flush in `IoRing` is a file handle's, so a file is
the default's unit, and flush domains scope failures rather than units of flushing. A consumer
provider is asked per domain instead (DI-D-27, built in `DI-3.2.6`), and a file can be answered for
by both. The row now says so.

Walking DI-D-34, the engineer stopped at "a write that already completed as failed isn't suspected":
if a write fails, it didn't happen. That was the intent; the real question was narrower. A suspect
set is frozen when its failure is observed, so a write in flight then is in it, and if that write
then fails, nothing in the set records it. The engineer's answer: the set needs a "nullifier" item
marking that the write didn't happen. Asked whether that should change what holds the high-water
mark -- after it was clarified that "blocking" means only the reporting of durability, not I/O --
the engineer set the purpose: "to enable after the fact reconciliation of the history based on
write-only markings, so it's not that it affects the mark per se but it affects the way that
bookkeeping is reported". Recorded as DI-D-36 and queued as `DI-3.2.4.2`, with its shape still
open.

Offered four shape questions -- where markings are read, short writes, other kinds of marking, and
records surviving resolution -- the engineer answered that all of them are useful and should be
allowed for, and named the problem being solved: a failure arrives as "an important record of
information which also carries unique important data (timestamp, error code)", and the design had
nowhere to keep it. That makes the marking a carrier of the completion's data, not just a flag. Left
open: which clock the timestamp comes from, whether `Failed` itself is timestamped, and how one
`io::Error` reaches both the completion and the marking.

The engineer settled all three. The clock: "we need to have a uniform answer for time base; at this
time we're using the interrupt time since it's extremely cheap to get and we don't have any
apparent requirement for higher resolution" -- recorded as DI-D-37, which also closes `DI-3.6`'s
time-base question. `Failed` timestamped: "yes definitely". The error: "the error code should be
part of the CQE or general failure asynchronous or not. capture it. Use whatever is natural." One
blocker surfaced: dioring forbids `unsafe`, and the only interrupt-time wrapper in the workspace is
`windows-threadpool-sys`' crate-private one, so where a safe public read lives is a placement
question for the engineer.

## A clock the core is given (2026-10-08)

Implementing WT-2.2 -- dioring's first timestamp, the observation time on a `Failed` entry -- the
assistant first gave the core a function pointer for its clock, keeping it free of I/O. The
engineer: "The core should be generic over a clock and should be able to take a 'mock clock' to be
driven however that clock wants to act. It must act contractually and rationally with regards to
other observable time facts."

The assistant pointed out that `Clock` promised nothing about direction -- the system clocks go
backwards when the time is set -- so a mock on interrupt time's timeline could run backwards and
still satisfy the bound, and put four points:

1. A marker trait `Steady: Clock`, promising readings never decrease, as the contract a mock signs.
   The engineer: "the contract is what you think it should be." (WT-D-9.)
2. The time facts dioring states: failure stamps never decrease in observation order, and the
   inventory reports the stamp the entry carried; the oracle checks them. "correct".
3. How far a mock must agree with time observed outside dioring. The engineer: some facts may slide
   and others are contractually significant. Code should almost never use timestamps, but it may use
   a timed wait resumed until the whole amount has accumulated, to simulate a sleep; then the
   clock's accounting must be rational enough for that to work -- not wall-clock time, but
   affordances for the wait to collude with a mock, so that code which "thinks" it waits 30 minutes
   sees them pass in 5 seconds, the time base advancing correspondingly and the event signalled at
   the right time, "with the code none the wiser". It "would require collusion to enable
   correctly". Recorded in win-time-sys as a working position, parked as WT-inf.2 until a timed
   wait exists to collude with.
4. Whether `Dioring` takes the clock too, or only the core. The engineer: "I don't have a good idea
   on how to go with this. I will take your advice." The advice: the instance too, as a defaulted
   parameter -- the code that waits sits above or beside an instance, so a mock that colludes with
   it must be the instance's, and a default leaves every existing spelling unchanged. (DI-D-38.)

WT-2.2 was split in two, since the bound lives in win-time-sys: WT-2.2.1 adds `Steady`, and
WT-2.2.2 has dioring adopt it.

## DI-3.2.4.2: markings (2026-10-08)

The engineer asked whether the item was ready, and to implement it if so. The assistant found no
blocker: the prerequisite had landed, and DI-D-36 left the shapes and the error-code
representation to the assistant. It named two gaps the build had to close (the core did not record
a write's requested length, and no test can hold a write in flight across a failure) and one
correction to the item's text: a covered marking was to name a write "in a seal that later
succeeded", but coverage is per file, since a seal that fails on one file has still committed the
others. The shapes are recorded as DI-D-39, for the engineer to confirm. One consequence the item did
not name: once `Healed` is in the stream, the oracle can require a `Durable` to follow it, and now
does.

## Lineage handles (2026-10-08)

On DI-3.2.5's open question -- what `epoch_state` answers for a lineage the instance does not have --
the engineer asked how instance A could have any insight into instance B's state. It cannot, and
the question never asked it to: an `Epoch` carries its lineage, so a caller can hand A an epoch of
B's, and A must say "not mine" rather than panic, as its other calls already did. The harder half is
A's own retired lineages.

The engineer then asked what Rust practice is, and whether a lineage should be registered and
returned as a lifetime-managed type instead of passing arbitrary values. The assistant set out the
API Guidelines' preference (C-VALIDATE: types over checks, checks over panics), plain IDs checked at
run time, owned handles, and branded lifetimes, and noted that failures already split a `Copy`
name from an affine token. Values written into a consumer's logs must stay plain, so handles help
the live operations, not stored values.

The engineer's first answers: dropping the handle ends the lineage and the handle is `#[must_use]`;
the handle comes back from construction; "apply the rule that makes them consistent, I don't want to
mis-specify"; and the handle joins the contract trait. Asked for feedback first ("if something is a
bad answer or is inconsistent, I expect feedback before proceeding"), the assistant raised:

- **A conflict:** the default lineage can be neither ended nor retired (DI-D-19, DI-D-30), so its
  handle's drop needed an answer.
- **A sharp edge:** `#[must_use]` does not catch `let _ =` or a handle dropped with its struct, so
  an accidental release abandons pending work.
- **The rule, stated:** acting on a lineage takes its handle; naming or asking takes the plain value
  and says "not this instance's" explicitly. Applying it exposed two existing inconsistencies:
  `durable_through` and `sealed_through` answered `None` for both "nothing yet" and "not mine", and
  `import_failure` of a missing lineage made an empty failure.
- **A question:** whether an explicit `retire` stays.

The engineer proposed that the returned lineage be an `Arc`, with the instance keeping a reference
to the default's. The assistant: the lineage then ends when its last handle is released, which
makes the default's permanence a consequence of the instance's reference rather than an exception;
`end` and `retire` must take the last handle by value, or a surviving handle could name a dead
lineage; and the `Arc` belongs inside a newtype. The engineer: "yes I think this is the coherent
answer." Recorded as DI-D-40.

## DI-3.2.5 split, and heals across lineages (2026-10-08)

Asked whether DI-3.2.5 was ready, the assistant found one unsettled question: when a heal takes
effect for a failure spanning several lineages, since DI-D-12 (c)'s "the first seal made after the
heal" named no lineage. It proposed per lineage -- each lineage's re-issued writes are committed
only by its own next seal, so one lineage's seal ending the failure everywhere would let another's
mark pass epochs it had not committed -- at the cost of a failure staying in the inventory while
one of its lineages never seals. It also proposed splitting the item: handles, minting, ending and
failures across lineages together (`DI-3.2.5.1`), then gates (`DI-3.2.5.2`). The engineer: "do the
split", then "let's go with per lineage for now. we can worry about more complex cases later if
necessary." Recorded as DI-D-41, keeping "for now" as its reopen condition.

## DI-3.2.5.1: lineage handles (2026-10-08)

The engineer asked for DI-3.2.5.1 to be implemented. It needed no further question: DI-D-40 and
DI-D-41 had settled the semantics and left the signatures to the item. The assistant recorded the
shapes it chose as DI-D-42, for the engineer to confirm. Four of them change behaviour the earlier
steps had, and are worth the engineer's eye in particular:

- **A failure that suspects nothing heals at once.** DI-D-41's rule, applied to a failure that
  belongs to no lineage. With one lineage such a heal used to wait for the next seal; one existing
  test relied on that and now gives its failure a suspect write.
- **The oracle's DI-D-39 tightening is withdrawn.** It required a `Durable` that passes a healed
  failure to follow the failure's `Healed` entry. Per-lineage heals break that: one lineage's
  `Durable` legitimately comes first, while the failure still holds another. The stream cannot show
  which lineage's seal made a heal effective, so the rule reverts to "healed or abandoned".
- **An ended lineage stops being live at once**, though the core keeps its writes in flight until
  they complete, so that a failure already suspecting one still gains its marking.
- **Ending a lineage releases a heal waiting for it**, so `Healed` can follow `LineageEnded`
  directly.

Two follow-ups belong to gates and were queued in DI-3.2.5.2, which already owns them: gated
operations on an ended lineage's epochs end as `NeverIssued`, and `LineageBusy::held_for_gate`,
zero until gates exist, gets counted.

## DI-3.2.5.2: a seal can close a gate cycle (2026-10-08)

Asked to implement DI-3.2.5.2, the assistant stopped before writing the cycle search: DI-D-23
refused the *push* that would close a cycle, but a seal can close one too. Which held writes a
lineage must complete to reach an epoch depends on how far the seal covering that epoch reaches, and
the consumer chooses that after the gated writes were accepted; a pending heal waits for the next
seal however far it reaches. The worked case: a write at M:4 gated on L:9, and one at L:9 gated on
M:3, close nothing when pushed, yet sealing M through 5 deadlocks, while sealing through 3 first
does not. It offered three answers -- refuse such a seal; split seals at gated epochs inside dioring;
or leave seal-closed cycles undetected as a stated limit -- recommending the first, and two smaller
points: a gate on an ended lineage (DI-D-30 said `GateAbandoned`, DI-D-40 and DI-D-42 made such a
lineage unknown), and a held write whose own epoch is abandoned first.

The engineer chose the first: "The answer needs to be (a) Also refuse a seal that would close a
cycle." With it came a direction on how: the normal, non-error case must be fast; reduce everything
to small identifiers, sorted lists and intersections, with closures over the lineages and heals in
flight; take more time for diagnostics only once the case is known to be an error; and "Don't plan
out the exploration of the graph so that we get the information up front if it's more costly than a
trivialization of the graph algorithm. I've found that often these graph algorithms can be
trivialized away from pointer chasing for the important cases."

The trivialization found: what a lineage must complete to reach an epoch only grows with the epoch,
so the search keeps one number per lineage and scans each lineage's held writes, sorted by epoch,
once, as that number rises. A seal is tested from all the held writes beneath it at once, and only
if that is positive, one write at a time; the path naming the cycle is a separate search on the
error path. The two smaller points were not answered, so the assistant took its recommendations and
recorded them, with the shapes it chose, in DI-D-43 for confirmation.

## DI-3.2.6: the consumer's provider (2026-10-08)

Asked to implement DI-3.2.6, the assistant found no open design question: DI-D-27 and the contract
already state what a request names, what success covers, what a domain's failure reaches, and that
a dropped completion is a failure of its own cause. One thing was left to choose -- on which thread,
and under what lock, dioring calls the provider -- and two constraints already fixed it. DI-D-27
lets a provider answer inside `make_durable`, and that answer takes dioring's lock, so the call
cannot be made with the lock held; and a provider panic propagates, which it can only do to a
caller, so the call cannot be made from a completion callback on a pool thread. So a seal that
becomes ready queues its request, and the consumer's next call on the instance makes it with the
lock released. Every request is built before any is made, so a panic in the provider drops the rest
and each is counted abandoned, as DI-D-27 says.

The checklist item asked for a provider failure to be observed "with its own `Reach`". None was
needed: `Reach::Domains` of the one failed domain is exactly DI-D-27's rule -- files containing it,
and files declared with none -- so the existing reach is reused. The assistant recorded the call
rule and the smaller shapes it chose in DI-D-44 for confirmation.

## Confirmations become checklist items (2026-10-08)

Asked what "open confirmations" meant, the assistant said: each was a clause in a decision row,
"decided by the assistant while implementing DI-3.2.x, to be confirmed by the engineer", tracked
nowhere but those rows and its own end-of-turn summaries -- which the repository's rules forbid as a
work queue -- and each checked-off item that left one had broken the move-or-spawn rule. It
proposed one review item per decision. The engineer did not answer that, and asked for DI-3.2.7
next; the assistant applied the rule to DI-3.2.7's own decision, DI-D-45, and left the earlier
ones for the engineer's answer.

## DI-3.2.7: ending (2026-10-08)

Lineage ending and retiring had already landed in DI-3.2.5.1, so DI-3.2.7 came to `remove_file`,
`close()` and drop. No question was open: DI-D-25 and API.md section 8 state the rules. Two shapes
had to be chosen and one hazard found. `FileBusy`'s in-flight count clears at the kernel's
completion, as `LineageBusy`'s already did. The hazard: DI-D-25 waits for every provider call in
flight, and a provider that keeps its completions in itself answers them only when it is dropped,
which a wait inside the instance's own drop would come before; so ending drops the provider first.
Running the suite found the wait doing its job -- two tests ended an instance while holding a
completion, and hung -- and those tests were the ones to change. Recorded in DI-D-45 for
confirmation, which the engineer gave as written: "di-3.2.8: confirmed!" With it DI-3.2 -- dioring
over the ring crate, construction through ending -- is complete.

## The assistant's decisions become review items (2026-10-08)

The engineer answered the proposal: "yes please put them into the checklist". DI-3.9 queues one
review per decision the assistant made while building DI-3.2 -- DI-D-34, -39, -42, -43 and -44 --
placed before DI-3.3, and win-time-sys' WT-M3 the two of its own, WT-D-7 and WT-D-8.

## DI-3.9.1: reviewing DI-D-34, point by point (2026-10-08)

At the engineer's request ("explain di-3.9.1 in more details, and let's work through it step by
step"), the assistant walked DI-D-34 in nine steps against the code as it stood, noting first that
the row had gone stale without saying so: DI-D-41 and DI-D-42 had changed its heal and its
lineage-scoped import, and it carried neither marker. Most steps were confirmed as built. Three
were amended, and each is recorded in DI-D-46:

- **Reach.** An imported domain no file declared is dropped, which is right under complete
  declarations but makes a misspelling silent. The engineer: "I would prefer for it to report the
  names but not if it's difficult" -- it was not, so `import_failure` now returns the names it
  could not match.
- **Heal.** Checking the rule against DI-D-41's own rationale found a gap: "the first seal made
  after the heal" need not reach the re-issued writes, so a seal through 5 could release a heal
  whose data went to 10, and a seal naming no write released one with no I/O at all. The engineer
  chose option (b): the releasing seal must reach the highest epoch pushed when the heal was made.
  The gate cycle search follows the same seal, and a test shows a cycle the old rule missed.
- **Inconsistencies.** The assistant offered tracing, a query, or fail-stop for release builds,
  and then deferring the panic to the consumer's next call. The engineer rejected the debug and
  release split and the deferral alike: "panic where it's detected. Don't let the situation
  persist", and, on a panic the process catches, "we still will panic again if the situation is
  again detected" -- so there is no sticky state, and nothing is recorded at all.

Two answers were incomplete, and the assistant said so rather than assume: the engineer had answered
only one point of three, and then one question was garbled (C), which turned out to have been
answered by step 4. On the fault seam the engineer agreed it serves a different purpose from
DI-3.3, so no merge-or-delete decision is queued.

## DI-3.9.2: reviewing DI-D-39, and provenance (2026-10-08)

The engineer asked the assistant to evaluate DI-D-39 against the current state and lay out what
needed adjudicating. The evaluation found the row's account of a covered marking stale: it said
"the write's own file's flush", written before DI-D-27's provider, while the code already marked a
write covered only once every part its file depends on answered success. The rulings, recorded in
DI-D-47:

- **A. What covered means.** DI-D-27's meaning, confirmed ("yes that is correct"); the row,
  CONTRACT.md and the `MarkingKind` doc were corrected to it.
- **B. Codes.** The assistant's question was whether every failure carries a code. The engineer:
  "when an actual failure occurs and is reported from the kernel, we are responsible for capturing
  it and escorting that code through and reporting it. If the failure is relayed by the consumer,
  that fact should itself be relayed *instead* of the error code." Asked how that applies to a
  provider's own error, the engineer stated the principle the rest follows from: "The thing that
  matters is that the information is accurately portrayed regarding its provenance. If information
  is coming from a durability provider, it must be cited as such etc". So `Cause::Provider` keeps
  the provider's `io::Error`, documented as the provider's report, and every cause says whether it
  is the kernel's, the consumer's, the provider's, or dioring's own observation.
- **C. The nullifier's code.** The engineer first asked whether this was "a resolution maybe not
  an error?"; after the assistant's follow-up, accepted ("ok") that the code is a plain
  `ErrorCode` and that a failed write completion carrying none is an internal inconsistency under
  DI-D-46. No test reaches it: only a ring breaking its contract could.
- **D. A final record is a snapshot** as of its resolution: accepted.
- **E. A covered marking cites who answered**, `Covered { flushed, provider }`: "seems good".
- **F. Provenance on `Durable` or `epoch_state`:** declined. The engineer: "I don't see the need
  for it. When things are 'clean' there aren't really questions about how it was accomplished."

## DI-3.9.3 begins: a late failure, and what Windows promises after a failed flush (2026-10-08/09)

At the engineer's request the assistant evaluated DI-D-42 against the code and brought only the
points needing a ruling. The one that led here, case A: a flush of a lineage that ends while the
flush is in the kernel -- issued while the lineage was live; ending neither waits nor cancels
(DI-D-30) -- completes afterwards, and if it failed, `flushed` returns before observing anything,
as `provided` does for the provider's answers. DI-D-21 says a failed flush on F makes suspect the
uncovered writes of every lineage on any file whose domains intersect F's, so the failure concerns
live lineages too, and dropping it also contradicts DI-D-47's ruling that a kernel-reported failure
is captured and escorted. The default-flush half is DI-D-30's ("a flush that finishes afterwards
reports nothing", closed by the engineer); the provider half is DI-D-42's.

The engineer asked why an ended lineage would receive a flush at all (it receives none new: only
those in flight when it ended), and then named the question underneath this and earlier rounds:
"what the Windows Kernel contract is around data retention when a flush call fails. Evidently there
was a problem with Linux's contract. I am not quite convinced that Windows follows the same
contract. I think this issue deserves some research."

### What the published record says (research, 2026-10-09)

Windows documents nothing about what happens to cached data after a failed flush, and nothing
published measures it.

- **PostgreSQL's cross-OS survey of exactly this question lists Windows as "unknown"**
  ([Fsync Errors](https://wiki.postgresql.org/wiki/Fsync_Errors), "Closed source kernels").
- **Windows does define a path on which a failed write-back loses cached data.** The SDK's
  `ntstatus.h` and `winerror.h` (10.0.26100.0) define `STATUS_LOST_WRITEBEHIND_DATA` (0xC0000222) /
  `ERROR_LOST_WRITEBEHIND_DATA` (596) and three variants, `..._NETWORK_DISCONNECTED`,
  `..._NETWORK_SERVER_ERROR` and `..._LOCAL_DISK_ERROR` (0xC000A080-0xC000A082), all with the text
  "{Delayed Write Failed} Windows was unable to save all the data for the file %hs; the data has
  been lost." The local-disk variant names a removed device or write-protected media, so the path is
  not specific to network shares.
- **Microsoft assigns the handling to the application.** Its article for event ID 50
  ([KB 2021074](https://learn.microsoft.com/en-us/troubleshoot/windows-server/networking/delayed-write-failed-error-data-lost))
  says the pending cached write "cannot be written to the target file", "the target file may now
  be corrupt", and "It is the responsibility of the application to handle this error."
- **The interfaces are silent on retention.**
  [`FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)
  says only that it fails;
  [`CcFlushCache`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ccflushcache)
  and
  [`CcUnpinRepinnedBcb`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ccunpinrepinnedbcb)
  report a status and how many bytes were flushed. Microsoft's open-source FAT sample
  (`microsoft/Windows-driver-samples`, `filesys/fastfat/flush.c`) passes the cache manager's status
  up and re-dirties nothing on failure; what becomes of the pages is decided inside the cache and
  memory managers, which are not published.
- **Microsoft's own database does not rely on the cache for durability.** SQL Server opens every
  log and data file with `FILE_FLAG_WRITE_THROUGH` and requires "guaranteed delivery to stable
  media"
  ([Logging and data storage algorithms](https://learn.microsoft.com/en-us/troubleshoot/sql/database-engine/database-file-operations/logging-data-storage-algorithms)).
  Suggestive, not evidence of the kernel's behaviour.
- **A handle's buffering mode can be read back.** `NtQueryInformationFile` with
  [`FileModeInformation`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_mode_information)
  reports `FILE_NO_INTERMEDIATE_BUFFERING` and `FILE_WRITE_THROUGH` for an open handle.

**Unknown, and what `DI-4.1` exists to measure:** whether a failed `FlushFileBuffers` leaves the
pages dirty for a later flush or drops them; whether a background write-back failure is reported to
a later flush on any handle (Linux has done so per open file description since 4.13; Windows
documents no equivalent); and whether a later flush can then succeed over lost data. With
`FILE_FLAG_NO_BUFFERING` the system cache holds nothing, so the first two do not arise; what remains
is the device's volatile cache across a failed device flush, which is the device's behaviour, not
Windows'.

**Discarded:** two web-search summaries asserted that Windows "likely" marks pages clean as Linux
does. The only source either cited is a
[2019 Stack Overflow question](https://stackoverflow.com/questions/54660780) asking exactly this,
which has no answer. The best-known study,
[Can Applications Recover from fsync Failures?](https://www.usenix.org/conference/atc20/presentation/rebello)
(USENIX ATC 2020), covers Linux file systems only.

**What it does to case A:** the answer changes how much case A costs, not whether it is a defect.
If Windows keeps the pages dirty and retries them, a live lineage's own next flush rewrites its
data and the dropped failure hides a failure without losing data; if it drops them, a live
lineage's later flush can succeed over data that is gone. DI-D-21 was written on the response space
Windows permits, which includes the second, and case A contradicts it either way.

### The engineer's position on unbuffered handles (2026-10-09)

The engineer: "I think that our position has to largely be that we cannot provide much consistency
without NO_BUFFERING." Recorded as stated. What it implies for the contract, for dioring's
behaviour, and for case A was raised with the engineer in the same exchange and is not yet settled;
nothing binds to it until it is.

### What can fail without telling anyone, by substrate (2026-10-09)

The engineer, to establish "the truths of the matter": "if the user does not use no_buffering, then
at any time, the filesystem could have had a failed eviction without notifying any party, including
us, is that correct?" The assistant: yes as far as anything published says, refined three ways. The
system's background write-back of a dirty page can fail between a write's completion and its
covering flush, and Windows' own text calls that data lost, with no documented report to the writer;
Windows does log it system-wide (event ID 50 per KB 2021074, for shares; the local-disk surface is
unverified), so "no party" is too strong -- "no party that wrote it, through any I/O interface" is
the claim. What happens next -- dropped or retained, and whether a later flush reports it -- is
`DI-4.1`'s. And it is a failed write-back, not a failed eviction: a clean page is evicted without
loss, and data already made durable is not exposed.

| Substrate | Layers between a write and the media that can fail without telling the writer |
|---|---|
| Cached file | system-cache write-back of data; file-system metadata write-back; the device's volatile cache |
| Unbuffered file | file-system metadata write-back, unless write-through or into allocated space; the device's volatile cache |
| Raw partition | the device's volatile cache only |

Asked how a ring over a raw partition would change what dioring consumes and what it can offer, the
assistant set out, from `CreateFile`'s "Physical Disks and Volumes" remarks and `FSCTL_LOCK_VOLUME`'s
documentation: every data-path failure then arrives as a completion of something dioring issued, so
CONTRACT.md's assumption that every failure is reported becomes a property of the stack, except for
co-tenants of the device's cache and the device itself; most of what `DI-4.1` would measure stops
mattering, and rewrite-then-seal heals soundly without any knowledge of Windows' retention; a
partition is a host-side construct, so the device's cache is shared with every other partition on
it; opening a volume requires `FILE_SHARE_WRITE`, so exclusivity comes from `FSCTL_LOCK_VOLUME`
("can be accessed only through" the locking handle; refused for the system volume, a page-file
volume, or a volume with open files), which reaches the volume and not the device; a volume's disks
are discoverable through `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`, so its flush domain could be
observed rather than declared; the "file" vocabulary collapses to one handle with regions in it and
one flush per volume; access needs administrative privileges and `OPEN_EXISTING`, volume handles are
to be treated as noncached with alignment enforced, direct writes to a volume with a mounted file
system are restricted, and the last sectors need `FSCTL_ALLOW_EXTENDED_DASD_IO`. Unverified, and a
spike's ([D-44](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-44)): whether IoRing reads, writes
and flushes a volume handle, and whether a physical disk can be held exclusively. The shape that
follows is a contract per substrate class -- raw and exclusive; unbuffered, with or without
write-through and allocated space; cached -- each stated truthfully, with dioring able to tell the
buffering class of a handle itself (`FileModeInformation`).

### The principal goals (the engineer, 2026-10-09)

> I have two primary plans on dioring consumers. one is a filesystem layer which is currently
> layered over a substrate much like our ioring, but it has no durability mechanisms. We should
> assume that its I/O is based on either direct partition access, or the driver stack has a
> partition driver that consumes the ioring-equivalent over a vhdx and exposes partitions.
>
> I want to modify the vhdx driver to consume and produce the right kind of interface to give the
> right durability contracts needed for the filesystems.
>
> The second overall plan is to provide for products like databases that are aware of the platform
> requirements and will use file handles that are opened with the correct attributes for dioring
> optimal performance.

Asked to confirm, the engineer: "These are the principal goals." Two questions open since the
2026-10-05 section "The stack" are now answered: the VHDX driver is user-mode Rust that would
consume dioring directly ("yes"), and the VHDX is **dynamic** -- "We can control this, it's why I'm
bringing it up now."

**Why the step back** (the engineer): "trying to shore up the buffered I/O strategy can only be done
on a 'best effort' basis. At some point, it's not guaranteeable, so continuing to try to refine it
will become an exercise in frustration."

The assistant's reading of what the goals imply (directional; none of it decided):

- **The VHDX driver produces `DurableRing`.** The filesystem then cannot tell dioring over a
  partition from the VHDX driver over dioring, and the trait gains the second real implementation
  the "Open source for everyone" section said the public crate needs. The driver can pass up no
  more than dioring gives it over the host file, plus its own discipline of reporting failures
  upward and never letting a later success mask one.
- **The two bottoms give different classes.** A raw partition can be locked, leaving other
  partitions on the device and the device itself. A `.vhdx` is best opened unbuffered and
  write-through, but its host volume is shared with everything the system writes there and cannot
  be locked, so a filesystem over it gets "truthful relative to what was reported", the unreported
  part being the host's other writers on the device's cache.
- **Dynamic means the host file grows**, and growth is host-file-system metadata. How the driver
  grows it -- per write or in large preallocated steps -- is the driver's to choose, and decides how
  often a write lands outside allocated space; a growth step is itself something the BAT update
  pointing into it must wait on (the release-after-durable ordering the "The stack" section already
  named inside VHDX).
- **`Cause` assumes dioring's own world.** Its variants -- `Flush { file, error }`, kernel-reported;
  `Imported`; `Provider`; `ProviderAbandoned` -- fit none of a VHDX driver's failures, which are
  relayed from the dioring beneath it and translated from host-file offsets into virtual-disk
  terms. Under DI-D-47 it must not report them as `Flush`, so the trait likely needs a cause for
  "relayed from the layer below", carrying that layer's own report.
- **Databases are the unbuffered class**, which dioring can confirm per handle rather than take on
  declaration -- the stronger provenance -- and the class where `WriteCaching::WriteThrough` works
  at all and DI-6+.6's one-device-sync-per-domain aims.
- **Cached handles stay supported**, with the weaker contract stated rather than refined: neither
  goal depends on them, and the engineer's reason for the step back is that refining them further
  cannot reach a guarantee.

### Targets, not files (decided 2026-10-09; DI-D-48)

The assistant proposed four checklist items: a raw-partition spike, a relayed cause, the contract per
substrate class, and raw-partition support. The engineer's answers reshaped three of them:

- On the spike: "I don't like this 'we have to run against the kernel' point of view. Let's
  describe an interface and implement to it." The assistant flagged that this runs against
  [D-44](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-44)'s spike-before-wrapping rule, and
  proposed reconciling them by designing against the interface and treating whether a Windows
  backing meets it as part of implementing that backing, not as a gate on the design. Not yet ruled
  on.
- On the relayed cause: "this seems reasonable, and it's true of all cases, we're always just
  relaying." So dioring over Windows relays the kernel exactly as the VHDX driver relays the dioring
  beneath it; the kernel is one "layer below" among others.
- On the per-substrate contract: "I wonder if we should perhaps abstract away from 'files' entirely
  and instead have a placeholder object with only its contractual obligations called out." The
  assistant observed that this and the interface of the first answer are one thing, drafted the
  obligations (now in DI-D-48), and noted that the substrate classes then become how far each
  backing warrants the reporting obligation, with dioring's guarantee "durable to the extent its
  targets honour theirs". It treated the "I wonder" as a working position and asked whether to
  queue it.
- On raw-partition support: "ok".

The engineer: "Let's switch to this. I think it's a much stronger position. Then focusing on the
win32 file object as a target can be a separate concern." Recorded as DI-D-48 and queued as
`DI-3.10`, ahead of `DI-3.9.5` and `DI-3.3`, the two pending items written in the file vocabulary.
The assistant had first said `DI-3.9.4` was affected too; on re-reading, it uses none of that
vocabulary, and stays ahead.

### Any fault pattern, an algorithm at the core, and layers as targets (decided 2026-10-09; DI-D-49)

The engineer: "Given this pivot, we can construct any fault pattern on a target that we wish." The
assistant agreed and drew out what it changes: producing failures on demand stops being hard, so
paths untested only because the kernel will not misbehave become reachable, case A among them; tests
run the real core over a faulting target rather than a second implementation of the contract; and
`DI-4.1` stops serving testing, mattering only for what the Win32 file backing can warrant. It
named the choice this forces -- a faulting target under the real core needs the core to send I/O
through targets, making IoRing the Win32 backing's mechanism -- and noted that the core is already
half split that way: `durability.rs` and its children never touch the ring and already return what
is due.

- **Targets beneath the core.** The engineer: "That's what I think, let me know if you disagree."
  The assistant agreed with two refinements. First, it is two interfaces: a **backend** that
  submits and completes I/O -- one IoRing shared by every Win32 target, so a batch is one
  submission -- and the **targets** it serves, which carry obligations; the engineer: "seems OK,
  I'd like some idea of what the general shape of the i/o api is - as long as it's shaped like a
  ring with a sq/cq and a doorbell and notification mechanism, I'm sure I'll be ok." The assistant
  sketched it (push, submit as the doorbell, pop, a notification on every empty-to-non-empty
  transition; write, read and commit entries; a fault carrying the reporting layer's code and
  source) and observed that dioring turns a ring without durability into one with it -- possibly
  the shape of the engineer's existing portable substrate. Second, **layers are targets**: a
  partition is a window over another target and a VHDX driver a target whose commit succeeds once
  its own writes are durable in its format's order. The engineer: "yes that seems like the right
  refinement. I didn't see why a partition driver would provide an entire dioring."
- **DI-3.3.** The engineer agreed it becomes a faulting target ("I think so, do you have an
  alternative?"); the assistant proposed adding a faulting backend, since refusals, a full
  submission queue and late or reordered completions are the backend's, and recording the
  2026-10-05 "public second implementation" purpose as served by the target interface. Carried
  into `DI-3.10.1`'s proposals.
- **Portable.** The engineer: "the filesystems that I alluded to are portable, and the ioring-like
  api that they use today works over both windows ioring and linux iouring. right now it has no
  provisions at all to consider durability ... if our dioring solution can be algorithmic only
  that's actually ideal." The assistant listed the core's remaining platform ties (interrupt time,
  the `HRESULT` code, the trace macro, `SharedHandle`, `Event`, the buffer types) and named the
  workspace's Windows-only rule as the blocker. The engineer: "the crate is 'durable-ioring', not
  'win-durable-ioring', so perhaps it can be non-windows-only? I think the rule was meant to
  simplify thinking. There is no hard requirement for disabling non-windows building." The
  assistant found the precedent already in DEVELOPMENT.md -- `wtf-string`'s portable core, verified
  by CI's `portable` job -- and that the instruction files overstated the rule.
- **Settled together:** the state machine public "with the threaded driver on top, possibly as a
  separate 'win-' crate since it *is* windows specific"; the instruction files aligned with
  DEVELOPMENT.md ("yes"); restated in place; and `DI-3.10.1` handed to the assistant ("I am
  suffering from 'design fatigue' at this moment and to make better progress you should make
  concrete proposals that I approve or disapprove"). The assistant noted that the workspace's
  naming decision gives `win-` only to crates raising a Windows API into safe Rust, so the driver's
  name is among its proposals.

### DI-3.10.1 proposals: names and the obligations (2026-10-09)

The assistant brought thirteen proposals (P1-P13) for approval. Settled so far:

- **Names** (the engineer: "I think maybe it's IoTarget, and it's DurableIoCore"; then, worried
  about "overly short names", asked whether the cause enum should be `FaultCause` or `ErrCause`).
  The assistant proposed `FailureCause` -- it is the cause of a *failure*, beside `FailureId`,
  `FailureToken` and `FailureInfo`; `FaultCause` would read as the cause of an `IoFault`, and
  `ErrCause` abbreviates -- and lengthened the rest to match. Approved: `IoTarget`, `IoTargetKey`,
  `IoTargetBusy`, `IoTargetWrites`, `DurableIoCore`, `CoreActions`, `FailureCause`, `IoFault`;
  `add_target`, `remove_target` and `ErrorCode` unchanged.
- **P2, the obligations.** The engineer caught that the first draft said a failed commit "warrants
  nothing for writes not yet covered": covered is dioring's notion, not a target's. Restated in
  target terms: for each write that completed successfully, the first commit submitted after its
  completion decides it -- success puts its bytes on stable media; failure ends every statement
  about it, and a later commit's success does not warrant it. A write that completes with a fault
  is never warranted. A target's failure domains declare that a failed commit may have lost any
  completed write not yet decided by a successful commit, on it or on a target sharing a domain;
  acting on that is dioring's. Covered then means a write's deciding commit succeeded, and suspect
  that it failed or a sharing target's commit failed first -- dioring's behaviour unchanged. The
  "first commit decides" rule is what makes Linux's 2018 behaviour a target *breach*.
- **Short writes.** The engineer: "Reads can come up short but I've never heard of a valid reason
  for a write to be short." The assistant found Windows documents exactly one case, a non-blocking
  byte-mode pipe (`WriteFile`), never a target; Linux's `write(2)` names running out of space, the
  `RLIMIT_FSIZE` limit, and a signal. The engineer preferred that a write complete in full or with
  a fault, and asked whether there is enough information to reissue the remainder: there is,
  because the backend owns the write until its final completion, and the core cannot commit
  between the parts. Then: "is that the right thing to do under Linux though? do we need to ensure
  that we don't enter into an indefinite loop?" A progress rule bounds it, but a retry after a short
  write caused by `RLIMIT_FSIZE` raises `SIGXFSZ`, whose default action terminates the process
  (`setrlimit(2)`). The engineer: "this is only if `setrlimit(2)` was called"; the assistant: a
  finite limit is needed, but it is inherited, settable by a parent shell, a service manager or
  PAM, and the kernel default is unlimited. The engineer: "can the limit be changed after a write
  has been issued?" Yes -- `prlimit(2)` changes another process's limits, and the limit applies when
  the write executes -- so checking `getrlimit` before reissuing is a race. The engineer: "Can we
  just ask the client for how to proceed?" Asking per event would report the short completion after
  a seal may already be committing its epoch; asking once, at construction, lets the backend act
  before the completion is reported. The engineer: "yeah go with (i)". **Settled:** a target's
  write completes in full or with a fault; on Linux the backend handles a short count under a
  policy the client chooses when building it -- `Fault` by default, `Reissue { max_attempts }` for
  a client that handles `SIGXFSZ`; the Windows backend never sees one. The core never sees a short
  write, so DI-D-36's short-write allowance is withdrawn.
- **P3-P13.** Approved together: "otherwise yes good. In-memory is a good basis for unit testing
  and driving", except P12's second crate, which the engineer named `win-durable-ioring`. The
  assistant noted that the workspace's naming decision gives `win-` only to crates raising a
  Windows API into safe Rust, and left to the engineer whether that decision is amended or this is
  an exception. Recorded as DI-D-50, with amendment markers on the rows it restates (DI-D-21,
  DI-D-27, DI-D-29 superseded, DI-D-36, DI-D-39, DI-D-44, DI-D-47); `DI-3.10.1` closed, and the fault
  seam's merge-or-delete queued as `DI-3.11`.
- **The `win-` prefix.** Offered amending the workspace naming rule or recording an exception, the
  engineer: "b record this as an exception". Recorded in the root naming decision.

### DI-3.10.2: the contract restated over targets (2026-10-09)

The engineer: "implement di-3.10.2". The assistant restated CONTRACT.md in place rather than drafting
a second copy, since two statements of one contract are the drift the workspace's contract-integrity
rule names. Its worked examples are the crate's doctests and compile against today's API, so they
were left alone and a "Vocabulary ahead of the code" note says so until `DI-3.10.3` restates the
code. Choices the assistant made in the restatement, for the engineer to see:

- **Warranty classes by backing.** `Complete`: a device held exclusively with nothing else on it,
  or a layered target over `Complete` targets that relays all they report. `SharedDevice`: a Win32
  file opened unbuffered, or a raw volume whose disks also hold other volumes -- a partition is a
  host-side construct, so a locked volume still shares its device's cache with its siblings.
  `BestEffort`: a Win32 file opened for cached I/O. A layered target claims no more than the weakest
  it rests on.
- **Overlapping writes in flight** leave the overlapping bytes unspecified -- either may land last,
  and a target may interleave them -- tightening "land in an unspecified order", as the short-write
  discussion found it already meant together with "no atomicity beyond the device's".
- **"Domain"** is the contract's word for what the code calls a flush domain, since a target
  declares domains for failure sharing and a provider serves them.
- **Reporting outside failures through the import** is required for every target whose class is
  not `Complete`.
- **Diagnostics:** waits are reported through the core's tracing hook, which the Windows driver
  publishes as ETW.

The engineer's review: "No significant objections", with two changes. **"Domain" reverts to
"flush domain"** ("let's call it a FlushDomain"), applied to CONTRACT.md and noted in DI-D-50. And
on diagnostics: "I think that we need another level of indirection for the tracing on Windows or
maybe is it just in general? At least even a windows only consumer may not want to use ETW and may
want to just have their own stderr based or OutputDebug based printing." Taken up as its own
proposal, since it changes DI-D-26.

The assistant answered "in general": the core is portable, so the indirection belongs there, and
proposed T1-T5 -- a `DiagnosticSink` trait receiving typed events, so an ETW manifest maps each
one-to-one and a text sink formats them; a sink stating which kinds it wants, checked before an
event is built, keeping DI-D-26's "costs nothing when no session listens"; `NoDiagnostics`,
`StderrDiagnostics` and `FanOutDiagnostics` portable, `EtwDiagnostics` and
`DebugOutputDiagnostics` in `win-durable-ioring`; ETW the Windows driver's default, any sink
passable when it is built; and the `trace` feature removed, its one trace point -- the
inconsistency report -- going through the sink. The engineer: "approved". Recorded as DI-D-51.

## DI-3.10.3.5's shape (2026-10-10)

After DI-3.10.3.4 landed, the assistant put six questions before starting DI-3.10.3.5: Q1, a split
into four steps building the new path beside the old; Q2, the core defining the submission shape
rather than the backend, correcting its own step-4 wording ("entries whose shape the backend
defines"), since a backend-defined shape would make the core build entries through a trait it has
no other use for; Q3, the trait shapes; Q4, a refused push undone and a refused commit kept; Q5,
Windows threading unchanged, `EventDelivery`'s callback waiting and draining; Q6, readiness an
associated type ahead of the split.

The engineer: Q1, "split into as many steps as you see fit to make this achievable with high
quality and predictability"; Q2, "Yes. I feel confident we had already approved this"; Q3, "in
principle sounds good", with feedback to come once there is source; Q4, "ok. I assume there is no
real alternative, correct? Block waiting for the ring to be processed?"; Q5, "Yes but factored out
hopefully? But the threadless vs. numa domain bound aspect seems like it should be retained"; Q6,
"yes".

On Q4 the assistant answered that there is an alternative for consumer operations. The ring refuses
for two recoverable reasons -- a full submission queue, which a submit clears, and a too-full
completion queue, which draining clears -- and on Windows the pool callback drains without the
consumer, so a push could submit, wait on the notification and retry. It cannot be the core's,
which does no I/O and never blocks; it cannot happen inside a completion or provider callback,
which would wait for itself, so a commit the core emits there can only be kept and retried; and on
a threadless backend it deadlocks unless the driver pumps while it waits. It proposed keeping
refusal, and a blocking push later as an optional driver policy.

On Q5 it corrected the premise: as built, dioring drains only on the default pool
(`EventDelivery::new(ring, ..., None)`), so neither a NUMA-bound pool nor a domain thread is
retained today -- DI-D-18 names both of the ring crate's models, and dioring built Model A only.
It proposed a threadless backend with delivery as a separate layer in the driver: pool delivery
with an optional callback environment, and pumped delivery, in which the consumer's own thread
waits and drains.

The engineer: Q3, "do not block on me reviewing the source, I will see it and offer commentary as we
go along. I cannot be the blocking factor"; Q4, "I agree with the refusal, ensure that the cause of
the refusal is clear so that the caller can take the correct action (submit, drain) without having
to probe for why the refusal occurred. At least, if the cause of the refusal is known, pass it
through, so perhaps there are many states (submit needed, drain needed, both needed, unknown,
others...)"; Q5, "good". The blocking push was not ruled on, and is queued as a proposal. Recorded
as DI-D-52, with the steps as DI-3.10.3.5.1 to DI-3.10.3.5.8.

## DI-3.10.3.5.6: where pool delivery lives (2026-10-10 to 2026-10-11)

Starting DI-3.10.3.5.6, the assistant found a blocker. DI-3.10.3.5.5 had given the `IoRing` to a
threadless `WindowsBackend`, but today's pool delivery is the ring crate's `EventDelivery`, which
must own the ring. dioring cannot build pool delivery itself around the backend's ring: a
thread-pool wait on the ring's completion event needs `WaitableHandle::assume_waitable`, which is
`unsafe` and dioring forbids it (DI-D-4); and copying `EventDelivery`'s delivery loop -- which took
the ring crate three fixes to get right (M26.9, M26.12) -- would duplicate exactly the code most
likely to break.

It proposed R1: the Windows backend in two forms, threadless and pool, the pool form handing its
ring to `EventDelivery` with an optional callback environment, completions arriving by callback
already converted, pushes going through `EventDelivery`'s scope, and `pop` returning nothing --
no change to the ring crate, at the cost of a pool-form backend having threads behind it. And R2:
change the ring crate so `EventDelivery` can wake an owner who drains the ring itself, keeping the
backend threadless, at the cost of a release-triggering change to `windows-ioring-sys` and a design
pass on lock ownership there. It recommended R1, and R3, splitting 5.6 into the pool form and the
switch-over.

The engineer asked: "which shape would fit a model with a linux iouring based back end solution
best?"

The assistant's answer, recorded in full because the engineer asked for the analysis to be saved:

**R2's shape fits a Linux `io_uring` backend; R1 would make the Windows backend the odd one out.**

What a Linux backend looks like on its own. It is naturally threadless: submission entries are
filled in shared memory, `io_uring_enter` submits, and completions are read from shared memory with
no system call; even SQPOLL adds a kernel polling thread, not a user thread. So a Linux backend is
exactly push / submit / pop, owned by whoever holds it. Its notification is an eventfd registered
with the ring and signalled when completions are posted -- a plain file descriptor that something
outside the ring waits on: epoll, an async runtime's reactor, or a dedicated thread. There is no
pool-callback primitive like Windows' thread-pool wait, so "pool delivery" on Linux is necessarily
a separate layer: something waits on the eventfd, then calls the owner to drain -- R2's shape, in
which the waiter wakes an owner and the owner pops its own backend under its own lock. Waiting in
place exists too: `io_uring_enter` can wait for a minimum number of completions, which matches
pumped delivery (Model B), as `submit_and_wait` does on Windows.

How each proposal maps onto Linux. R1 puts Windows' delivery inside the Windows backend, so a Linux
backend would need its own delivery built in too, a different shape per platform; `IoBackend::pop`
would mean nothing for any pool-form backend, and the backend trait would stop describing what
drivers use. R2 gives every platform the same three layers:

| Layer | Windows | Linux |
|---|---|---|
| Backend (threadless) | `IoRing` | `io_uring` |
| Notification | completion event | eventfd |
| Waker (delivery) | thread-pool wait, optionally NUMA-bound | epoll, a runtime, or a thread |

The driver's lock wraps the core and the backend together, and the waker only says "drain now", so
no lock is shared across layers (DI-D-18).

Two smaller points also favour R2. The push/submit split is real on Linux -- submission entries
can be batched before one `io_uring_enter` -- whereas on Windows the ring crate's D-5 submits on
every push, so the trait as it now stands fits Linux better than Windows already. And the refusal
causes carry over: a full submission queue maps to `SubmitNeeded`, and an overflowing completion
queue makes submission return `-EBUSY` (with `IORING_FEAT_NODROP`), which maps to `DrainNeeded`.

What R2 costs on Windows is a change to `windows-ioring-sys`: a wake-only form of `EventDelivery`
that keeps the hard-won parts -- the event attached unsignalled, the wait armed, then the setup
signal raised; the owner called, the wait re-armed, the owner called again -- with the owner obliged
to drain to empty on every call, because the event fires only when the completion queue goes from
empty to non-empty. That is a release-triggering change to another crate, so it is its own item,
done before 5.6.

The assistant changed its recommendation to R2, sequenced as the ring crate's wake-only delivery,
then pool delivery in dioring built on it with the optional callback environment, then the
switch-over; and asked whether to proceed or whether the engineer wanted to shape the waker's API
first.

The engineer: "yes, i thought r2 was better for this reason, its good to have a detailed analysis.
be sure to save this and then go ahead." Recorded as DI-D-53, with the ring crate's D-84 and M33.1,
and DI-3.10.3.5.6 split into DI-3.10.3.5.6.1 and DI-3.10.3.5.6.2. The waker's API was not shaped by
the engineer; the assistant's choices for it are queued for review in the ring crate's M33.1.

## DI-3.10.3.5.6.2 re-planned, and what the backend refuses for good (2026-10-11)

Not a discussion with the engineer: the assistant's own finding while starting the switch-over,
recorded here because it changed the plan and produced a decision (DI-D-54) queued for review.

Mapping today's driver onto the core showed that the core held a gated operation without asking
the backend whether it could ever issue it, and kept every refused submission of its own for
re-emission. A cause that cannot clear therefore looped forever. The concrete case: a gated
registered-span write on an instance with no registered buffers. Today's driver refuses that at
push (`NoRegisteredBuffers`); the core would accept it, hold it, and on release meet
`Unsupported` at every re-emission, so the write never completes and its seal never resolves.
Today's driver has the same flaw for any released operation the ring refuses after its own checks
-- its comment says "no test reaches that".

The switch-over was re-planned from one step into four: the fix first (`DI-3.10.3.5.6.2.1`), then
readiness as an associated type and targets as the implementation's own type, which stand alone on
today's driver, then the swap itself.

The fix has two halves. Before holding, the core asks `IoBackend::check`, after its own checks,
so the backend's word comes last. After release, a submission of the core's own refused for a cause
that is not retryable completes as the backend would have with a fault, under a new `REFUSED`
source whose error is the `RefusalCause` itself. `Unknown` counts as permanent: retrying it blind is
the hang. For the Windows backend, `check` needs the registration's span check without a push; the
ring crate kept that private, so it gained `RegisteredBuffers::check_span` (its M34.1) rather than
dioring re-implementing the bounds, which would be a second copy of the rule. The push still leaves
spans to the ring, which keeps the ring-refusal path the backend rebuilds a submission on
reachable by a test.

One wording error was caught before commit: the first draft said an ended write "fails its epoch".
It does not -- a failed write is nullified (a `Nullified` marking) and its epoch can still become
durable without it, as `a_failed_write_completes_as_failed_and_is_never_committed` already showed.

## DI-3.10.3.5.6.2.6: the swap, and two edges no test reached (2026-10-11)

Not a discussion with the engineer: the assistant's findings while landing the swap, queued for
review in `DI-3.9.6`.

Moving the old path's sabotages over found three rules the new `pop` carried with nothing to show
them, and one that the old path carried in two places.

- `pop`'s retry of the core's refused submissions was an equivalent mutant. A push submits as it
  goes, so a retryable refusal is either SubmitNeeded after a failed submission or DrainNeeded under
  completion pressure, and either way completions are still to come; the pool drain re-emits the
  refused submission with the actions of the first of them. The retry was removed rather than kept
  with a comment claiming a difference it did not make.
- `pop` making the provider's requests was untested: every provider test let its writes complete
  before sealing, so each request was made inside the seal's own call. A test now seals with the
  write still in flight, so the request is made in the drain and only a pop can make it.
- `pop`'s report of a failed submission is not reached by any test. It is a declared blind spot.
  The assistant first said nothing could make the ring fail a submission on demand, and proposed a
  new ring-crate seam. That was wrong, and the next section records the correction.
- Of the old sabotages, nine had the same predicate as an existing core or backend entry, now at
  one site. They were retired rather than kept as duplicates of the same patch.
- The full sweep found a guard the swap had dropped. When a handle consumed by `end_lineage` is
  not disarmed, its release ends the lineage first. The old core reported the second end as an
  inconsistency. The new driver ignored the core's refusal there, so that sabotage survived. The
  refusal is now reported as an inconsistency through the hub.

## A failed submit on demand: the seam exists, but follows threads, not rings (2026-10-11)

The engineer asked whether the failed-submit report is structural -- a path that can never run --
or only not yet tested. It is neither. `SubmitIoRing` is documented to fail with the entries left
queued (the ring crate's `RS-P-7`), so the path is real. And the ring crate already had a way to
make it fail: the `kernel-seam` feature's responder (its `D-60`, `M26.2`), whose `submit` a test
can override. The assistant had looked for a `fault-injection`-style seam and missed it, so the
earlier claim and the `M35.1` proposal built on it were both wrong.

The engineer then asked: "It seems fragile if the seam depends on thread affinity, doesn't it?"
It does, for this crate. `D-60` makes the responder thread-local so tests running as threads of
one process cannot answer each other's calls, with the thread standing in for a test's rings. Under
pool delivery one ring is submitted to from the consumer's thread and from pool threads, and the
proxy fails both ways:

- A drain's real submit on a pool thread can undo an injected failure, so a test passes for the
  wrong reason. Each drain pass begins with a submit.
- A failure on a pool thread cannot be injected at all.

The assistant's workaround -- keeping nothing in flight so no drain runs -- depended on when the
wake happens to fire, which is the incidental behaviour the PLATFORM INTEGRITY rule forbids
binding to.

The assistant proposed registering a responder for one ring, keyed by its process-unique `RingId`,
answering on whatever thread makes the call, beside the thread-local form rather than replacing
it. The engineer: "This is a more solid plan please make it and replan/reshape the plans to make
it happen." Recorded as the ring crate's
[D-85](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-85), built in its `M35.1`, and used by
durable-ioring's `DI-3.10.3.5.6.3`.

## The thread-local responder removed, and DI-M3 flattened (2026-10-11)

The engineer: "Can we get rid of the thread-affined ring form of the ring responder? It just feels
like a bad workaround if we have the non-thread-affined variant available."

The assistant checked the thread-local form's uses before answering. The per-ring form covers
them: every call through the seam names a ring, no ring constructor makes a seam call, and every
current user names its rings. Removing it is justified on analytic grounds, not on a measurement:
for any ring used from more than one thread, the thread-local form gives a wrong answer silently.
It costs four things:

- `Resolver` is made `Send`.
- `Resolver::scoped`'s install-before-the-ring guarantee is replaced by a registration that ends
  when its ring drops, after the rundown.
- The shipped `kernel-seam` surface takes a breaking change.
- One capability is lost: answering calls a component makes on a ring of its own before handing
  the ring out. Nothing uses it, but durable-ioring's construction is exactly that case: it
  registers files and buffers and submits the registrations before returning.

The engineer: "make the plan changes including the construction time hook. also please re-do the
di-m3 plan, this depth of subitem is hard to work with."

- The ring crate's [D-85](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-85) now says the
  per-ring form replaces the thread-local one. `M35.1` builds it, and `M35.2` moves its users and
  removes `sys::install`.
- The construction-time hook is durable-ioring's `DI-7.2`: a test-only hook passed down through
  construction, never a thread-local or a global, called with the ring as soon as it exists.
- DI-M3 is closed. Its open work is now DI-M7 to DI-M12, two levels deep. The old-to-new ID table
  is in durable-ioring's archive.
- One placement changed in the restructure. The reviews, now DI-M9, gate nothing, because the
  engineer is not to be the blocking factor. They had been placed ahead of the scripted backend so
  a rework could not change the trait it implements; a rework that does is now that item's to
  absorb.

## DI-7.1: the seam needs `unsafe`, so the ring crate provides the safe form (2026-10-11)

Not a discussion with the engineer: a finding while implementing, with the assistant's choice queued for review in `DI-9.7`.

The plan for `DI-7.1` was to register the ring crate's responder on the instance's ring and fail its
submits. Writing it showed the plan could not be built as written: `Responses`' methods are `unsafe
fn`, which the crate's `#![forbid(unsafe_code)]` rejects, even in a test.
[DI-D-4](../crates/durable-ioring/DESIGN-NOTES.md#di-d-4) had already said what to do -- "A build
that fails on the `forbid` is the moment to raise it, not a lint to relax", with the safe wrapper
added to the lower crate -- so the step was re-planned before any code was written, and the ring
crate got `sys::submit_faults` (its `M37.1`, `D-86`).

Two ways round it were considered and rejected as the lint relaxed by another name: an
`expect(unsafe_code)` in the test module, and a macro in the ring crate expanding to the `unsafe`
impl inside this crate, which a lint does not see through an external macro and so would hide the
`unsafe` from the review the `forbid` exists to force. Providing a safe general `Responses` was also
rejected: the trait mirrors the FFI on purpose, and the one case needed is small enough to provide
whole.

The pool-thread test needed one thing the plan left open. Showing a failure was answered off the
test's thread is only sound if a drain is certain to run with a submit owed. Holding the engine's
lock across both pushes makes it so: the drain the first read's completion calls for blocks on the
lock, then starts with an owed submit. The responder records the thread of each failed call, which
is what lets the test say so rather than infer it from a count.

## DI-7.2: the registration paths need three behaviours, so the ring crate gains `call_faults` (2026-10-12)

Not a discussion with the engineer: a finding while implementing, with the assistant's choices queued for review in `DI-9.7`.

Reading the construction paths showed that `DI-7.1`'s `submit_faults` could not reach them. What
`Dioring::new` does between creating its ring and returning it needs three behaviours the safe
responder lacked: a `Build*` call refused, a completion's result failed, and a completion that
never arrives. The step was re-planned before any code was written, and the ring crate's `M38.1`
widened its responder to any seam call, a popped completion's result, and stalled submits (its
[D-87](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-87)), `submit_faults` staying as the submit-only
name over the same responder.

Three ways to get a completion that does not arrive were weighed. The ring crate's seeded `Resolver`
fails calls probabilistically, so it cannot be asked for one call on demand. Shortening
`REGISTRATION_BOUND` under `cfg(test)` would change every test's wait, and could fail a slow
machine's real registration. Withholding the completions the kernel has made would make
`SubmitIoRing(wait)` fail with `E_INVALIDARG` and the ring's rundown panic. What was built instead
stalls the submit, which the platform permits (RS-P-1): the operation stays queued, a bounded wait
expires on the real clock, and once the stall ends submits reach the kernel and run what is queued,
so a ring dropped mid-stall still runs down.

The ring crate's own sweep found a hole in its first test of a failed completion. The failed
completion was the first one popped, so zeroing its identity named the same operation and nothing
noticed; the test now fails the last of three in flight, whose identity cannot coincide with a
zeroed one.

In this crate the hook and the bound travel in one `Construction`, passed down explicitly, because a
thread-local would miss the calls a pool thread makes and a global would be shared by tests running
as threads of one process. A ring without the registration operation stays unreached as a cause: its
path is the refused build's, which is reached, and the capability probe is outside the seam.

## DI-7.3: pumped delivery needs a wait nobody had written safely (2026-10-12)

Not a discussion with the engineer: a finding while implementing, with the assistant's choices
queued for review in `DI-9.7`.

The item said the consumer's own thread waits on the notification and drains through a `pump`
call. Reading what existed showed the wait was the missing piece: the Windows backend's
notification is the ring's completion event as an `OwnedHandle`, waiting on a handle is
`WaitForSingleObject`, and this crate forbids `unsafe`
([DI-D-4](../crates/durable-ioring/DESIGN-NOTES.md#di-d-4)). `win-sync-sys` had parked exactly that
as its wait functions, behind `Event`
([WS-D-2](../crates/win-sync-sys/DESIGN-NOTES.md#ws-d-2)), so the step was re-planned before any
code: `win-sync-sys` takes up the single-object wait, the first slice of that item, and `DI-7.3`
waits on it. Two other homes were weighed and rejected: the ring crate, which is about rings, and a
relay through a thread-pool wait, which would put a pool thread under a delivery whose point is
that there is none.

Three design points came out of reading the ring's contract rather than being chosen in advance.
The wait has to be made outside the engine's lock, or nothing another thread does -- a push, a
provider's answer, a handle's release -- can end it. A pump has to drain even when its wait failed
or timed out, since the drain of nothing is a no-op and a delivery that cannot wait should still
deliver what completed. And the ring crate signals the event once as it attaches it
([D-20](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-20)), so a first pump returns at once and a
return promises no entries; the documentation says to pump in a loop and pop to empty.

Two mistakes of mine were caught by running rather than reading. A test that a handle without wait
access reports its failure assumed `CreateFile` would grant none; it always adds `SYNCHRONIZE`, so
that failure cannot be reached without `unsafe`, and the sabotage manifest declares it a blind spot
beside the abandoned ending, which needs a mutex. And a test of the readiness signal asserted it
unset after a pump had set it; the event is auto-reset and nothing had taken it.

One question was raised and not built: a pumped consumer that wants to wait on the ring's event
beside its own handles, which the ring crate's D-20 names as what the event's reachability is for.
Handing the event out puts a handle that can reset it in the consumer's hands, which
[DI-D-10](../crates/durable-ioring/DESIGN-NOTES.md#di-d-10) rules out for anything that interferes
with dioring's control of its ring. It is `DI-7.5`, awaiting the engineer's ruling.

## DI-7.5: the tee, and a correction (2026-10-12)

A discussion with the engineer, after `DI-7.3`, on a consumer that mixes ring I/O with other waits.
The assistant raised it as a question because handing out the ring's event has two problems: the
handle can reset the event, which [DI-D-10](../crates/durable-ioring/DESIGN-NOTES.md#di-d-10) rules
out, and the consumer's wait takes the edge the pump waits on.

The engineer asked whether a handle could be had that waits but cannot set or reset. It can: access
rights are per handle, and a duplicate with `SYNCHRONIZE` alone cannot be set or reset. That removes
the first problem and not the second. The assistant then said the only way to a private signal was a
tee, a waiter that sets a second event, and that it needed a thread. **That was wrong, and the
engineer corrected it:** a thread-pool wait holds no thread while it waits, which the ring crate's
D-3 calls threadless dispatch, and its `CompletionWake` already is exactly that waiter. The engineer
then ruled a third delivery mode.

Thinking through the shared edge turned up a defect in `DI-7.3`'s pump, which waited and then
drained. The ring crate's D-19 says a waiter must drain before it waits again, and a wait that
another handle on the same event had already satisfied would leave a pump blocked while the
completions it announced sat in the backend. Nothing shared an event until this mode, so it was not
reachable before; the mode made it so, and the pump now drains first in both pumped modes. The
test of it is at the delivery layer, over a notification that never reports a raising, because over
the real event the wake's two calls per activation set the signal again and hide the difference.

Not built: the wait-only duplicate of the ring's own event, which serves a consumer wanting no
callback at all. It needs a duplicate-with-access in `win-sync-sys`, and stays available.

## DI-7.4: the blocking push, as an example (2026-10-12)

A discussion with the engineer. Asked to explain the `DI-7.4` question and why they would or would
not decline, the assistant set out both sides. For: back-pressure as a convenience, and a loop that
is subtle to write for three delivery modes. Against: it is not a capability, since the refusal
names its cause and hands the buffer back; the policy -- wait, shed or queue -- is the caller's; and
a blocking call that a pool thread or a callback can hold up deadlocks, which a library method
invites. The assistant leaned to parking it, offering "too early to say" as an answer.

The engineer's reply changed the question: build it as an example, "an on the side helper", a
maintained source the consumer takes literally or adapts. That keeps what the assistant valued in
building it -- the delivery cases got right once -- and drops what it feared, since an example can
state the deadlock rule plainly and leave the policy to the caller. It also made the test of the
claim concrete: an example built only on public API is the proof that the API is enough. It was,
and `PushRefusal::Backend`, `RefusalCause::is_retryable`, `pump` and `completion_signal` were all
that was needed.

Building it found a defect the library's own tests had not. The first test of a full queue dropped a
pumped instance with its completions unpopped and an entry still queued behind them, and the ring's
rundown failed with `IORING_E_COMPLETION_QUEUE_TOO_FULL`: `SubmitIoRing` with a wait reports that
whatever it waits for when the queue is full, and rundown waited before it reaped. It is the same
rule as the completion event's edge, drain then wait, applied to the one waiter that did not follow
it. It was fixed in the ring crate rather than worked round (its `M39.1`, `D-88`). Two false starts
on the way, both caught by running rather than reading: a ring-level reproduction that did not
reproduce, because it filled the queue without leaving an entry queued behind it, and a first fix
that reaped before the quiescence check, which broke four unit tests over a ring whose handle the
kernel refuses, since a bound of zero makes no kernel call.

## WS-1+.5: a handle with chosen access, and a premise that was false (2026-10-12)

Asked whether the wait-only duplicate of the ring's event needed a checklist item, the assistant found
it recorded only as prose, in `DI-D-55` and a done-note, and queued it as `WS-1+.5` in `win-sync-sys`
and `DI-7+.1` in this crate; the engineer then asked for `WS-1+.5`.

The item said a duplicate "can only narrow", on the assistant's reading that Windows would refuse a
duplicate with a right its source lacks. The first test written for it, a refusal expected when
widening a `SYNCHRONIZE`-only handle, failed: Windows allowed it, and the widened handle set the
event. A short probe over several masks confirmed it, and showed one mask Windows does refuse, every
bit set, which became the error path's test. Nothing had bound to the claim yet, so what changed was
the documentation of the function, of the new type, of the README and of `DI-7+.1`, and a test that
records the platform behaviour: a wait-only handle cannot be used to signal, and is not a boundary
against a holder that duplicates it wider, which no in-process handle is.

The type question the item left open was settled as a distinct `WaitOnlyEvent`, since an `Event` with
reduced access is an `Event` whose `set` fails, and this has no `set`. The function is over any handle
and returns an `OwnedHandle`, because the first consumer holds the ring's event as one and whether a
handle is an event cannot be asked. A thread-pool conversion for the new type was queued in
`windows-threadpool-sys` as `M-T14+.1` rather than built here, since it is that crate's `From` and its
release.

## Open, not yet discussed

- Where the crate's checklist and design notes live, and the `M33+.5` amendment.
- **Is `Epoch` public vocabulary?** It follows from the naming argument: if the epoch is the
  mechanism, exposing it binds consumers to the mechanism. Release-after-durable still needs a
  public, totally ordered name for "the durability point covering this write". Assistant's early
  lean: an ordered high-water-mark type whose meaning the contract defines, with "it is an epoch"
  documented as the current implementation rather than promised. Not discussed further yet.
- Whether the type mirrors `IoRing`/`Batch` surface closely, and how a push reports its epoch.
- The ring's sidecar: the epoch layer needs its own per-operation metadata (epoch, file), which
  composes with `IoRing<T, X>` ([D-73](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-73)) but
  the exact shape is open.
- Flush modes per file (`FlushMode`), write-through as a latency knob, and whether device facts
  (write cache disabled, atomic write unit) are inputs the caller declares.
- Whether a file-set "flush regime" is declared by the caller -- the `M33+.5` musing about
  flush equivalence, which this crate is the natural home for.
- The containment direction between the epoch ring and a future domain.
