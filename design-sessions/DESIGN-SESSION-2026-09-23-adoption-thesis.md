# Design session -- the adoption thesis behind the ring, queue and locality work (2026-09-23)

**Summary.** The engineer stated the strategic intent behind the whole ring / queue /
topology / durability line of work: that making queues and rings reachable, and having
locality benefits arrive adaptively out of that structure, could start a virtuous cycle
ending with consumer hardware exposing what only server hardware exposes today. The
session produced the Tier 1 section
[The adoption thesis](../DESIGN-NOTES.md#the-adoption-thesis), the Tier 2 entry
[Why no option is foreclosed while the hardware gap lasts](../DESIGN-RATIONALE.md#why-no-option-is-foreclosed),
and milestone `M27` in
[CHECKLIST.md](../crates/windows-ioring-sys/CHECKLIST.md). It also supplies the reason
behind the **OPTION INTEGRITY** rule recorded in
[copilot-instructions.md](../.github/copilot-instructions.md) one commit earlier.

## Why it was stated now

It followed directly from a correction. The prior commit had concluded that a commit
strategy's justification was "dead on structural grounds" on the strength of a harness
that could not have shown the strategy working. The engineer's response was to reject the
move in general rather than only in that instance:

> Unless we can determine that the option has no possible value, we should expose it and
> provide the tools for clients to be able to choose it when applicable and to gather the
> appropriate data to make the wisest choice possible.

That produced OPTION INTEGRITY as a rule. This session supplies what the rule was missing:
the reason such foreclosures are *specifically* costly in this repository, which is not a
general preference for breadth but a consequence of what this work is for and what
hardware it is being built on.

## The engineer's framing, recorded

### The hardware trend

Non-uniform memory has been relegated to very expensive machines. As Moore's law has
plateaued, the need to introduce wider multiprocessors in contexts closer to consumers has
risen: you can only go so far with a uniform memory architecture. Even the consumer AMD
memory architectures are non-uniform with a uniform facade in front of them, and arguably
Intel also.

### Why NUMA is not a consumer-level concept

One of the main reasons is that it is difficult to program for, and the benefits are
difficult to measure in comparison to how you would have to program your system otherwise.
Today you have to make a significant architectural decision at the beginning of your
system architecture: do I want to take advantage of NUMA or not? Who would make such a
choice unless targeting larger datacenter-class hardware?

The framing worth preserving here is that the **decision point**, not the difficulty, is
the principal barrier. It is a commitment demanded at the moment the least is known, and
its payoff is invisible to anyone not already buying datacenter hardware.

### The parallel problem with queues

The programming techniques of `IoRing` and queues in general are of general purpose
utility but are not available as readily as one might wish. There are a lot of building
blocks, but except in certain small domains they are not easily grasped for how to
structure your system from the beginning.

This is the same shape as
[The value is existence, not cleverness](../DESIGN-NOTES.md#the-value-is-existence-not-cleverness),
which was already recorded: the correct construction is not within reach, so people reach
for what is.

### The thesis

There is a potential virtuous-cycle synergy to launch. Enabling applications to more
easily tap into the benefits of rings, queues and the Windows `IoRing`, and then to have
adaptive NUMA benefits without having to write specialized code to receive those benefits,
would:

- make "normal" application writers attracted to the techniques;
- yield benefits to I/O-bound application writers who would benefit from the gains of
  `IoRing` and the epoch-based durability idiom;
- provide a useful substrate for high-end application designers who actually do want to
  target NUMA systems;
- and then, if there is adoption, give reason for systems designers to expose more system
  NUMA features at the consumer hardware level, so that lower-capability hardware can
  receive the same kinds of benefits.

The operative phrase, and the one with consequences for the code, is **"without having to
write specialized code to receive those benefits"**. That is a claim about adaptivity, and
it is not what the tree does today.

### The honest expectation

Today the engineer expects low benefits to all but server-class hardware, to be fair. And
even though this work is being developed on a machine that is logically server-class
hardware, it is nonetheless just a slice, which does not exhibit the non-uniform memory
characteristics.

Thus the reason to avoid all early foreclosures of techniques which may yield benefits to
application authors: we are not the application authors, so we do not actually know what
they may want to do; and we do not have the hardware to make significant analyses of
performance tradeoffs on real NUMA hardware.

## What this changes, and what it deliberately does not

**Unchanged.** The hardware-gap analysis in
[DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md)
already worked out what is blocked on real NUMA hardware and what is not, and concluded
that the blocked column "threatens the justification and the tuning", not the structure.
Nothing in this session revises that; the thesis explains why the justification is worth
keeping open rather than settling locally, which is a different question.

**Unchanged.** [D-8](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-8) leaves partitioning
policy with the consumer. The thesis does not overturn it, and `M27` is explicitly written
so that "the library decides for you" is one candidate answer rather than the presumed
one. A thesis about making a benefit reachable is not automatically a thesis about making
it automatic, and conflating the two would be the crate taking a workload decision it has
repeatedly refused to take.

**Changed.** The gap between "benefits arrive adaptively out of the structure" and a tree
where partitioning is an explicit `Policy` a consumer selects is now a queued question
rather than an unstated aspiration. Per the repository's rule that design notes are not a
work queue, an intent that implies work on existing code has to become a checklist item or
say explicitly that it schedules none. This one implies work, so it is `M27`.

## The distinction that keeps the thesis honest

A thesis is not evidence, and this one is unusually exposed to being mistaken for
evidence, because the repository's other claims are unusually well measured. Two guards
apply, both already established here:

- The 2026-08-30 session's practice of **marking documented-but-unwitnessed claims
  distinctly from measured ones** applies to the thesis in full. The Tier 1 section says
  "thesis, not a measurement" in its first sentence for that reason.
- The repository rule to **present what was observed and not write the conclusion** means
  the thesis must not leak into places that report measurements. `cache_domains.rs` prints
  `L1: 8`, `L2: 8`, `L3: 1` and marks which the heuristic chose; it does not tell the
  reader what that implies for their design. That separation is the thesis working
  correctly, not a limitation of the sample.

The measured observations the thesis leans on are genuinely measured, and are cited rather
than restated: the ARM laptop reporting no L3 and zero nodes
([D-48](../crates/windows-ioring-sys/DESIGN-NOTES.md#d-48)), and this host's L3 spanning
all 16 processors over a real 8-way L2 partition. What is *not* measured is every forward
step of the cycle, and the Tier 1 section says so.

## Later the same day: the mechanism has a name, and it already existed

The thesis above was recorded without naming the component that delivers it, and the gap was queued
as an adaptivity question inside `windows-ioring-sys`. The engineer's correction:

> This is why it's the "topology planner". The idea is to have the developer give a sufficiently
> abstract definition of the application's input, output, and processing code paths and then the
> topology planner would be able to infer both the general connectivity / directed flow of data
> needed to realize the graph and then when given a physical machine model would respond with one or
> more suggested realizations of the graph in terms of specific execution threads pinned on which
> processor groups, numbers of queues of which types, etc.

[topology-planner](../crates/topology-planner/COMPONENT.md) had been planned since 2026-09-03 and
its input was the one part left open: `EP-D-4` recorded the goal as an input whose shape was
"deferred for litigation". That deferral is what this statement discharges, and it is recorded as
[EP-D-6](../crates/topology-planner/DESIGN-NOTES.md#ep-d-6).

**Three things arrived at once, and only the first was the deferred question.**

1. **The input is a dataflow description** -- the application's input, output, and processing code
   paths. Not a topology preference and not a set of tuning hints.
2. **Planning is two stages.** Connectivity and directed flow are inferred with no machine in hand;
   the machine enters only at the second stage. This was not asked for separately and follows from
   the first: a derivation that needs no machine should not be entangled with one.
3. **The answer is plural** -- one or more *suggested* realizations, which the developer chooses
   between.

**The third is the one worth guarding.** A single returned plan is easier to consume, test and
document, and those pressures will argue for collapsing the set at some later convenient moment. The
reason not to is structural: ranking candidates requires knowing what the developer values, and this
component was handed a description of an application rather than a statement of preference. A
planner that returns one arrangement has either acquired a preference it was not given or hidden a
choice it was not entitled to make.

### What this corrected about the morning's work

**`M27` was in the wrong crate.** It had been written that morning, asking whether
`windows-ioring-sys` should derive a partition for a consumer who expresses no preference. Answering
that there would have produced a second policy surface beside the planner's -- which is precisely the
`outermost_partitioning_cache` defect the planner exists to avoid, a policy answer landing in a crate
whose job is something else. `M27` was re-planned the same day into what the ring crate genuinely
owes: being **realizable from** a plan it did not choose. The checklist rules require saying that
plainly rather than silently rewriting the milestone, which is why both the milestone and the two
PLANS rows describing it now carry the correction.

**The thesis section was incomplete rather than wrong.** It named the barrier -- an architectural
commitment demanded when the least is known -- and did not name what removes it. The mechanism
section added afterwards is the answer, and the three properties it lists are each doing work: the
developer never makes a topology decision, stage 1 is stable across every machine the application
will run on, and the plural answer is OPTION INTEGRITY at component scale.

**And the deferral held up, which is worth recording.** `EP-D-4`'s goal input had been undefined for
twenty days across four documents, and nothing was built on a guess in the meantime, because the
deferral was *named* at every site that mentioned it rather than being an absence. Correcting it was
a sweep of four sites, all of which were found by grepping the phrase the deferral was recorded
under. An unnamed omission would have left nothing to grep for.