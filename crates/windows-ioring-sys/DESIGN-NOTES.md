# Design notes: windows-ioring-sys (Tier 1)

This crate does not exist yet as compiled code. This file, the checklist beside it, and the design session
it references are the design record that precedes it. Creating the Cargo skeleton is M1.1 in
[CHECKLIST.md](CHECKLIST.md).

## Intent

Windows 11 / Server 2022 added `IoRing`: a submission/completion ring for file I/O, closer in shape to
`io_uring` than to anything else Windows offers. This crate raises those primitives into memory-safe Rust
with the minimum additional CPU and memory cost, in the same spirit as the rest of this repository.

The goal is **not** to solve every consumer's I/O architecture for them. It is to provide a safe toolkit
they can build their own answer with, plus honest guidance on the patterns that actually matter -- how to
construct rings, how to give them the right affinities, and what the trade-offs are. Consumers of these
crates are trying to maximize I/O throughput; the information in "Two delivery architectures" below is
written for them, not only for this crate's maintainers.

Where a choice would impose a policy on the consumer (how many rings, how to partition them, which thread
runs a continuation), this crate exposes the mechanism and documents the trade-off rather than picking.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **IoRing lives in its own crate, not as a third backend inside `windows-overlapped-io-sys`.** Duplicate-then-decide, per the repository's PLATFORM INTEGRITY rule: the ring path is speculative, and building it beside the working IOCP path keeps that path stable. The genuinely shared surface turned out to be small (see D-2), which strengthens rather than weakens the separation. `IoBuf`/`IoBufMut` are duplicated initially; the extract-or-share decision is deferred until the ring path is proven, and tracked as M6+ rather than left implicit. |
| <a id="d-2"></a>D-2 | **IoRing is a file data plane, not a general completion backend, and the division is forced by the kernel rather than chosen by us.** The op table is fixed: `NOP`, `READ`, `WRITE`, `FLUSH`, `REGISTER_FILES`, `REGISTER_BUFFERS`, `CANCEL`. Verified by spike against `IsIoRingOpSupported` on a fully current machine (`MaxVersion` 400). There is no ioctl op, no socket op, and no directory-change op, and unlike Linux's `io_uring` -- which grew to roughly fifty opcodes including full socket support -- Windows IoRing has not grown beyond file I/O. So `windows-overlapped-io-sys` remains the crate for arbitrary I/O (any handle, any operation), and this crate covers a strict subset of one of its three families. Neither can subsume the other. |
| <a id="d-3"></a>D-3 | **There are two delivery architectures, both first-class; neither is a degraded form of the other.** See the detail section below. This supersedes an earlier framing in which the thread-pool path was "primary" and the pinned-thread path was a "fallback" for a missing capability. That framing was wrong twice over: the pinned-thread path is the high-performance architecture, and the capability fallback is only its least interesting justification. |
| <a id="d-4"></a>D-4 | **Completion allocates nothing: the token owns the buffer, and the caller supplies the type.** `push()` returns a `Token<B>` that owns the `B` it was given; the ring stores only a generation counter and an in-flight count for rundown. No slab entry, no box, no type erasure -- the caller already knows `B`, so making it say so is free. Dropping a token whose operation is still in flight `mem::forget`s the buffer: leaking is safe, use-after-free is not, and this is the same leak-and-reclaim discipline `windows-overlapped-io-sys`'s `Operation` uses with the leak as the failure mode rather than the normal path. The ergonomic, allocating variant is layered on top of this, never underneath it. |
| <a id="d-5"></a>D-5 | **The submission queue is ring state, not batch state, so buffers are owned from `Build*` and not from `Submit`.** Once `BuildIoRingReadFile` returns, the SQE is queued and there is no rewind. If a batch could be abandoned and its buffers freed, a later unrelated `submit()` would hand the kernel freed memory. A `Batch` therefore submits on drop, and holds `&mut IoRing` so that two concurrent batches do not compile -- which turns Win32's "you must serialize submission" footnote into a compiler-enforced guarantee. |
| <a id="d-6"></a>D-6 | **Capability is negotiated and cached, never assumed.** The ring version is `min(highest we understand, caps.MaxVersion)`, stored and exposed, because the spike found an OS reporting `MaxVersion = 400` while `windows-sys` 0.61.2 names only up to `IORING_VERSION_3 = 300`; hardcoding a version would cap us permanently. `IsIoRingOpSupported` is probed once per op at construction into a capability set, so per-call cost is a bit test. `QueryIoRingCapabilities` needs no ring at all, so capability inspection is free and side-effect-free. |
| <a id="d-7"></a>D-7 | **The op set will grow, and the API is shaped so that growth is additive.** The public op enum is `#[non_exhaustive]` so a consumer cannot write an exhaustive `match` that a new op would break; new ops arrive as new builder methods; `supports_raw(op_code)` answers for ops the OS has but this crate has not wrapped; and a narrow unsafe raw-SQE seam lets a consumer use such an op before we wrap it -- the same shape, and the same justification, as the `device` family's unsafe arbitrary-control-code `ioctl` in `windows-overlapped-io-sys`. Honest limit: this covers new ops that reuse existing parameter types. An op needing genuinely new structs still requires a `windows-sys` bump, and no API shape avoids that. |
| <a id="d-8"></a>D-8 | **Locality is the consumer's decision. This crate makes a ring cheap and correct, makes its affinity explicit, and documents the trade-offs -- it does not partition anything.** Baking "one ring per NUMA node" into the layer would be policy in a primitive, and would also be wrong: see "Why the NUMA node is the wrong key" below. A `RingFleet`-style abstraction may come later, once there is evidence about what sharding actually helps; it is deliberately not in the initial plan. |
| <a id="d-9"></a>D-9 | **An IoRing cannot feed an I/O completion port, and no amount of userspace bridging recovers what is lost.** The only completion hook in the entire API is `SetIoRingCompletionEvent`, which takes an event; there is no port variant, and the CQ is a userspace ring the consumer pops. More decisively: the device-to-CPU association is lost inside the kernel, before any userspace code runs. By the time a wait callback could call `PostQueuedCompletionStatus`, the packet enters the port from an already-arbitrary processor, so the port routes on where the post came from rather than where the device completed. A bridge is therefore not merely two kernel transitions for nothing -- it is structurally incapable of delivering the associativity that would motivate it. No such bridge is provided, and no example demonstrates one. |
| <a id="d-10"></a>D-10 | **Recorded as an explicitly unverified assumption: we do not believe IOCP performs NUMA-local completion dispatch either.** Its documented and relied-upon property is LIFO thread wakeup for cache warmth, which is a different thing. The indirect evidence is that the standard high-performance IOCP pattern is one port per node with threads explicitly affinitized -- which nobody would build by hand if the kernel did it for them. This is belief, not measurement: settling it needs a multi-node machine, a device whose interrupts are affinitized to a known node, and instrumentation correlating the completing node with the callback's processor. It is recorded rather than resolved because **the design consequence is the same either way** -- a consumer who needs guaranteed locality affinitizes their own threads (Model B below). No work is scheduled against this decision. |

## Two delivery architectures

This is the section written for consumers rather than for maintainers, and the reason it sits in a design
note rather than only in a commit message.

There are two coherent high-performance shapes, and they are mutually exclusive on the hot path.

**Model A -- shared queue, kernel load-balances.** A pool of threads waits; work is handed to whichever
thread the system picks. Load balancing is automatic, locality is incidental. Classic Windows IOCP is this,
and the Win32 thread pool *is* this, architecturally. In this crate, Model A is
`SetIoRingCompletionEvent` plus a `ThreadpoolWait` from `windows-threadpool-sys`: the ring signals an
event, the pool wakes a thread, the callback drains the completion queue.

**Model B -- shared-nothing execution domains.** One pinned thread per domain, owning its ring, its buffer
pool, and its shard of the application's state, with no cross-thread synchronization on the data path.
This is SPDK, Seastar, and essentially every serious `io_uring` deployment. In this crate, Model B is a
pinned thread parked directly in `SubmitIoRing(ring, wait_n, timeout, &submitted)` -- the fused
submit-and-wait *is* the event loop. No event, no wait object, no wakeup indirection, and no drain/re-arm
race, because there is nothing to re-arm.

**IoRing is shaped for Model B.** The submission queue not being thread-safe, registration being per-ring,
and there being exactly one completion event per ring are not limitations to work around; they are the API
assuming a shared-nothing consumer.

### Why the three-way tension dissolves in Model B

A ring is three things at once, and in Model A they want different granularities:

| Role | Wants |
|---|---|
| Serialization domain (submission is not thread-safe) | finest possible -- per submitting thread |
| Dispatch domain (one completion event, one waiter set) | whatever is being affinitized |
| Registration domain (registered buffers and files are per-ring) | coarsest -- registration pins pages |

Registration is the axis that punishes over-sharding, and it is easy to miss: registering one buffer pool
into sixteen rings means sixteen separate pinnings of that memory, or sixteen pools each a sixteenth the
size. There is no partition that is optimal on all three axes -- which is a further argument for D-8.

In Model B all three coincide, because one thread per domain means per-thread and per-domain are the same
partition, and the buffer pool is per-domain anyway. The tension is an artifact of trying to share
something.

So the unit is not "a NUMA node." It is an **execution domain**: one pinned thread, its ring, its
node-local registered buffer pool, and its shard of the work.

### Why the NUMA node is the wrong key

Node count is a firmware setting, not a hardware property. AMD's NPS (Nodes Per Socket) presents the same
EPYC silicon as one node or four; Intel's Sub-NUMA Clustering does the same. A design keyed on node gets a
different partition on identical hardware depending on a BIOS option no process can see. On an NPS1 EPYC,
sharding "per node" puts 64 cores in one ring and calls it NUMA-aware.

It is worse in virtualized deployments, which is where most of this code will run: the machine this was
investigated on reported **zero** `Win32_NumaNode` instances. Any strategy keyed on node must degrade to
"one ring" when the answer is unknowable, which is the common case.

A better default heuristic is the **last-level cache domain**: `GetLogicalProcessorInformationEx` with
`RelationCache` filtered to `CacheLevel == 3`. On EPYC that is the CCX/CCD boundary, which has a real
latency cliff even inside a single NPS1 node, because crossing it goes out to the IO die over Infinity
Fabric. It is meaningful on Intel and ARM too, where the NUMA node often is not, and it degrades sanely: a
VM reporting one L3 domain yields one ring, which is correct.

**Processor groups are a hard floor.** A thread's affinity is a `GROUP_AFFINITY` and a ring's waiter lives
in exactly one group, so above 64 logical processors the partition is forced whether or not it is wanted.

### Buffer placement probably dominates thread placement

For a storage workload the device DMAs directly into the registered buffer. A buffer on a node remote from
the device means **every byte crosses the interconnect, on every operation, forever**. Where the completion
callback happens to run is a one-time cache-warmth question by comparison.

So `VirtualAllocExNuma` for the pool, on the node closest to the device, registered once into that domain's
ring, is very likely the highest-leverage locality decision available -- and it is independent of
everything above about completion routing.

### What is not reachable

Mapping a **file handle to the NUMA node of the device backing it** has no clean user-mode path. It means
walking volume to disk to device instance and reading `DEVPKEY_Device_Numa_Node`, with real failure modes
(spanned volumes, Storage Spaces, network paths, VHDs) where the question may have no answer. This crate
will not offer an automatic "put this file's I/O on the right ring." It offers "bind a ring to a domain and
submit from there," and leaves the mapping to whoever knows their storage layout.

### The practical shape

Almost nobody runs pure Model B. What works is hybrid: Model B on the hot data path (pinned threads,
per-domain rings, node-local registered pools, run-to-completion continuations, cross-domain work by
explicit message passing rather than shared state), and Model A for the control plane, background, and cold
paths, where the thread pool's quiescence is worth more than locality.

Both paths are therefore first-class in this crate, which is what D-3 records.

On sizing: one domain per physical core (not per SMT sibling) maximizes isolation; one per L3 domain gives
a smaller number of domains that can still share cache-resident state cheaply -- eight rather than
sixty-four on a 64-core EPYC. Fewer domains balance load better and duplicate registered buffers less; more
isolate better. That is a workload call, and this crate does not make it.

## What the spike established

A throwaway spike (see the design session) probed a current machine directly. Findings that the design
above depends on:

- `QueryIoRingCapabilities` succeeds with no ring; `MaxVersion` 400, max SQ 65536, max CQ 131072.
- `FeatureFlags` reported `SET_COMPLETION_EVENT` present and `UM_EMULATION` absent -- a real kernel ring
  rather than user-mode emulation.
- All seven ops supported, and only those seven.
- `PopIoRingCompletion` returns `S_FALSE` on an empty queue.
- **A file handle does not need `FILE_FLAG_OVERLAPPED`.** Reads succeed on an ordinary handle, which means
  `UnassociatedEndpoint` is not the required input type and this crate need not depend on that model.
- The completion event signals correctly and auto-resets.
- Registered file handles and registered buffers both work, including a read addressing both by index.
- A batch of eight reads submitted in one call reports `submittedEntries = 8` with all `UserData`
  preserved.
- Overflowing a 64-entry submission queue fails at entry 64 with `0x80460002` -- clean build-time
  backpressure, which is what D-5's design leans on.
- Cancelling a target that is not outstanding succeeds at build time and reports `0x80070490`
  (`ERROR_NOT_FOUND`) in the completion, not at build time.
