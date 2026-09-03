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
`simultaneous_multithreading` and the sibling set, so point 3 is a walk of `Topology::cores()`.

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
