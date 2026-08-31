# Design notes: windows-topology-sys (Tier 1)

This crate does not exist yet as compiled code. This file, the checklist beside it, and the design session
it references are the design record that precedes it. Creating the Cargo skeleton is M1.1 in
[CHECKLIST.md](CHECKLIST.md).

## Intent

Safe enumeration of the running system's processor, cache, and memory topology, plus a
JSON-serializable description of it that can be persisted, hand-written, or fed in from another machine.

It exists to serve [windows-ioring-sys](../windows-ioring-sys/DESIGN-NOTES.md)'s locality story without
that crate having to own a partitioning policy (its D-8), but it is not specific to it: the description is
the input to a policy, and the policy is somebody else's code.

Same philosophy as the rest of this repository. Raise a Win32 primitive into memory-safe Rust at minimum
additional CPU and memory cost; do not solve the consumer's architecture for them.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **This crate exists because the `windows` crate does not provide memory safety here, only typed FFI.** Verified rather than assumed: `windows` 0.61.1 exposes `pub unsafe fn GetLogicalProcessorInformationEx(relationshiptype, buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>, returnedlength: *mut u32) -> windows_core::Result<()>`. Every real hazard stays with the caller, and they are worse than usual: two-call sizing against `ERROR_INSUFFICIENT_BUFFER`; a **walk-by-`Size`** over variable-length records, where treating the buffer as a `&[T]` silently misparses; **trailing arrays that lie in the type system** (`PROCESSOR_RELATIONSHIP.GroupMask` is declared `[GROUP_AFFINITY; 1]` but is actually `GroupCount` long, and `NUMA_NODE_RELATIONSHIP` / `CACHE_RELATIONSHIP` do the same through a union); and an undiscriminated union keyed by `Relationship`. Reading past element 0 of that array is exactly what the API requires and exactly what Rust calls undefined behavior. |
| <a id="d-2"></a>D-2 | **Shape: safe enumeration, not a topology renderer.** Enumeration is durable -- `GetLogicalProcessorInformationEx` gains relations rather than changing shape -- while *interpretation* (what constitutes a "domain" worth partitioning by) is exactly what CXL and further chiplet federation will churn. The durable thing belongs in the crate and the volatile thing above it. A consumer wanting an opinionated model builds it from these records; this crate does not ship one. |
| <a id="d-3"></a>D-3 | **`ProcessorSet` is the one abstraction added above faithful records, because it is where the bugs actually are.** Handing back raw `usize` masks would be safe FFI rather than a safe API: callers assume a single processor group, assume 64 or fewer processors, and lose the group when they flatten to an index. `ProcessorSet` carries `(group, mask)` correctly and spans groups. Everything else stays a faithful mirror of what Win32 reported. |
| <a id="d-4"></a>D-4 | **Domains are open-kinded rather than a fixed set of relation types.** Forced by the Linux cross-check (see the design session): Linux models `die`, `cluster`, and on s390x `book` and `drawer`, none of which Windows reports and none of which cache domains reliably approximate -- Zen 2 had two L3 domains per die, so cache and die genuinely differ. Enumerating every level any architecture will ever have is a losing game, so a domain is `{ kind, id, processors, ...kind-specific attributes }` with well-known kinds documented rather than closed. The cost is honest: a policy filters `kind == "cache" && level == 3` instead of reading a typed `topology.caches`, which is weaker typing at the schema boundary in exchange for surviving hardware nobody has shipped yet. Rust-side typed accessors over the open representation keep the ergonomic loss confined to the JSON. |
| <a id="d-5"></a>D-5 | **A NUMA node is modelled as a memory domain that may contain no processors, not as a processor grouping that happens to have memory.** Linux exposes `has_cpu` / `has_memory` per node precisely because memory-only nodes are now ordinary: CXL memory expanders, persistent memory in system-RAM mode, HBM tiers, and GPU-attached memory on coherent parts all appear that way. An earlier draft of this schema defined a node by its processor list, which makes a CXL expander a degenerate case rather than a first-class one -- backwards for the hardware direction this repository expects. This is the correction the Linux comparison was most valuable for, because Windows' own model is currently impoverished here and looking only at Windows would have missed it. |
| <a id="d-6"></a>D-6 | **Domains reference processors; they do not nest.** No hierarchy is imposed, so the schema never asserts that packages contain nodes or that nodes contain cache domains. Chiplets and CXL already violate those assumptions, and Linux's own levels do not form a strict hierarchy either -- clusters cut across cache domains on some ARM parts. Refusing to impose one is what lets a synthetic description describe a machine whose shape we have not seen. |
| <a id="d-7"></a>D-7 | **A processor is identified by `(group, number)`, never by a flat index.** The Windows processor group is the hard affinity boundary -- a thread's affinity is a `GROUP_AFFINITY` and cannot span groups -- so flattening destroys the one constraint a planner must respect. A description sourced from a system without groups simply has one group, which is lossless in that direction. |
| <a id="d-8"></a>D-8 | **The JSON schema is explicitly not covered by this crate's semver contract**, following the precedent set by `windows-file-watcher`'s D-71 for its scenario schema. This is load-bearing rather than incidental: it is precisely what makes D-9's deferrals safe rather than merely convenient. A schema v2 when HMAT-class hardware matters is permitted, so the cost of *not* pre-building for it is bounded. The Rust API is covered by semver as always. |
| <a id="d-9"></a>D-9 | **Deliberately excluded, with reasons.** See the detail section below. **No work is scheduled against any of it** -- the absence of checklist items is intentional, not an oversight. |
| <a id="d-10"></a>D-10 | **The description is platform-neutral; platform constraints live in the planner.** A description sourced from Linux will have one group possibly containing more than 64 processors, which is unrepresentable as a Windows affinity mask. The schema does not enforce the Windows limit. A Windows planner consuming such a description must reject or split it rather than silently emitting an affinity mask that cannot exist. Keeping the constraint in the planner is what allows a description of a machine to be written on, and for, a different platform. |
| <a id="d-11"></a>D-11 | **A `Memory` domain's `memory_bytes` is `Option<u64>`, not a bare `u64`, because Windows's own enumeration cannot report it.** `GetLogicalProcessorInformationEx`'s NUMA-node relationship carries a processor set and a node number, never a capacity; measuring node memory would mean a different API entirely. A `Topology` this crate discovers therefore always sets `memory_bytes: None` for every memory domain it produces from `RelationNumaNode`/`RelationNumaNodeEx`. Using `Some(0)` as a stand-in would be indistinguishable from "this node genuinely has no memory," which is exactly the CXL-expander case D-5 exists to represent honestly; `None` is the only choice that does not silently invent data. A hand-written or fed-in description may still supply a real value. |
| <a id="d-12"></a>D-12 | **A topology carries its own provenance, and the untrusted value is the default.** This crate deliberately lets a topology be discovered, built by hand, or deserialized from a description written for a machine you do not have -- and until now the three were indistinguishable once built. [`Provenance`] is `Synthetic` by `Default`, so forgetting is safe and claiming is deliberate; only `discover` yields `Measured`; and deserialization can only ever *downgrade*, so a file cannot assert it is the machine you are on. |

## D-12: provenance, and why the default points at distrust

Three ways to obtain a `Topology` are supported on purpose, and the crate's own front page advertises
the third: "deserialize one from JSON written for a machine you do not have". That is a feature -- it
is how a consumer tests against hardware it lacks, and this workspace needs it right now, because
`probe-core-affinity` must exercise NUMA selection logic on hosts that have exactly one NUMA node.

The hazard is that **the resulting value looked exactly like a discovered one**. There is a passing
test in this crate that parses a *Linux-shaped* description, complete with an ACPI SLIT-style distance
matrix, on a Windows-only crate. Nothing downstream could tell that apart from the machine it was
running on.

Three decisions make the marker hard to lose.

**`Synthetic` is `Default`.** This is the load-bearing one. `Topology::default()`,
`..Default::default()`, and every construction that simply does not think about provenance come out
tainted. A caller must do work to claim data is real, rather than work to admit it is not. The reverse
default would mean every forgetful construction silently asserts it read the machine -- which is
precisely the accident this exists to catch, and it would be catastrophically quiet.

**The variants are ordered by trust**, `Synthetic < Restored < Measured`, so the derived `Ord` *is* the
trust order and `min` implements "never upgrade". `downgraded_to` is that one line, which is why there
is no second, subtly different rule anywhere: a ceiling is a maximum, not an assignment, so passing a
synthetic description through a loader does not launder it into a restored one.

**Deserialization refuses any claim above `Restored`.** A hand-edited `"provenance": "measured"` is
ignored. This is the one place forgery rather than accident is refused, and the asymmetry is
deliberate: a line of code claiming `Measured` had to be written by someone who meant it, whereas a
JSON file is data that travels, gets copied between machines, and is edited by people who never read
this note. The marker is still *serialized*, so it is visible in the persisted form -- the goal is that
a tainted topology is loud, not that it is unwritable.

The consequence to be aware of: **a measured topology cannot be archived and reloaded as measured.**
That is intended. What you reload is a description of a machine, and the fact that it was once read
from a real one does not make it a statement about the host doing the reading.

**The threat model is accident, not forgery.** A caller who writes `provenance: Provenance::Measured`
over data they fabricated has lied deliberately, and no type in a crate with public fields prevents
that. Adding a private field with a constructor was considered and rejected: it would break the
hand-construction the crate deliberately supports (D-8's "plain data" property), for a guarantee that
only holds against an adversary this crate does not have.

`from_relations` stamps `Synthetic` and `discover` overwrites it with `Measured`, rather than the
transform claiming it. `from_relations` is a pure function of whatever relations it is handed and
cannot know where they came from; putting the claim in `discover` keeps it attached to the act of
asking the operating system, so a future second caller of the transform does not silently inherit an
assertion it has not earned.

## What was deliberately excluded (D-9)

Recorded because what a design declines is as important as what it adopts, and because each of these was
considered and rejected rather than overlooked. Each entry states what would justify revisiting it.

**HMAT-style attributed relations.** ACPI's Heterogeneous Memory Attribute Table supersedes SLIT, giving
per-initiator/per-target read and write latency and bandwidth -- four numbers where SLIT gives one scalar --
and Linux already exposes it. A general edge list (`{ from, to, read_latency_ns, read_bandwidth_mbps, ... }`)
would absorb HMAT, asymmetry, and multi-hop CXL fabrics; the scalar distance matrix this schema keeps will
not. That was raised as an argument for building the edge list now, on the grounds that retrofitting it is
a breaking schema change. **Deferred anyway, and the deferral is safe because of D-8:** the schema carries
no stability promise, so a v2 is permitted. The trade taken is a simpler, hand-writable description now
against a schema break later. *Revisit when:* tiered-memory hardware is in scope for a consumer, or a
scalar distance demonstrably mismodels a machine somebody is tuning for.

**Devices as topology participants and as initiators.** HMAT models initiators separately from targets
because GPUs, DMA-capable NICs, and NVMe controllers all initiate memory access, and for an I/O-focused
consumer the device is the locality question. Naming devices in the description would let a synthetic
topology drive ring planning against a device layout Windows cannot easily enumerate anyway (the
handle-to-device-node walk goes through SetupAPI/CfgMgr with real failure modes on spanned volumes,
Storage Spaces, network paths, and VHDs). **Excluded as scope, by the engineer's direction:** it changes
the crate's identity from processor topology to system topology, which is a materially larger surface than
the name implies and a larger promise than is wanted now. *Revisit when:* a consumer needs device-aligned
planning badly enough to accept that surface, at which point the crate probably wants a different name.

**Queue and interrupt affinity.** NVMe queue-pair to CPU mapping is the mechanism by which
submission-core and completion-core locality actually happens, and it is what would let a plan align rings
with hardware queues. **Excluded:** it is downstream of devices being representable at all, so it cannot
precede the previous entry.

**Power and thermal domains, cache partitioning (Intel RDT/CAT, ARM MPAM), and
confidential-computing memory-encryption domains.** All are real partitioning concepts that could map onto
ring assignment. **Excluded:** none of them is locality, each needs its own vocabulary, and D-4's open
domain kinds mean adding any of them later is additive rather than breaking -- so there is no cost to
waiting and no benefit to guessing at their shape now.

**Memory tiering abstract distance.** Linux's tiering model assigns nodes an abstract distance for
promotion and demotion decisions. **Excluded** for the same reason as HMAT, and it would arrive with it.

## What the Linux comparison established

The cross-check was run to find future expansion directions rather than to validate the schema, and it did
both. Details are in the design session; the summary is that three things in the then-current draft were
genuinely violated -- memory-only nodes (D-5), fixed domain kinds (D-4), and missing online/offline state
-- while three decisions held up unchanged: processor identity as `(group, number)` (D-7),
reference-don't-nest (D-6), and treating distances as optional, which Linux vindicated by actually having
SLIT where Windows does not.
