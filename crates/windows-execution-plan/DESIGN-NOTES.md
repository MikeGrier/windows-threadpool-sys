# Design notes: the execution-domain planner

Current canonical decisions for this component. See [COMPONENT.md](COMPONENT.md) for what the
component is; see [CHECKLIST.md](CHECKLIST.md) for what is planned.

While M1 runs, most entries here are **queries** rather than choices: the planner's requirements
on `windows-topology-sys`, stated precisely enough that the topology model can be designed against
a real caller instead of against a guess.

## Decision index

| ID | Decision |
|---|---|
| <a id="ep-d-1"></a>EP-D-1 | **The shard-set query**: what the planner must know to choose which processors host a domain, and what today's model cannot tell it. |
| <a id="ep-d-2"></a>EP-D-2 | **The proximity query**: how close two processors are, which selects the channel between their domains. Takes an **unordered** pair; the model has no answer today. |
| <a id="ep-d-3"></a>EP-D-3 | **The residency query**: where a domain's pool lives, and which side of a cross-domain pair should host a shared ring. **Ordered**, and the half the model cannot answer is structurally unanswerable rather than merely unpopulated. |

## EP-D-1: the shard-set query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.1.*

### What the planner is choosing

Which processors may host an execution domain, and how to group them so a policy can pick between
one domain per core and one per logical processor, and can decide whether efficiency cores are
peers, a second tier, or excluded.

This is the first step of the construction and it fixes the domain count, which everything
downstream is shaped by: the number of rings is quadratic in it, and each domain's memory pool is
sized against it.

### What it must know, and why

1. **Identity, as `(group, number)`.** Not a bare index. A processor number without its group names
   a different processor in every group and the wrong one in all but the first, and pinning is a
   `GROUP_AFFINITY` -- `SetThreadGroupAffinity`, not `SetThreadAffinityMask`, which cannot name
   another group at all. A planner that flattens this produces a plan that is silently wrong above
   64 processors.

2. **Whether the processor is online.** An offline slot exists and counts toward a group's maximum;
   planning a domain onto one is planning a thread that cannot run.

3. **Core membership and whether the core is SMT.** The choice between one domain per core and one
   per logical processor is the single largest policy lever, and it needs the sibling grouping, not
   just a count.

4. **Efficiency class.** On a hybrid part, putting latency-sensitive domains on efficiency cores is
   a defect the client will not see in a functional test, only in a percentile.

5. **Whether the processor is available to this process at all** -- parked by the scheduler, or
   outside the CPU-set allocation the process was given.

### What today's model answers

Points 1 through 3 cleanly. `ProcessorId` is `(group, number)` by construction and documents why
(D-7). `Processor::online` is exactly the distinction in point 2. `DomainKind::Core` carries
`simultaneous_multithreading` and the sibling set, so point 3 is a walk of `MachineMemoryTopology::cores()`.

Point 4 is answered, but **twice, in two shapes, and one of them is unsafe to use** -- see below.

Point 5 is **not answered at all**. `GetSystemCpuSetInformation` is consumed nowhere in the
workspace, so `Parked`, `Allocated` and `AllocatedToTargetProcess` are unavailable. A planner
cannot currently avoid pinning a domain to a parked processor, and the client cannot detect that it
happened. Tracked as `SH-16.10` in
[CHECKLIST-ship-topology-and-queues.md](../../CHECKLIST-ship-topology-and-queues.md).

### `Processor::capacity` must not be used for point 4

**Use `DomainKind::Core { efficiency_class, .. }`. Do not use `Processor::capacity`.**

`capacity` is computed as `online.then(|| find the owning Core domain).flatten().unwrap_or(0)`, so
the value `0` means any of three different things:

- the processor is offline;
- the processor is online but no `Core` domain names it, which the topology tolerates by design
  since firmware coverage is not guaranteed;
- the processor is online, has a core, and its efficiency class genuinely **is** `0`.

The third is not an edge case. It is **every processor on every non-hybrid machine**, so the
sentinel collides with the overwhelmingly common legitimate value.

For this planner the collision is worse than for most consumers, because Windows orders efficiency
class with `0` as the *least* performant. On a hybrid part an unknown processor is therefore
indistinguishable from an efficiency core, and a policy that excludes efficiency cores would
silently drop a processor that might be a performance core -- while a policy that tiers them would
place it in the wrong tier. Both failures are invisible in a functional test.

`Core { efficiency_class }` carries the firmware value with no sentinel, and absence is represented
by the processor being in no `Core` domain, which is a distinguishable state rather than a value.

**This is the same defect the locality-model session exists to fix, in a third place.** The others:
`ProcessorPlace::cache_domain: Option<u32>`, where `None` conflates "no level partitions this
machine" with "this processor was not named at the level that does" (`SH-16.5`); and
`MachineDescription::cpu_model`, where the same conflation was noticed and solved with a side
boolean. Recorded here so the sweep that fixes the model does not stop at the two already known.

### Partial core coverage is a real state, not a corruption

A processor in no `Core` domain is a firmware gap, not a contradiction, and the topology crate
tolerates it deliberately. The planner must therefore decide what to do with a processor it cannot
group -- it is a candidate host whose SMT relationships and class are unknown, which is exactly the
"unanswered query" case that [CHECKLIST.md](CHECKLIST.md) EP-1.4 owns. It is named here so that
item is not written as though the case were hypothetical.

### What this asks of the topology model

Nothing new in shape; three things in substance.

- Availability (parked, allocated) has to become expressible, since no policy can be correct
  without it.
- Efficiency class has to have exactly one representation, and it must distinguish "class zero"
  from "not known".
- Core membership has to admit that a processor may be in no core, without that being an error.

## EP-D-2: the proximity query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.2.*

### What the planner is choosing

For two domains, what connects them: a dedicated SPSC ring, a shared MPSC ring fanning several
producers into one consumer, or a routed hop through an intermediate domain. That choice is made
once per pair, and it is made from how close the two processors are.

This is the query the whole model question turns on. Everything else the planner asks is either
per-processor (EP-D-1) or per-memory-domain (EP-1.3); this is the only one that is *relational*,
and it is the one today's model cannot answer.

### It takes an unordered pair. The checklist item said ordered, and was wrong.

`windows-placement-probe` already settled this and stated the reasoning, which is worth quoting
because it is easy to get backwards:

> These names are deliberately symmetric, and that is not an oversight left over from before hops
> became directed. The *relationship* between two processors genuinely is symmetric -- two
> processors either are SMT siblings or are not, share a cache domain or do not -- so there is no
> honest `CrossNumaNodeForward` to name. Splitting the labels by direction would invent a
> distinction the topology does not have.
>
> The *workload* is what is asymmetric: the producer writes and the consumer reads, so swapping
> them swaps which side pays. Direction therefore lives where it is real, not in the label.

And on the measured side: "a hop is not symmetric even though the link is."

So the split is clean, and the planner needs both halves in different places:

- **Proximity is the link.** Symmetric, unordered pair, answered here.
- **Residency is the hop.** Asymmetric -- which side hosts the ring buffer, which the probe
  measures with a dedicated ring-placement column because it was found to matter. That belongs to
  EP-1.3, not here.

Putting direction in the proximity query would invent an asymmetry the topology does not have, and
would double the size of an answer that has no second half to fill.

### What the answer must contain

Not a boolean, and not a bare identifier. Three things:

1. **The tightest granularity the two share.** Comparable against other pairs' answers, because the
   policy's threshold ("SPSC within this, MPSC beyond it") is a comparison. The *identity* of the
   granularity matters less than its position.

2. **The membership of that granularity.** Selecting MPSC is not enough -- the planner must size the
   fan-in, which is "how many other domains sit at this same proximity". Without membership the
   planner would ask the proximity query O(n^2) times and reconstruct the grouping itself, which is
   the re-derivation the seam exists to prevent.

3. **Whether a finer granularity went unobserved.** This is the part that a naive design drops. If
   L3 was observed and L2 was not, "tightest shared is L3" is *not* the answer -- the answer is "at
   most L3, and finer was not looked at". A planner told the first would choose a slower channel
   than the machine can support and never learn why. Under the model's bar -- usable without further
   measurement -- it cannot go and check, so the distinction has to be in the answer.

### The query should be total, which needs a top element

Two processors in the same machine always share *something*: one address space, one scheduler, one
memory system, however far apart. If the granularity order has no top, the query returns "nothing
in common" for a cross-node pair and every caller writes the same empty-case branch.

Making "the machine" an explicit top granularity is honest -- it is a real, if loose, locality tier
-- and makes the query total. A bottom ("this processor alone") is the same argument at the other
end and makes `proximity(a, a)` answerable rather than a special case, though a planner has no
reason to ask it.

### A partial order means the answer may not be a single granularity

If the order is by observed set inclusion rather than by firmware numbering, two granularities can
be **incomparable** -- neither refines the other. The tightest shared granularity is then not
unique, and the honest answer is the set of *minimal* shared granularities, which is almost always
exactly one.

This is a cost, and it is worth naming rather than discovering later: every caller either handles a
multi-element answer or documents that it takes the first. But the alternative -- forcing a linear
order -- means silently discarding a real boundary on a machine whose levels do not nest, and this
repository has been bitten specifically by structure that was assumed rather than checked.

### What today's model answers: nothing

`MachineMemoryTopology::outermost_partitioning_cache` reports **one level for the whole machine**, and
`Slice::same_cache_domain` reduces that to a boolean at that one level. Neither is pairwise. There
is no query anywhere in `windows-topology-sys` that takes two processors.

So a planner today reconstructs proximity from the partition list -- which is exactly what
`SH-16.9` records three consumers already doing, in two mutually inconsistent ways. The absence of
this query is the cause of that defect, not a separate problem.

### What this asks of the model

- A granularity order derived from **observed set inclusion**, not firmware level numbers, so a
  measured-only tier and a machine with no L3 both have positions.
- Access to that order **as a collection**, with a pairwise helper derived from it, returning minimal
  shared granularities plus their membership. Stated here first as a pairwise query, which
  [windows-topology-sys](../windows-topology-sys/CHECKLIST.md) `M4+.1` corrected: requiring the answer
  to carry the block containing both processors makes it a question about the partition, not the pair,
  and a pairwise-primary surface would force the planner into the O(n^2) reconstruction that `SH-16.9`
  records going wrong three times. The three *requirements* below are unchanged; only the shape is.
- Unobserved granularities represented, so an answer can be an upper bound and say so.
- A top element, so the query is total.

## EP-D-3: the residency query

*Recorded by [CHECKLIST.md](CHECKLIST.md) EP-1.3.*

### What the planner is choosing

Two things, and they are different questions that happen to share a subject:

- **Where each domain's own pool lives.** A domain allocates node-locally to the processor it is
  pinned to. Per-processor, unordered, cheap.
- **Which side of a cross-domain pair hosts their shared ring.** Ordered, because the producer
  writes and the consumer reads, so the placement decides which of them pays for the crossing.

This is where the direction that [EP-D-2](#ep-d-2) deliberately refused lands. Proximity is the
link and is symmetric; residency is the hop and is not.

### The first half is answered, with one asymmetry worth keeping

`MachineMemoryTopology::memory_domains()` yields the memory domains with their processor sets, so
processor-to-domain is a lookup.

Partial coverage exists here as it does for caches -- a processor may be named by no memory domain
-- but **the right response is different, and `windows-placement-probe` already got this right**.
Its `places_from_topology` refuses on a missing NUMA node while tolerating a missing cache domain,
and the asymmetry is principled: an unknown cache domain costs an optimisation, whereas an unknown
memory domain has no honest fallback at all, since the pool has to be allocated *somewhere* and
guessing means quietly allocating remote memory for the life of the process.

So the planner inherits that: an unplaced processor may still host a domain, but not with a
node-local pool, and the difference has to be visible in the plan rather than assumed away.

### The second half is not merely unpopulated -- it cannot be measured, by construction

`MachineMemoryTopology::distances` exists, and it is easy to read its permanent `None` as an oversight. It is
not. The field is documented as being for a fed-in description, because "Windows exposes no
user-mode SLIT reader", and that is accurate.

The sharper problem is what follows from it. `distances` has exactly two input paths: hand
construction, which defaults to `Provenance::Synthetic`, and deserialization, which
`downgraded_to(Provenance::Restored)` caps. `MachineMemoryTopology::discover` hardcodes `None`. So **no path
exists by which `distances` can ever carry `Measured` provenance** -- not because nobody wrote the
code, but because the only sources are a literal and a file, and a file cannot establish that it
describes the machine you are on.

Under the model's bar -- usable without further measurement -- a planner on a real machine
therefore cannot obtain trustworthy distance for that machine today, and no amount of populating
the existing field would change that.

### A scalar distance cannot express what this query asks

Even a populated `Distances` would not answer it. The matrix is SLIT-shaped: one scalar per pair,
with `matrix[i][i]` conventionally `10`. That is a *symmetric, workload-independent* abstraction,
and the residency question is neither. It asks which of two directions is cheaper for a specific
access pattern -- a ring one side writes and the other reads.

`windows-topology-sys` D-9 already anticipated this precisely, and excluded it deliberately:

> **HMAT-style attributed relations.** ACPI's Heterogeneous Memory Attribute Table supersedes SLIT,
> giving per-initiator/per-target read and write latency and bandwidth -- four numbers where SLIT
> gives one scalar [...] A general edge list (`{ from, to, read_latency_ns, read_bandwidth_mbps,
> ... }`) would absorb HMAT, **asymmetry**, and multi-hop CXL fabrics; the scalar distance matrix
> this schema keeps will [be revisited when] scalar distance demonstrably mismodels a machine
> somebody is tuning for.

**This planner is the machine-tuner that deferral names, and asymmetry is exactly the property it
needs.** D-8 makes the revision cheap by keeping the JSON schema outside the semver contract, which
that decision says is "precisely what makes D-9's deferrals safe rather than merely convenient".

### The trigger is approached, not met, and saying which matters

D-9's condition is *demonstrable* mismodelling, and honesty requires separating what is shown from
what is expected.

What is shown: `windows-placement-probe` measures per-hop cost as four numbers per undirected edge
-- two directions times two ring placements -- and its code states that "a hop is not symmetric even
though the link is". The apparatus treats direction as real.

What is **not** shown: any measurement demonstrating that the four numbers differ. Both development
hosts report a single NUMA node, so every such run is vacuous -- the spike says so itself, printing
"VACUOUS ON THIS MACHINE" and "Apparatus works; question unanswered". So the claim "scalar distance
mismodels this machine" is currently unproven on hardware anyone here has.

The requirement is real either way, because the planner must choose a side and today has nothing to
choose with. But the *specific* claim that a scalar is insufficient needs a multi-node measurement,
and that measurement should be taken before D-9 is reopened on those grounds rather than after.

### A measured locality fact must carry what it measured

The probe's numbers are nanoseconds for one ring-handoff pattern at one message size. Promoting
them into the topology as "the distance" would bake one workload into a model other consumers share
-- and a different consumer, streaming large buffers rather than handing off small messages, would
read them as authoritative and be wrong.

So a measured relation has to name its measurement, not just its value. This is the concrete reason
per-relation provenance has to be more than a trust label: "measured" is not a sufficient
description of a number whose meaning depends on how it was obtained.

### What this asks of the model

- Processor-to-memory-domain, with the unplaced case distinguishable rather than defaulted, because
  here it has no honest default.
- A **directed** cost between memory domains, which SLIT's scalar cannot express and which D-9
  already sketched as an attributed edge list.
- Provenance rich enough to say *what* a measured number measured, so one consumer's workload does
  not become every consumer's constant.
- And, before reopening D-9 on the asymmetry argument: a multi-node measurement showing the
  directions actually differ.
