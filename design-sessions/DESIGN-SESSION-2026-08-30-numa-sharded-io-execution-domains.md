# Design session -- NUMA-sharded I/O execution domains (2026-08-30)

> Tier-3 record. [DESIGN-NOTES.md](../DESIGN-NOTES.md) is authoritative and wins
> on any conflict. This file records how the discussion went, what was measured,
> and what is still open.

**Status: OPEN. This session has only just begun.** What follows is the opening
survey and the first measurement, not a set of converged decisions. No decision
below is settled unless it says so explicitly.

Repo-wide by scope: it touches
[windows-ioring-sys](../crates/windows-ioring-sys/DESIGN-NOTES.md),
[windows-threadpool-sys](../crates/windows-threadpool-sys/README.md) (which has no
DESIGN-NOTES.md of its own),
[windows-topology-sys](../crates/windows-topology-sys/DESIGN-NOTES.md), and the
deferred namespace facility designed in
[DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md](DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md).

## Starting intent

The engineer's framing: a high-performance flow of deferred `CreateFile` ->
offloaded `CreateFile` -> `IoRing` (or several), with NUMA-affined buffers
backing sector-aligned I/O, organized as Seastar-style shards over queues.

Stated afterwards, and worth recording because it shapes how the material below
should be read: the topic was seeded to see what would grow from prior work
planted around the repository, rather than started from nothing.

## What already exists, so it is not redesigned

- **The namespace/data plane split is decided.** Win32 is asynchronous on the
  data plane (overlapped I/O, `IoRing`) and synchronous-only on the namespace
  plane (open, delete, rename, attributes). "Deferred `CreateFile` feeding an
  `IoRing`" is precisely a namespace-plane operation handing to a data-plane one.
- **The Win32 ring is not the `IoRing`** (that session's decision 7): share the
  ring type, not the storage, and unify at the wait.
- **The namespace facility owns no threads**: pooled, elastic, `runs_long`
  mandatory, quarantine ceiling.
- **Model B is already specified** in the ioring crate's
  [DESIGN-NOTES.md](../crates/windows-ioring-sys/DESIGN-NOTES.md), naming the unit
  as an execution domain -- one pinned thread, its ring, its node-local
  registered buffer pool, its shard of the work -- with
  [D-27](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-27) arguing pinning is
  what makes per-thread a proxy for per-CPU.
- **`ring_copy` already implements it end to end**: `SetThreadGroupAffinity` per
  domain, `VirtualAllocExNuma` buffers, one ring per domain, policy selectable
  as `ByL3` / `ByNode` / `ByPackage` / `ByCore` / `Single`.
- **[D-8](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-8) reserves the
  abstraction** being discussed here: a `RingFleet`-style layer, deferred until
  "there is evidence about what sharding actually helps."

## Constraints established during the session

- **Registration is one-shot per ring, for both buffers and file handles.**
  Verified in `Batch::register_buffers` and `Batch::register_files`: a second
  call is refused because `BuildIoRingRegister*` **replaces the whole table**,
  invalidating every index already handed out. A freshly opened handle therefore
  **cannot be added to a running ring's file table**. Any "deferred `CreateFile`
  -> ring" pipeline that expects to register each new file hits this on
  operation two.
- **Ring count is forced to equal pinned-thread count**, because the submission
  queue is not thread-safe. Choosing a coarse policy does not give many threads
  sharing few rings; it gives *few threads*.
- **Processor groups are a hard floor**: above 64 logical processors the
  partition is forced whether wanted or not.
- **No library in this workspace can pin a thread.** `windows-topology-sys` is
  read-only and reports `ProcessorSet` without applying it;
  `windows-threadpool-sys` has no affinity, NUMA, or pinning support at all. The
  only pinning in the repository is inline in the `ring_copy` example.
- **No SPSC or MPSC queue exists in this workspace.** There is a domain-specific
  bounded queue in `windows-file-watcher` and `std::sync::mpsc` in a probe.
  Nothing general, and no `crossbeam` dependency.

## Crossbeam: assessed, and it does not provide the doorbell

The engineer asked specifically about crossbeam's doorbell mechanism and its
buffer management. Checked against the published API rather than reputation:

- **`crossbeam-queue`** (`ArrayQueue`, `SegQueue`) is lock-free MPMC. `pop`
  returns `Option`; it never blocks and never signals. There is nothing to wait
  on.
- **`crossbeam-channel`** blocks in `recv`, but parks on its own internal
  primitive and exposes **no waitable HANDLE**. Its `Select` is built purely
  from channel operations (`sel.recv`, `sel.send`); there is no method to
  register a foreign OS object.

So the mismatch is symmetric and fatal for a Model B shard:
`WaitForMultipleObjects` cannot see a crossbeam channel, and crossbeam's
`Select` cannot see the `IoRing` completion event. A shard that must park on
"my ring completed something **or** a peer sent me work" cannot express that
wait with crossbeam, and would have to poll one while blocking on the other.

**This workspace already solved the doorbell problem** in
[queue.rs](../crates/windows-file-watcher/src/queue.rs), whose module
documentation states the general principle: on Windows a HANDLE **is** the
universal waitable currency, so an event is the native composition point rather
than a lowest common denominator. It hands out a lazily created manual-reset
event, signalled under the same lock a receiver holds while deciding there is
nothing to take, "so a wakeup cannot be lost in the gap between those two
decisions, because there is no gap" -- the same lost-wakeup hazard class as
[D-19](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-19)'s edge-triggered ring
contract.

**Buffer management, two senses, and conflating them is a trap.** `ArrayQueue`
allocates a fixed buffer at construction and fails `push` when full -- that
failure is the backpressure signal. `SegQueue` is unbounded, allocates segments
on demand, and needs deferred reclamation, which couples shards that were
supposed to share nothing. But either way crossbeam manages **message** storage,
never **I/O buffer** storage: the I/O buffers are the registered pool, allocated
once, NUMA-affined, sector-aligned, one-shot per ring. A cross-shard queue must
carry descriptors (buffer index, handle, completion record), never bytes -- if it
carried bytes, the copy would have defeated the reason for registering.

*Not yet decided:* whether to adopt `crossbeam-queue` for the data structure and
add a doorbell beside it, or build the queue with the doorbell integral. The
argument for integral is the file-watcher's: an external doorbell cannot be
signalled under the queue's own lock, which reintroduces the gap.

## What "shard" means here

Asked directly, and answered: the shard is **the execution domain -- one pinned
thread**, with its ring, its node-local registered pool, and its slice of state.
It is **not** the last-level cache domain. L3 is one *policy* for deciding how
many shards and where to pin them.

A consequence that is easy to miss: because ring count equals thread count,
selecting `ByL3` on a 64-core, 8-CCX part does not yield 64 threads over 8 rings.
It yields **8 pinned threads in total**. That is correct for `ring_copy`, a
bandwidth-bound copy where a few threads saturate memory, and probably wrong for
a general execution substrate, where Seastar shards per logical core and uses
NUMA only to place each shard's memory. The repository currently has one answer
where the effort may need two.

## Measurement M-1: a shipping ARM laptop reports no L3 at all

Probed with `Topology::discover()` on the development machine.

**Snapdragon(R) X2 Elite - X2E80100 - Qualcomm Oryon(TM) CPU**, 12 cores, 12
logical processors, no SMT:

```
processor groups : 1 {0: 12}
domains by kind  : Cache 26, Core 12, Group 1, Memory(NUMA) 1, Module 2, Package 1
cache by level   : L1 -> 24 domains (1 processor each)
                   L2 ->  2 domains (6 processors each)
                   L3 -> none
pinned-thread count each ring_copy policy would produce:
  ByCore 12 | ByL3 0 | ByNode 1 | ByPackage 1 | Single 1
```

Corroborated by WMI: `L3CacheSize = 0`, and **zero `Win32_NumaNode` instances** --
the same signature the ioring notes already recorded for the machine they were
investigated on.

**What this does and does not show.** An initial reading of this session claimed
`ByL3` returning zero domains would create zero rings. **That was wrong, and
checking the code corrected it**: `Policy::select` falls through to a
whole-machine domain when the preferred relation matches nothing, and reports
`degraded = true` so the sample can say so honestly. There is no defect in
`ring_copy`.

What the measurement does falsify is narrower and is in the prose. The ioring
notes justify the L3 heuristic partly by saying it "is meaningful on Intel and
ARM too, where the NUMA node often is not." On this ARM part L3 is **not**
meaningful, because it does not exist; the natural cluster boundary is **L2**,
two domains of six, corroborated by the two `Module` domains the same probe
reported. The heuristic is still right that L3 beats the NUMA node. The durable
idea underneath it appears to be "the outermost cache level that actually
partitions the machine," not "L3 specifically" -- and on this machine the
difference is between describing two clusters and describing one whole machine.

Whether the two-cluster structure is worth using is a separate question this
session has not answered.

## Finding F-1: a file handle's NUMA node is reachable, and answers a coarser question

Contributed by the engineer as research, **not measured here**. It corrects the
ioring notes on mechanism while leaving their conclusion standing for a better
reason.

- **`FSCTL_QUERY_VOLUME_NUMA_INFO` is documented and takes a file or directory
  handle directly**, returning `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT { ULONG
  NumaNode }`. Confirmed independently against the IFS documentation during the
  session. The ioring notes say this mapping "has no clean user-mode path" and
  "means walking volume to disk to device instance and reading
  `DEVPKEY_Device_Numa_Node`". **That is wrong**: there is one documented call,
  and it accepts the handle a caller already has.
- **What it returns is the node the *volume* resides on**, not where the file's
  extents live. NTFS does not expose per-file or per-extent NUMA through this
  API, and nothing states that `FileNumaNodeInformation` is filled from MFT or
  runlist locality.
- **`GetNumaNodeNumberFromHandle` is the other path**: a Win32 wrapper over
  `NtQueryInformationFile` with `FileNumaNodeInformation` (class 53, Windows 7
  and later), yielding `FILE_NUMA_NODE_INFORMATION { USHORT NodeNumber }`. PHNT
  and the WDK mark that class **reserved for system use**. Documented Win32
  behaviour when there is no node is `FALSE` with an undefined `NodeNumber`.
  This crate must not build on it.
- **The volume node exists only when the device layer advertised one**:
  `IoGetDeviceNumaNode` on the PDO, or user-mode
  `DEVPKEY_Numa_Proximity_Domain` with `GetNumaProximityNode`. A single-node
  machine, a PDO returning `STATUS_NOT_FOUND`, or a software or virtual disk
  with no proximity data is precisely the "no association" case.
- **No published experiment could be found** showing either call succeeding on a
  garden-variety NTFS data file and naming a node. There is also no evidence
  that success depends on `FILE_FLAG_NO_BUFFERING`, on overlapped I/O, or on
  which process opened the file.

### F-1a: one weak datapoint from the vacuous machine

The spike was smoke-run on the development machine, not for an answer but to
prove the instrument works before handing it to someone with real hardware. It
was worth doing twice over.

**It found a defect in itself.** The first version opened the directory for Q5
with `File::open`, which fails on a directory without
`FILE_FLAG_BACKUP_SEMANTICS`, so Q5 could never have been answered. Corrected to
`CreateFileW`. An instrument checked in unrun is one whose bugs are still in it;
running it on hardware where the *result* is vacuous still validates the
*apparatus*.

**And it does establish one thing, narrowly.** On ARM64 Windows, single node:

```
regular NTFS data file  : FSCTL ok, NumaNode = 0 | GetNumaNodeNumberFromHandle ok, NodeNumber = 0
directory handle        : FSCTL ok, NumaNode = 0 | GetNumaNodeNumberFromHandle ok, NodeNumber = 0
```

Both calls **succeed** on a garden-variety NTFS data file, and agree. That is
directly responsive to "no published experiment shows
`GetNumaNodeNumberFromHandle` succeeding on a garden-variety NTFS data file":
here it does. It also shows the FSCTL accepting a directory handle, as the IFS
docs say.

**What it does not establish**, and the distinction is the whole value of the
result: node `0` is the *only* node this machine has, so neither call is shown
to name a *meaningful* node. What is refined is the negative case -- the
documented "returns FALSE when the object has no node" did **not** occur here,
so "ordinary NTFS file" is not itself the absent case. Absence must come from
the device layer advertising no proximity domain, which is exactly what cannot
be reproduced on this hardware.

**Why this matters to the seam, and it is an opportunity rather than a
problem.** The discriminating check is to call both on the same handle: if they
agree, what is being observed is volume locality. And volume locality, though
coarse, arrives at exactly the right moment -- the namespace worker has the
handle in hand at the instant it completes the open, so a routing key for
"which domain should own this file's I/O" is available **for free at the seam**,
with no extra open and no device-tree walk. That does not make automatic
placement correct, and the conclusion below stands, but it does mean the
information is cheaper than the notes imply.

The conclusion the notes draw survives, restated: the crate should still not
offer "put this file's I/O on the right ring," because the answer is
volume-granular, frequently absent, and meaningless for spanned volumes and
Storage Spaces where one volume sits on several devices. It also still does not
pin thread-pool completions.

**Named blocker, per the repository's deferral protocol.** Settling this
empirically needs a multi-node machine with storage whose PDO advertises a
proximity domain. The development machine has one node and reports zero
`Win32_NumaNode` instances, so any run here is vacuous: failure would prove
nothing and success could only report `0`. The instrument is therefore checked
in unrun as
[file-handle-numa-spike.rs](../crates/windows-ioring-sys/design-sessions/spikes/file-handle-numa-spike.rs),
with the hardware gap stated in the spikes
[README.md](../crates/windows-ioring-sys/design-sessions/spikes/README.md). The
documentation defect is independent of the measurement and is queued as M20.4
regardless.

## Converged: one uniform, tunable architecture, sized by the topology

This is the session's first converged position, and it arrived by correcting a
framing this record had already adopted.

**The framing that was wrong.** An earlier turn concluded that because almost
every consumer machine yields a single domain, "this entire apparatus is
server-class-only, and should be sized and justified as such." The engineer
rejected that, on the grounds that it sounds like the feature does not work on
laptops, and that it should extend up and down smoothly instead.

That objection is right, and it is right against a **standing rule of this
repository** rather than merely on taste. PLATFORM INTEGRITY says: "do not
narrow the platform to serve the visible goal -- every platform component must
remain a *level* platform, its lower baselines first-class, not optional
trimmings to cut because the current task does not need them." Scoping the work
to server hardware is exactly that narrowing. The ioring notes already had the
correct words where this session lost them: a machine that yields one ring is
"correct", not degraded.

**The architecture.** There is one shape at every size -- a domain is a pinned
thread, its ring, its node-local registered pool, and its shard of the work.
A laptop runs one. A server runs several. There is no laptop mode and no server
mode; there is one shape and a count. N=1 is not a fallback that lost something:
on a single-node, single-LLC machine one domain **is** the optimal partition.

**What is genuinely additive above one domain**, and it is additive rather than
a second mode:

- the **cross-domain** queue and its doorbell -- with one domain there is no peer
  to message. **This is not the client-facing queue**, and the distinction
  matters enough to state plainly, because reading "the queue" as "all queues"
  would wrongly suggest no queue work is needed for the first deliverable.
  There are two roles:
  - **client to domain (the SQ), and domain to client (the CQ)** -- needed at
    **every** size including N=1, because a foreign client thread must still
    reach the single domain. This is the two-layer ring, and it is on the N=1
    critical path.
  - **domain to domain** -- genuinely absent below two domains.
- the routing policy -- with one domain there is no choice to make, so the
  volume-to-node key has nothing to select.

Both are **absent** at N=1, not stubbed or bypassed. Nothing on the
single-domain path consults a router that always answers zero.

**Consequence for build order, and it inverts what this session had implied.**
N=1 is the **first deliverable and the substrate**, not the leftover. It is the
common case, it is complete and correct on its own, and N>1 extends it without
disturbing it. That is a better sequencing argument than starting from the
fleet, and it means the first thing built is useful on every machine in the
table below rather than on none of them.

**The mechanism that makes "smooth" concrete: affinity is a set, not a point.**
`SetThreadGroupAffinity` takes a mask, and `windows-topology-sys` already hands
out `ProcessorSet` with correct multi-group handling. A domain's affinity is
simply the `ProcessorSet` of its partition:

| Machine | N | Each domain's affinity set |
|---|---|---|
| Uniform laptop or VM | 1 | the whole machine |
| Heterogeneous laptop (P/E, or ARM clusters) | 1 | the performance cluster only |
| 2-CCD desktop | 1-2 | each CCD's processors |
| 12-CCD EPYC | 4-12 | each CCD's processors |

Same call, same type, different set. Nothing special-cases the small end.

**Domain count and pinning tightness are separate knobs.** Pinning to a single
core buys locality *relative to other domains*; with one domain there is nothing
to be local relative to, and hard-pinning an I/O thread to one core of a laptop
that is also running everything else may be worse than letting it float across
the performance cores. But **heterogeneity means even N=1 wants an affinity
mask**, because an unconstrained thread can be scheduled onto an efficiency core
or an LPE island. The development machine is the case in point: two clusters of
six, and `efficiency_class` is already exposed on `Core` by the topology crate.
So the small end does not want *no* affinity -- it wants a *set*, which is
exactly what the large end wants too.

Stated once: **one mechanism, sized by the topology.** A domain is affinitized
to a `ProcessorSet`; how many domains exist, and how wide each set is, falls out
of the machine.

## Converged: round-robin is incoherent with the model, not merely suboptimal

The engineer's position was that round-robin assignment "seems actually
dangerous" and that a high-performance consumer might prefer a single thread on
a single processor. Agreed, and the reason is stronger than the averaging
argument that first suggested it.

Round-robin across domains **breaks the ownership premise Model B exists for**.
If a file's I/O lands on domain 1 now and domain 3 next, that file's state --
buffer slots, outstanding accounting, continuations -- is owned by no single
shard. That is sharing on the data path, which is the one thing shared-nothing
is defined by. It is not a worse point on the same curve; it is off the curve.

The consequence for a latency-sensitive consumer follows directly: a single
thread on a single processor is deterministic and owned, where round-robin is
non-deterministic and shared. A known cost can be engineered around; jitter
cannot.

**Position:** round-robin is acceptable only as an explicitly chosen default for
consumers who have expressed no preference, and it should be named to admit what
it is rather than offered beside "by node" as though it were a peer policy.

## Converged: report, do not route

The engineer proposed that when no useful mapping is available, the facility
should message the caller and have them respond with how to proceed.

**One mechanism correction:** an arbitrary completion cannot be posted into an
`IoRing` completion queue. The namespace session already rejected that path --
"the `IoRing` API has no post/user-completion entry point, so a namespace
completion cannot be placed in its CQ." (`IORING_OP_NOP` can inject a marker,
but only the ring's owning thread may submit, so a namespace worker on another
thread cannot reach in.) On the facility's **own** ring, which it owns outright,
posting is available.

**A simpler form needs no new mechanism at all.** Rather than the facility
asking a question and awaiting an answer, the open's completion carries the hint
and the client routes:

```
completion = { handle, volume_node: Option<u32>, provenance: Measured | Absent | Overridden }
```

The client already receives that completion. No round trip, no pending-decision
state, no "what if the client never answers," and no new queue direction. It is
the principle the ioring notes already state -- "leaves the mapping to whoever
knows their storage layout" -- made concrete: the facility reports, the client
routes.

The upcall form remains the right answer for a higher layer that owns a flow
end to end and cannot hand control back. Both are kept, with the reporting form
as the primitive and the upcall as something built on it if a consumer needs it.

## Converged: a domain runtime is not a thread pool

The engineer raised a fear of ending up writing a thread pool, given that the
Windows pool cannot affinitize to any of the objects of interest.

What Model B needs is a **domain runtime**, and every distinguishing feature is
the *absence* of a thread-pool feature:

| A thread pool does | A domain runtime does |
|---|---|
| dynamic sizing, injection, retirement | spawn N at startup, join at shutdown |
| work stealing and load balancing | nothing -- stealing would violate share-nothing |
| queue management, priorities, quarantine | one loop per thread: park, drain, run |
| grow under blocking (`runs_long`) | never blocks on namespace work by construction |

`ring_copy` already contains a working instance of it inline: pin, allocate
node-local, register, loop. The ioring notes already committed to the shape --
"Model B on the hot data path ... and Model A for the control plane, background,
and cold paths" -- and `M6+.4` already queues binding a ring's thread with
`SetThreadGroupAffinity`.

The load-bearing point is the one that motivated the fear: the Windows pool
**cannot** affinitize, so this capability cannot come from it. That is not a
reason to rebuild the pool; it is the reason the two halves are separate. The
namespace plane keeps the pool because it needs quarantine and elasticity for
blocking calls; the data plane owns threads because it needs pinning. Neither
can serve the other, and that is the design rather than a compromise.

## The target architecture: a two-layer ring

The engineer's refined vision: an SQ/CQ against an "async API substrate". You
post a deferred `CreateFileW`; the CQ may return a request to clarify NUMA
placement (behind an option set on the SQ, so the complexity is opt-in); you
answer; you get back a token for high-performance I/O using the epoch/durability
metaphor. All ring-based, NUMA-aware, "without a lot of exotic client
programming".

**The structural resolution is that the client-facing ring is ours, and the
`IoRing` lives inside a domain as an implementation detail.**

```
client thread --post--> [ our SQ: MPSC + doorbell ] --> domain thread --> IoRing SQ
client thread <-drain-- [ our CQ: MPSC + doorbell ] <-- domain thread <-- IoRing CQ
```

Four problems collapse into that one decision, which is the main reason to
believe the decomposition is right:

- **Submission thread-safety.** Only the domain thread touches the real SQ. This
  is a *third* answer to `M6+.2`, which is parked with "needs either a
  submit-ownership handoff or an internal lock. Neither is obviously right" --
  the answer is neither: a queue only the domain drains.
- **The missing post entry point.** The namespace session rejected posting a
  completion into an `IoRing` CQ on mechanism. Our CQ is ours, so the
  clarification CQE the vision needs becomes possible.
- **One ring or two.** The client sees one; the namespace and data planes keep
  separate storage. That is decision 7 -- "share the ring type, not the storage;
  unify at the wait" -- seen from the client's side.
- **The epoch metaphor ports**, because it is already written against a CQ.

### The client never allocates an I/O buffer

This, rather than the ring shape, is what removes the "exotic client
programming". The facility owns the registered pool: `VirtualAllocExNuma` on the
domain's node, sector-aligned, registered once. The client acquires a **slot**,
writes into it, submits, and gets it back.

The client cannot place a buffer wrongly because it never chooses. Every
alternative -- client allocates and we validate, client hints and we advise --
returns the decision to where the knowledge is not. It also fits the one-shot
registration constraint exactly: the pool is the long-lived fixed thing, so
registration's principal limitation stops being one.

### Three policy tiers, generalized beyond placement

| Tier | Client says | Facility does |
|---|---|---|
| Default | nothing | picks; at N=1 there is no choice, so this is free |
| Informed | "tell me" | completion carries the hint and its provenance; client routes |
| Consulted | "ask me" (SQ option) | posts a clarification CQE; client answers |

The engineer generalized this past NUMA: most things "need to just be taken care
of for people", but policy in general must be expressible either as **optional
parameters** or as **queries via CQE/SQE pairs**. So the tiers are the shape for
every policy decision the facility must make, not a placement-specific device.

## Costs of the target architecture, and their mitigations

### C-1 The queue hop

Every foreign-thread submission crosses an MPSC push plus a doorbell before
reaching the ring, where a run-to-completion client would have none.

- **The push is cheap; the doorbell is the syscall.** Signal only on the
  empty-to-non-empty edge, and only when the consumer is parked. A queue that
  stays non-empty needs no signal at all, so a busy domain -- the case that
  matters -- pays approximately zero doorbells.
- **A brief consumer spin before parking** removes the park/unpark round trip
  under load. Spin duration is another knob sized by the topology: generous when
  a domain owns a core exclusively, zero when it shares one with the rest of a
  laptop.
- **The hop is where batching happens.** N submissions become N pushes, one
  doorbell, one drain, and **one** `SubmitIoRing`. Under load it plausibly
  reduces syscalls rather than adding them.
- **It should be pay-for-what-you-use**: a client whose continuation runs on the
  domain thread submits directly, with no queue and no doorbell.
- **Measurable now**, on this hardware, with no infrastructure: time `SetEvent`,
  an uncontended atomic push, and a `SubmitIoRing` round trip. If `SetEvent` is
  a few percent of the syscall, the hop is noise.

### C-1a Why the doorbell must be a HANDLE, and cannot be `WaitOnAddress`

`WaitOnAddress` is plausibly cheaper in isolation. It is still unusable here,
and cost does not enter into it: `WaitOnAddress` waits on a **memory location**,
`WaitForMultipleObjects` waits on **kernel objects**, and no API combines them.
A domain that must wake on either "my ring completed" or "a peer sent me work"
waits on both at once, and the ring's completion event is a HANDLE.

This is the same structural constraint reached from a different direction than
the crossbeam analysis above, which is good evidence it is a property of the
platform rather than of a library.

The two ideas also interact decisively: **the skip-when-busy rule removes the
very cost `WaitOnAddress` would save.** They are alternatives, not complements,
and only one of them composes.

A corollary worth stating plainly: a domain parked in the fused
`SubmitIoRing(wait_n)` cannot observe a queue at all. **A domain that accepts
foreign submissions must use [D-20](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-20)'s
multiplexed-wait row.** Accepting foreign work and using the lowest-overhead
park are mutually exclusive.

### C-1b Which side of the lock the doorbell is touched on

Asked directly by the engineer: why can the event not be signalled after the
lock is released, and does it have to represent the fullness of the queue?

**It can be signalled outside the lock. Only the reset must be inside.** A late
`SetEvent` can at worst arrive after the consumer already drained that item and
parked, producing a spurious wakeup -- the consumer wakes, finds nothing, parks
again. A reset outside the lock is fatal:

```
Consumer: lock, drain to empty, unlock
Producer: lock, push(B), unlock, SetEvent
Consumer: ResetEvent          <-- clears the signal for B
Consumer: park                <-- lost wakeup; B is stranded
```

So the invariant is precisely: **the reset must be atomic with the observation
that there is nothing to take.** And yes -- the event represents the fullness of
the queue. It is *level* state, a function of the contents rather than a record
of edges, which is why it is manual-reset and why a redundant signal is free
while a stale reset is fatal.

### C-2 The pending-clarification handle -- dissolved

Deliver the handle **with** the question rather than holding it behind the
question. The client then owns it under ordinary rules; a client that never
answers has leaked its own resource, not stranded a facility-held object with no
owner and no deadline.

Following that through: **at the primitive layer the consulted tier collapses
into the informed tier.** Ask what the facility would do with the answer.
Register the file into that domain's ring? Unavailable -- registration is
one-shot. Allocate slots on the right node? Those come from the domain when the
client asks it. There is no work the answer unlocks that the client cannot do by
submitting to the domain it chose. The round trip has no payload.

The consulted tier survives where the facility performs I/O **on the client's
behalf** -- a higher-level "read this whole file" API that must choose placement
and has nobody to ask. That is a layer above the primitive.

Whatever the facility still holds transiently needs a deadline and a disposal
path **allowed to block**, since closing a handle to a dead network path is
exactly the work this facility exists to keep off a caller's thread.

### C-3 The durability model graduates to its own crate

Recorded as the engineer's decision: the `epoch_log` sample becomes a canonical
layer, probably a separate crate. Durability groups are a natural capability
whose absence is surprising.

It composes because the mechanism it needs was already measured:
[D-23](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-23) (an unflagged flush
does **not** cover preceding writes) and
[D-24](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-24)
(`DRAIN_PRECEDING_OPS` is a full ring-wide barrier spanning submissions). Group
commit is policy over that mechanism, which is a textbook reason for a separate
crate rather than a feature.

**One constraint to carry from the start:** the barrier stops at the ring's
edge, so a durability epoch is **per-domain**. A client writing through two
domains and wanting one durability point needs two flushes and an explicit join.
The crate must represent that or refuse it; it must not quietly imply an epoch
spans domains.

### C-4 The composed layer swallows the sharp edges

The engineer's direction: primitives matter, but this is the "build layers that
compose them" phase, and the composed layer should not expose sharp edges.

The mechanism is already recorded as the namespace session's decision 8 -- a
type-level traversal where each step offers only the legal next steps -- and
this repository already applies it (`RingScope` so no `&mut IoRing` escapes;
`get(&mut self)` so a hazard is a type error). The edges to swallow:

- edge-triggered drain ([D-19](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-19)):
  drain to empty every pass; a single `try_pop` deadlocks;
- buffer slot lifecycle: acquire, outstanding accounting, release only on an
  observed completion;
- one-shot registration: fixed at construction, never named by the client;
- token claiming: `claim_if` matching both `user_data` and `ring_id`;
- flush barrier semantics (D-23);
- handle disposal, including the blocking close.

**If the client can name any of these, the layer has leaked.**

## Rejected: a `Ring` trait with a client-implemented `ring_doorbell()`

Proposed during the session, and rejected -- but the first argument offered
against it was wrong and is corrected here rather than quietly dropped.

**The withdrawn argument.** It was claimed that a client callback would run
under the queue lock and therefore risk deadlock. C-1b shows the signal side
need not be under the lock at all, so that argument does not hold.

**The arguments that do hold:**

1. **Set and reset are two halves of one invariant** and cannot be split across
   an ownership boundary. The reset must happen under our lock, atomic with the
   emptiness observation; if the client owns the signalling object, we cannot
   perform it. This is the file-watcher's recorded reason: owning the doorbell
   "makes the reset discipline an internal invariant rather than a client
   obligation".
2. **A client callback on the producer's submit path is a cadence hazard** -- if
   it blocks, it stalls the producer.
3. **Type-parameter propagation**, which is the cost the file-watcher actually
   measured: a `Doorbell` trait "would have made `Monitor`, `Session`, and
   `Sender` all generic over it".

And the extension point already exists at no cost: hand out the HANDLE. The
client composes it into `WaitForMultipleObjects`, a `ThreadpoolWait`, an async
reactor, or ignores it -- outside our lock, on their own schedule, with no type
parameter reaching `Ring`, `Domain`, and every producer handle.

On the narrower question of generics versus `dyn` should polymorphism be needed
elsewhere: static dispatch on the hot path, but the cost that actually bites is
the type parameter infecting every type that touches a queue, not the dispatch.

## Two locality consumers, not one

Raised by the engineer's question about what a client gets back and whether it
is affinitized to the calling thread. It exposes an incompleteness in the ioring
notes' strongest placement claim.

There are **two** consumers of a buffer's locality:

- the **device**, which DMAs into it;
- the **client**, which reads it after completion.

The notes argue placement dominates because "a buffer on a node remote from the
device means every byte crosses the interconnect, on every operation, forever".
That is true of the DMA and silent about the read side. With the device on one
node and the client thread on another, both cannot be satisfied:

| Buffer placed | DMA cost | Client read cost |
|---|---|---|
| near the device | local | remote, on every read |
| near the client | one crossing | local |

**A DMA writes once; a consumer may read many times.** So "near the device" is
not automatically right -- it wins when the consumer barely touches the bytes,
and loses when the consumer works over them repeatedly. The notes' claim is
incomplete rather than wrong, and the completion is that the *access pattern*
decides.

**On binding to the calling thread: no, not implicitly.** An unpinned client
thread migrates, so a binding inferred from where it happened to be is stale
before it is used. That is the namespace session's decision 9 -- "ambient state
is derived from an explicit binding, never the origin of one".

**Proposed instead:** the client *asks* -- "which domain is nearest me?" -- and
then uses it explicitly. Topology becomes an input the client may consult, never
an authority applied behind its back. A foreign thread has in any case already
accepted a hop and probably a remote read; a client wanting true consumer
locality should be running *on* the domain.

## Coherence assessment

Honest state of the design at the end of this session, since it was asked
directly.

**Solid.** The two-layer ring resolves submission thread-safety, the missing CQ
post entry point, and "one ring or two" with a single decision; three
constraints falling to one choice is usually a sign the decomposition is right.
The domain-owned pool makes NUMA invisible rather than merely easy. The uniform
architecture sizes from 1 to N without a second mode.

**Structurally open, and not to be papered over:**

1. **The client-facing ring is a substantial new artifact** -- MPSC, doorbell,
   descriptor format, completion tagging -- existing nowhere in this workspace.
   It needs a home and probably its own crate, and it is larger than anything
   this session has called "the seam".
2. ~~**CQ cardinality.**~~ **Resolved** -- one CQ per domain, client chooses the
   observation strategy. See "Resolved: CQ cardinality" above.
3. ~~**Whether the client ever sees an `IoRing`.**~~ **Resolved by the
   engineer: it does not.** `windows-ioring-sys` becomes an implementation
   detail of the higher crate. This is layering rather than absorption -- it
   remains a published crate in its own right (0.2.0 shipped 2026-08-30) and
   gains a dependent; direct consumers can still use it.

## Specification: the submission and completion queues

Requested as a specification rather than a sketch. These are the requirements
the session's conclusions imply. The two directions are **not** the same shape.

**R1 Cardinality.** The SQ is **MPSC** -- many client threads, one domain
thread. The CQ is **SPSC** -- one domain thread, one drainer. The CQ constraint
is deliberate: "drain to empty" is ambiguous with two racing drainers, and
drain-to-empty is what
[D-19](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-19) requires. Nothing is
lost, because per-domain CQs already give a client N drainers; a client wanting
parallel processing drains on one thread and dispatches.

**R2 Bounded.** Fixed capacity at construction. `push` on a full queue returns a
typed error; it never blocks and never grows. **That failure is the
backpressure** -- an unbounded queue has none, which is why `SegQueue` was the
wrong model.

**R3 Lock-free producers.** No mutex on the producer path. A producer-side lock
serializes precisely what multi-producer exists to parallelize. Park and notify
go through an **eventcount**: the consumer publishes intent to park, re-checks
the queue, and only then waits. That re-check closes the lost-wakeup gap without
a lock.

**R4 Doorbell.** A queue-owned **manual-reset event**, created **lazily** so a
polling-only consumer allocates no kernel object. Level semantics: signalled
exactly when the consumer has something to observe. **The reset is atomic with
the emptiness observation; the signal may be outside any lock** (see C-1b). The
signal is *skipped* when the queue was already non-empty, or when the consumer
is not parked. Handed out as a borrowed handle plus an owned duplicate.

**R5 Wakeup safety.** No lost wakeups. Spurious wakeups are permitted, and the
consumer must tolerate them. Drain to empty on every pass.

**R6 Parking.** Optional consumer spin before parking, with the duration tunable
and **sized by the topology** -- generous when a domain owns a core exclusively,
zero when it shares one with the rest of a laptop.

**R7 Payload.** POD descriptors only, never bytes: operation, target, buffer
slot index, offset, user tag. **No allocation on push.** Carrying bytes would
mean copying out of the registered pool, defeating the reason to register.

**R8 Shutdown.** The consumer learns when all producers are gone; producers
learn when the consumer is gone and fail with a typed error. Descriptors in
flight at teardown are **accounted, not dropped** -- some own handles, and their
disposal must be allowed to block.

**R9 Observability.** Depth and high-water for tuning, plus **a count of
doorbells actually rung**. That makes R4's skip rule measurable rather than
assumed, and sabotage-verifiable: disabling the skip must change the number.

**R10 No client callbacks.** No trait and no closure on the producer or consumer
path. The HANDLE is the extension point.

**What is reused from the file-watcher, and what is not.** The *invariant* is
reused: the event is level state, signalled exactly when there is something to
observe, with the reset atomic against the emptiness decision. The
*implementation* is not: [queue.rs](../crates/windows-file-watcher/src/queue.rs)
uses `Mutex` and `Condvar`, which is right for change-notification cadence and
wrong for an I/O hot path, because it puts a lock on the producer side. Stating
this explicitly so that "reuse the queue" does not become reuse of the wrong
half.

## Resolved: CQ cardinality, and a correction about how wide waits work

**Correction, from the engineer.** An earlier turn in this session claimed
`ThreadpoolWait` "internally manages the 64-handle groups". That is wrong. Modern
thread-pool waits are backed by **kernel-side wait completion packets**
associated with the pool's completion port; there is no user-mode grouping and
no fan-out of waiting threads per 64 handles.

That improves the answer rather than complicating it: wide waits cost the
dispatch hop, not a thread per group.

**Resolution: one CQ per domain, one HANDLE each, and the client chooses how to
observe them** -- `WaitForMultipleObjects` on its own thread when the count is
within the limit and no hop is wanted, `ThreadpoolWait` when wider or when the
hop is acceptable.

This preserves shared-nothing, since there is no single queue every domain
writes to, and it pushes the trade-off to the only party that knows which side
of it it is on. It is also the payoff from rejecting the `Ring` trait: because
the extension point is a HANDLE, this strategy did not have to be anticipated.

## Resolved: how a client places its own threads

Following from "two locality consumers" above and the question of whether the
facility would end up building a thread pool. There are three thread
populations, and each has a distinct justification:

| Population | Owner | Why |
|---|---|---|
| Namespace and blocking operations | the **Windows pool** | needs quarantine and elasticity; `runs_long`; must survive a wedged network call |
| Domain I/O threads | **us**, one per domain | pinning; the Windows pool cannot affinitize |
| Client continuations | **the client** | see below |

Three options were considered for the third row: (a) continuations run on the
domain thread, Seastar-style -- best locality, but client code must never block;
(b) placed worker threads per domain, which is the thread pool the engineer
wanted to avoid; (c) the facility reports placement and the client places its
own threads.

**Chosen: (c)**, with (a) available for consumers who want it, and (b) only
against a real consumer that neither serves. The risk worth guarding is not
building (b) but building it *first*, before knowing whether (c) suffices.

### Two tiers of thread construction, because binding afterwards is not equivalent

An earlier form of this section concluded that a one-call binder was sufficient
and that a `CreateThread` wrapper would be "edging too close to the slippery
slope". **That was wrong, and the engineer corrected it**: constructing a thread
so that it has the right attributes *from the beginning* -- stack as well as
processor affinity -- is the difficult part, and it is the part a consumer
cannot easily do.

**Why binding afterwards is strictly worse.** A thread's stack is allocated at
creation time, on whatever node the *creating* thread's policy selects. Spawn
from node 0, bind to node 1, and the stack stays on node 0 permanently -- every
local, every spill, every call frame is a remote access for the life of the
thread, and no amount of later `SetThreadGroupAffinity` moves it. There is also
a window before the bind lands in which the thread runs on the wrong processor
and warms the wrong caches.

**Why this is a missing layer rather than a convenience.** Creation-time
affinity requires `CreateRemoteThreadEx` against one's own process with a
`PROC_THREAD_ATTRIBUTE_LIST` carrying `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY` and
`PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR`, assembled through
`InitializeProcThreadAttributeList` and `UpdateProcThreadAttribute` -- a two-pass
sizing call, a manually managed opaque buffer, and lifetime rules requiring the
attribute values to outlive the call. `std::thread::Builder` can set a stack
size and **nothing else**; it spawns with no attribute list. So a Rust consumer
has *no path* to a correctly constructed thread without dropping to raw Win32,
and once there must also re-supply what `std` was doing for it, notably catching
unwind at the entry so a panic does not cross an `extern "system"` boundary.

Each of those steps is simple. Collectively they are a minefield nobody crosses,
which is the [SMOP principle](../DESIGN-NOTES.md#the-value-is-existence-not-cleverness)
exactly: the value is existence, and when the correct construction is difficult,
providing the constructor *is* the feature.

**The line is ownership, not construction.** The facility helps *construct* a
thread and never *owns* one: a builder assembles the attribute list, applies the
domain's `GROUP_AFFINITY`, sets a stack reservation, wraps the entry in
`catch_unwind`, and hands back a thread **the client owns**. No handle kept,
nothing monitored, nothing restarted. The slope is ownership; construction is
not a step down it.

| | correct from birth | for threads you did not create |
|---|---|---|
| stack placement | follows the creation-time affinity | already fixed, possibly remote |
| pre-bind window | none | exists |
| API | domain thread builder | `bind_current_thread()` plus restore guard |

The binder remains, honestly labelled as the degraded path, for threads the
client did not create -- a pool thread, an existing worker. Its restore guard is
required rather than decorative: a client binding a *pool* thread must restore
it, because the thread-pool contract is that a callback restores any thread
state it changes. That also places the feature, since affinity is thread-scoped
state applied and restored, exactly the family
[windows-thread-ambient-sys](../crates/windows-thread-ambient-sys/README.md)
already handles. It either belongs there or must mirror that crate's guard
discipline rather than inventing a second pattern.

**Unverified, and worth measuring rather than assuming:** whether a thread
created with `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY` actually receives a
node-local *stack*. It is measurable -- `QueryWorkingSetEx` returns
`PSAPI_WORKING_SET_EX_BLOCK` with a `Node` field, so the address of a local in
the new thread can be asked which node its page is on -- and it is added as Q7
to [file-handle-numa-spike.rs](../crates/windows-ioring-sys/design-sessions/spikes/file-handle-numa-spike.rs).
Same hardware blocker as F-1: on a single-node machine the answer is always 0.

So option (c) is concrete: the domain's `ProcessorSet`, an answer to "which
domain is nearest me", a builder that constructs a correctly placed thread, and
a binder for threads that already exist. Every piece needed to process a CQ on
the right thread with the right affinity, and not one thread of ours.

## Working position on domain counts (not a decision)

Only the first row is measured. The rest are from published topologies and must
be treated as unverified until probed.

Read this as **the count the one architecture takes on each machine**, not as a
boundary between supported and unsupported hardware -- see the convergence above.
Every row runs the same domain shape; the rows differ only in N and in how wide
each domain's affinity set is.

| Configuration | Cores / LPs | Cache domains | Nodes | Groups | I/O domains |
|---|---|---|---|---|---|
| Snapdragon X2 Elite (measured) | 12 / 12 | 2 x L2, no L3 | 1 | 1 | 1 |
| Intel Core Ultra laptop (P+E+LPE) | ~16 / 22 | 1 x L3 | 1 | 1 | 1 |
| Cloud VM, 4-8 vCPU | 4-8 | 0-1 | 0-1 | 1 | 1 |
| Ryzen 7800X3D / 9800X3D | 8 / 16 | 1 x L3 | 1 | 1 | 1 |
| Ryzen 7950X | 16 / 32 | 2 x L3 (2 CCD) | 1 | 1 | 1-2 |
| Threadripper / 1-socket Xeon | 32-64 | 2-8 | 1-4 | 1-2 | 2-4 |
| EPYC 9004 96-core NPS1 | 96 / 192 | 12 x L3 | 1 | 3 forced | 4-12 |
| Dual-socket Xeon SPR + SNC | 64 / 128 | 2 x L3 | 4 | 2 | 2-4 |

The reasoning behind the numbers matters more than the numbers:

- **The bound is the storage device, not the CPU.** Shard-per-core is a Seastar
  *application* answer, where the shard owns application state and I/O is
  incidental. For an *I/O domain* count, throughput comes from queue depth, not
  thread count: one consumer NVMe drive is saturated by one or two submitting
  threads at moderate queue depth, and further threads add registered pools and
  device-queue contention without adding throughput.
- **The notes already point here.** "Buffer placement probably dominates thread
  placement": a buffer on a node remote from the device means every byte crosses
  the interconnect forever, where the callback's location is a one-time
  cache-warmth question. If placement relative to the *device* dominates, the
  device sets the partition.
- **Registration punishes guessing high.** Registration pins pages and is
  one-shot per ring, so N domains means N separately pinned pools: a 256 MiB
  working set is 256 MiB pinned at one domain, or 3 GiB at twelve, or twelve
  pools too small to keep a device busy.
- **The mapping needed is coarse and often absent** -- see F-1 above, which
  corrects the ioring notes' stronger claim that it is unreachable. A volume's
  node is one documented FSCTL away, but it is volume-granular, frequently has
  no answer, and is meaningless where one volume spans several devices. Device
  topology is therefore still configuration rather than detection, for a
  narrower reason than the notes currently give.

**Proposed default, not yet agreed:** one domain, adding one only when the
device it serves can be named; above 64 logical processors take the group floor
and no more.

## Working under a hardware gap: what is blocked, and what breaks

The engineer expects access to a genuine multi-node NUMA machine eventually, but
not soon. Until then this design rests on documentation rather than measurement,
which is a different grade of evidence than most decisions in this repository
and must not be allowed to blur into them.

**Sorted by dependence on an unverified NUMA claim:**

| Not blocked -- verifiable on the development machine | Blocked on NUMA hardware |
|---|---|
| the MPSC, eventcount, and doorbell (R1-R10 is pure concurrency) | whether the FSCTL names a *meaningful* volume node (F-1) |
| the two-layer ring and the client-facing API shape | Q6, whether a Storage Space reports honestly or reports a fiction |
| one-shot registration semantics (already established) | Q7, whether creation-time affinity yields a node-local stack |
| the C-1 doorbell measurement (`SetEvent` against `SubmitIoRing`) | the *magnitude* of the buffer-placement benefit |
| the composed layer's type-level traversal | domain-count tuning above one |
| the durability crate, whose mechanism was already measured as D-23/D-24 | |
| whether `CreateRemoteThreadEx` with an attribute list works at all | |

**The pattern in the blocked column is the reassuring part: none of those
threaten the structure. They threaten the justification and the tuning.** If
every one came back badly, the architecture would stand and the NUMA-specific
features would be decorative rather than wrong.

**And the first deliverable depends on none of them.** At N=1 there is no
routing, no placement choice, and the buffer goes on the only node there is. So
"build N=1 first", chosen above because it is the common case and the substrate,
is *also* the plan that needs no NUMA hardware. The whole first deliverable and
most of the second can be built before the machine exists.

### Practices to adopt while the gap lasts

1. **Mark documented-but-unwitnessed claims distinctly from measured ones.**
   This repository's decisions are unusually well measured, which creates its own
   hazard: a reader cannot tell
   [D-23](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-23) -- measured, with a
   control case -- from a claim taken off a documentation page. Anything
   load-bearing that rests on documentation must say so *in the decision*, the
   way F-1 above says "contributed by the engineer as research, not measured
   here".

2. **Quarantine each unverified claim so that a correction is surgical.** Do not
   let "creation-time affinity yields a node-local stack" become load-bearing for
   anything beyond the thread builder's justification. If Q7 returns false, the
   response should be editing one rationale, not restructuring a design.

3. **Pre-build the instruments now, while the context is fresh.** Time on a
   borrowed machine is likely to be short and should be spent measuring, not
   writing `CreateRemoteThreadEx` attribute-list code. F-1's spike is already
   written *and smoke-tested*, and that smoke run found a real defect in it --
   the `File::open` directory failure that would otherwise have surfaced on the
   borrowed machine. Every remaining question deserves the same treatment before
   the hardware appears.

### What to run when the machine is available

Ordered so that a short session yields the most:

1. **F-1 / Q1-Q5** -- [file-handle-numa-spike.rs](../crates/windows-ioring-sys/design-sessions/spikes/file-handle-numa-spike.rs),
   as-is, against an ordinary volume. Establishes whether either call names a
   meaningful node, and whether they agree.
2. **Q6** -- the same spike with a Storage Space directory as `argv[1]`, with the
   space's layout recorded alongside. Distinguishes an honest answer from a
   fiction, which is the outcome that would be worse than no answer.
3. **Q7** -- [thread-stack-numa-spike.rs](../crates/windows-ioring-sys/design-sessions/spikes/thread-stack-numa-spike.rs),
   written and smoke-tested. Decides whether the thread builder's principal
   justification holds. Three threads (attribute at creation, plain control,
   plain-then-bound) each report a shallow and a deep stack page, which also
   distinguishes creation-time placement from first-touch placement.
4. **Magnitude** -- a read benchmark with the registered pool placed local
   against remote, which is the number the entire domain-count argument rests on
   and which nothing in this session has measured.

## Open questions

- **The three structural gaps** in the coherence assessment above: where the
  client-facing ring lives, CQ cardinality, and whether the client ever sees an
  `IoRing`. These are now the session's principal open questions and they
  supersede the framing below.
- **Which end to design from.** Three candidates were put: (a) the seam between
  the pooled namespace plane and the pinned data plane, given one-shot
  registration and no shared ring storage; (b) the `RingFleet` that
  [D-8](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-8) deferred; (c) the queue
  and doorbell substrate. Not yet chosen.
- **How a newly opened handle reaches a shard at all**, given registration cannot
  be extended. Candidates not yet evaluated: pre-registered fixed tables with
  slot management, unregistered handles on the hot path, ring recycling.
- **Shard count for a general substrate**: per core, or per outermost
  partitioning cache. Related: whether an I/O domain count and an application
  shard count are the same number, which the "all three coincide" argument
  assumes.
- **Heterogeneous cores.** Intel P/E/LPE and ARM performance/efficiency clusters
  mean pinning an I/O shard to an efficiency core is a latency trap.
  `windows-topology-sys` exposes `efficiency_class` on `Core`, so the input
  exists; no policy consumes it.
- **Whether the L2 cluster structure on ARM parts is worth sharding on**, or
  whether one whole-machine domain is the right answer there anyway.

## Corrections made during the session

- **"`ByL3` yielding zero domains would create zero rings" was wrong.**
  `Policy::select` already degrades to a whole-machine domain and flags it. The
  surviving finding is about the *justification prose* in the ioring notes, not
  about the sample's behaviour.
- **"The `epoch_log` example does sector-aligned I/O" was wrong**, and was part
  of the session's opening premise. That example opens a plain file and gets
  alignment structurally from 4096-byte slots. The actual
  `FILE_FLAG_NO_BUFFERING` work lives in `tests/flush_barrier.rs` and
  `tests/handover.rs`. NUMA-affined buffers (`ring_copy`) and sector-aligned
  unbuffered I/O (those tests) are currently **two separate things in two
  separate places**, never combined.
