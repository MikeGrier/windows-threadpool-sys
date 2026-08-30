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

## Working position on domain counts (not a decision)

Only the first row is measured. The rest are from published topologies and must
be treated as unverified until probed.

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

## Open questions

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
