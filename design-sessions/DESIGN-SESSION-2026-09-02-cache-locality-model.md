# Design session: the cache-locality model

**Status: OPEN, with direction settled and the representation converging.** The engineer has
taken sides on both underlying questions (see "Direction taken" below) and settled three
sub-questions about the proposed shape: provenance is **per-relation**, "determined absent"
is a **distinct record**, and the whole-object `Provenance` is **superseded** rather than
re-derived. Nothing is carried over from the old model for its own sake; two of its
properties are kept only because they re-derive independently. The
options section further down predates that direction and is kept as a record of what was
considered -- Options 1 and 2 are now insufficient on their own, because both preserve the
`Option`-shaped absence the direction rejects.

Prompted during PR #56's tenth review round while fixing
[SH-16.5](../CHECKLIST-ship-topology-and-queues.md). That item is **blocked on this
session** and must not be implemented before it concludes: the primitive SH-16.5 was about
to add is itself the thing under design.

A working prototype of the SH-16.5 fix was written and then reverted so as not to prejudge
the outcome. It is preserved outside the repository, in this session's agent workspace, as
`sh-16.5-prototype.patch`. It compiled, and its topology-side tests passed and were
sabotage-verified; it is evidence about one option, not a commitment to it.

## How the question arose

SH-16.5 reported a contract contradiction: `windows-topology-sys` documents that a
partitioning cache level is *not* required to cover every online processor, while
`windows-placement-probe` treats any uncovered processor as corruption and fails the whole
run. The agreed direction was to state the rule once, in the crate that owns the topology,
and have the consumer ask.

Mid-implementation the engineer raised a broader objection, which is the actual subject
here:

> i think the problem is that the model has to acknowledge more than exactly 3 levels of
> caching. why not 1? 5? where are the write buffers modeled? i am not saying we have to
> start over from scratch, but it is somewhat ironic that in a crate named "topology", we
> have a network where we assume 3 members and only having 2 confuses it

## Direction taken

The engineer separated the subject into two questions and answered both, then gave a
constraint on the representation. Recorded in their terms:

**1. What model does the Windows API set represent?** Do we expose all the topological
nuance that a real system, described through that API, would reveal -- "not every mechanical
combination of representable values: a reasoned set of logical derived models that would be
represented through the Win32 API set". So the target is completeness with respect to what
Win32 can express about a real machine, not an arbitrary product of enum values.

**2. What memory-hierarchy concepts might a system we encounter have, whether or not Win32
exposes them?** These affect analysis regardless of API exposure, and the `-probe` crates
exist partly to establish them by measurement.

**3. The representation constraint, which is the operative decision.** Where a level of
hierarchy may or may not be present, do **not** model it as a second-class `Option`, because
that conflates two different facts: *Win32 did not provide data* and *the level was
specifically found not to be present*. Instead choose a representation that is designed for
the topology to **represent the observed connectivity**.

This supersedes the framing further down that treated "collapse to one boundary" as the whole
problem. The collapse is a symptom; the cause is that presence and observation are not
modeled.

### What this rules in and out

- Out: `Option<u32>`, and equally `Option` plus a side boolean, as the way to say a level is
  missing. Also out: the `CachePlacement` prototype from SH-16.5, whose `Unknown` arm merges
  "not reported" with "reported, does not name this processor".
- In: a representation where a sharing relation that was *observed not to exist* and one that
  was *never observed* are different values, and where a measured relation can sit beside a
  firmware-reported one.
- Still open: the concrete shape. See "Proposed representation" below.

## New evidence gathered after the direction was set

**The principle is already in-house, written down, and applied inconsistently.**
`MachineDescription::cpu_model` in `windows-placement-probe` records:

> Suppression is recorded in `model_suppressed` rather than left to be inferred from absence:
> a field withheld by the runner and a field the host would not answer are different facts,
> and a collector that cannot tell them apart will eventually read one as the other.

That is the engineer's point exactly, reached independently for a different field. It is
solved there with `Option` plus a side boolean, which is the weaker form the direction above
rules out -- but the *reasoning* is settled precedent in this repository, not a new claim.

**Win32 is not fully consumed: CPU Sets is entirely absent.** The crate consumes seven
`GetLogicalProcessorInformationEx` relations (`ProcessorCore`, `ProcessorPackage`,
`ProcessorDie`, `ProcessorModule`, `Cache`, `NumaNode`, `Group`), which is essentially all of
that API. But `GetSystemCpuSetInformation` / `SYSTEM_CPU_SET_INFORMATION` is not referenced
anywhere in the workspace, and it is a **second, parallel topology model** Windows offers,
carrying at least: `LastLevelCacheIndex` (Windows's own LLC grouping, which is a *different*
answer from "outermost partitioning cache"), `SchedulingClass`, `AllocationTag`,
`EfficiencyClass`, and per-CPU `Parked` / `Allocated` / `RealTime` state. Verify the exact
field list against the SDK before relying on it. This is directly responsive to question 1:
the answer today is **no**, there is a whole Win32 model unexposed.

**Nothing currently infers a hierarchy level from measurement.** The engineer suspected the
`-probe` crates might already do this and flagged uncertainty. Checked: they do not. The
probes measure *cost per firmware-reported placement* (`core_affinity` times handoffs between
pairs already classified from the topology), and the NUMA spikes infer *policy* -- first-touch
versus creator affinity, per-volume versus per-file -- not structure. `Provenance` already has
a `Measured` variant, but it qualifies a whole `Topology`, not an individual relation. So the
capability question 2 describes is **new work, not a retrofit**, and the per-relation
provenance it needs does not exist yet.

**The "outermost partitioning cache" rule is stated three times, and two of the three
disagree.** `Topology::outermost_partitioning_cache` requires more than one partition **and**
pairwise disjointness. `Observation::outermost_partitioning_cache` in
`windows-platform-probes` is `caches.iter().filter(|c| c.domains > 1).max_by_key(|c| c.level)`
-- no disjointness check -- over a `CacheLevel` summary it builds itself, even though that
crate does depend on `windows-topology-sys`. On a hand-built or deserialized topology with
overlapping domains the two answer differently. `windows-placement-probe` restates it a third
time by rebuilding the map from the partition list, which is SH-16.5. Tracked separately as
SH-16.9.

## Proposed representation, for reaction

Offered as a starting shape, not a conclusion.

Stop modeling a *ladder of levels with optional rungs* and model the *observed sharing
relations* directly. A topology becomes a set of relations, each carrying:

- **what is shared** -- cache at level N, module, die, package, memory domain;
- **which processors share it** -- the `ProcessorSet` already used;
- **how it was established** -- reported by a named Win32 source, measured by a named probe,
  or determined absent.

Presence then stops being an `Option`. A machine with no L3 has *no L3 relation in the set*,
and that is an observation rather than a missing value; a machine whose firmware was not
queried for L3 carries a *not-observed* record for it. The two are different members, not the
same `None`.

Connectivity queries follow from it: "at which relations do A and B share?" returns the
observed set, and the difference between *they share nothing* (an empty answer over complete
observations) and *we do not know* (incomplete observations) is representable rather than
collapsed. `outermost_partitioning_cache` survives as one named projection over that set, for
the scheduler question of "give me exactly one boundary to shard on", and is documented as a
projection rather than as the model.

### Sub-questions, answered

**Provenance is per-relation.** Asked for a case where it could differ per *source*; there
is none worth having. Any per-source fact is expressible per-relation by repetition, and the
reverse is not true -- so per-relation strictly subsumes it. The case that decides it runs the
other way: **two sources describing the same relation**. Win32 reporting that A and B share
L3 while a probe measures otherwise is expressible only if one relation can hold both
observations; per-source would force two whole topologies and a diff.

What the per-source instinct was actually reaching for is not provenance but **completeness
of an observation attempt** -- "source S was queried about dies and said nothing" cannot
attach to a relation, because there is no relation. That is the absence record, settled
below.

**"Determined absent" is a distinct record**, not a relation with an empty processor set. An
empty set already means something else here (`memory_domains` deliberately keeps a
processor-less memory domain, D-5), so overloading it would be a trap.

**The whole-object `Provenance` is superseded, and should not be replaced by another
whole-object scalar.** Derivation: with trust per-relation, an object-level scalar can only be
the minimum (a topology with ninety-nine measured relations and one synthetic reads
`SYNTHETIC`, which is useless) or the maximum (which is dishonest). Trust belongs to an
**answer** -- "A and B share L3, established by these observations" carries its own -- and that
falls directly out of modeling observed connectivity, since a connectivity model exists to
answer queries and the query result is the thing needing a label.

### Two questions those answers open

**Is provenance a scalar or a chain?** Today it is a scalar, and deserialization is a *lossy*
downgrade: `downgraded_to` is `min` against a `Restored` ceiling. A measured relation that
round-trips through a file loses "originally measured, on this machine, at this time", which
matters more per-relation because a measured relation is expensive to establish. From base
principles these are two things one scalar was forced to conflate: **trust assertable now**
(never upgradeable) and **origin history** (recorded, conferring no trust).

**Can one relation hold more than one observation?** It probably must, and the project already
reasons this way. From `file-handle-numa-spike.rs`:

> **Agreement is consistent with volume locality; it does not establish it.** A genuinely
> per-file answer may equal its volume's node ... so one file agreeing rules nothing out. Only
> disagreement is decisive, because a per-volume answer cannot differ from itself.

That is an asymmetric adjudication rule over two independent observations of one underlying
fact, and it only works if the observations coexist. A model storing one winning value per
relation cannot express it -- and detecting a hypervisor that misreports topology is exactly
this shape.

### Two principles that re-derive rather than being inherited

The direction is explicitly to find the right model rather than carry anything over. Two
properties of the old `Provenance` are principles rather than model, and both survive that
test on their own merits:

- **The default is the untrusted value.** Under per-relation provenance this argument is
  *stronger*, not weaker: there are far more places to forget.
- **Trust never upgrades.** Identical derivation -- a file still cannot establish that it
  describes the machine you are on.

### Still open

- Scalar-versus-chain, and if a chain, what a consumer asks it.
- Whether multiple observations per relation are held as a set, or reduced on insert with the
  reduction recorded.
- What a query returns when observations disagree: a value plus a conflict marker, or the
  conflict itself, forcing the caller to adjudicate.

## What the code actually does, verified rather than assumed

The objection is half right, and the halves point at different files.

**The base model hardcodes no level count, and that was deliberate.** `DomainKind::Cache`
carries `level: u8`; `Topology::cache_levels()` returns whatever the firmware reported,
sorted and deduplicated; `caches_at_level` takes any `u8`. There is already a regression
test, `a_partitioning_cache_above_level_four_is_found`, whose comment reads: "`level` is a
`u8`. A consumer sweeping a hard-coded `1..=4` reports this machine as having no
partitioning cache at all." One level works too, and is a distinct answer
(`outermost_partitioning_cache` returns `None`, meaning "nothing divides this machine",
which the docs are explicit is a real answer and not a failure).

**The base model is also richer than caches.** `DomainKind` has seven variants -- `Group`,
`Package`, `Die`, `Module`, `Core`, `Cache`, `Memory` -- and `Die` and `Module` are
genuinely populated, from `Record::ProcessorDie` and `Record::ProcessorModule`. So the
crate models a multi-tier locality graph, not a three-level cache.

So the crate named "topology" does model a network. The collapse is downstream of it.

## Where the collapse actually lives

Three sites, in increasing severity:

1. **`Topology::outermost_partitioning_cache()`** -- selects exactly one level (outermost
   first, requiring more than one pairwise-disjoint domain) and discards every other level.
   This one sits *inside* the topology crate, which is where the engineer's irony lands
   squarely: the crate offers a rich model and then a lossy convenience view that consumers
   bind to instead.

2. **`ProcessorPlace::cache_domain: Option<u32>`** -- one scalar, therefore one level.

3. **`core_affinity::Placement`** -- three tiers of locality: `SameCoreSiblings`, one cache
   boundary (`{Same,Cross}Cache` x `{Same,Cross}Class`), and `CrossNumaNode`. `Package`,
   `Die`, and `Module` are absent entirely, and every cache level except the selected one is
   absent.

## Consequences

**A. The label is not portable across machines.** "Same cache" means *same L2* on the x64
development host (eight L2 domains, a single non-partitioning L3) and would mean *same L3*
on a two-CCD part where L3 has two disjoint domains. The same word denotes a different
boundary depending on the machine. `HostFingerprint` does record `partitioning_cache_level`
alongside `cache_domain_sizes`, so a reader *can* disambiguate -- but only by consulting a
different field, and nothing in the label says to.

**B. It has already cost this project a row in its own measurement matrix.**
[DESIGN-NOTES.md](../crates/windows-waitable-queues/DESIGN-NOTES.md) records, of the x64
host:

> Conversely this host cannot express `same cache, same class` at all: its outermost
> partitioning cache is L2, shared by exactly the two siblings of one core, so any two
> processors sharing a cache domain are siblings.

That is attributed to hardware. It is at least half the model: those sixteen processors
**do** all share one L3. A per-level model would express "different L2, same L3, same class"
on that very host, which is precisely the row the note reports as inexpressible. The
neighbouring claim that "neither host alone can produce the full table" is therefore partly
self-inflicted, and worth re-checking against whatever this session concludes.

**C. Two different localities are conflated on any machine with two live boundaries.** On a
part with several L3 domains and several L2 domains within each, `CrossCache` covers both
"different L2, same L3" and "different L3", which are very different costs. Separating costs
by locality is the probe's entire purpose, and D-28's conclusions about peer-index caching
are keyed to these labels.

## What is outside the model entirely, and why

Write buffers, store buffers, and line-fill buffers are **not modelable from this source**.
`GetLogicalProcessorInformationEx` reports caches, cores, modules, dies, packages, groups
and NUMA nodes; it does not report store-buffer topology at all. This is a limit of the OS
surface rather than an omission in the crate, and it should be stated somewhere rather than
left as an implied gap -- the question is reasonable and its absence currently reads as an
oversight.

Whether a *measured* locality tier (something the placement probe establishes empirically
rather than reading from firmware) belongs in this model is a separate and open question.
Note that `Provenance` already exists to distinguish measured from reported claims, so the
crate has a place to put such a thing if the answer is yes.

## Design space

Not mutually exclusive; roughly increasing in cost.

**Option 1 -- name the projection, change nothing else.** Document that
`outermost_partitioning_cache` is one view and that `Placement`'s three tiers are a
deliberate projection, with the portability caveat (consequence A) stated at both. Cheapest,
and it converts an apparent assumption into a recorded choice. Does not address B or C.

**Option 2 -- add a level-agnostic primitive beside the projection.** Something in the shape
of `shared_cache_levels(a, b) -> Vec<u8>` or `deepest_shared_cache(a, b) -> Option<u8>`, so
that "do these share a cache?" becomes "at which levels do these share?". The projection
stays for callers that want one boundary to shard on, which is a legitimate scheduler
question. Unblocks SH-16.5 without deepening the collapse. Does not by itself change what the
probe reports.

**Option 3 -- make a measurement row name its own boundary.** Reshape `Placement` (or the
row that carries it) so "same cache" is qualified by level. Fixes A and C. Changes
measurement output, so it touches D-28's recorded conclusions and the fingerprint's
comparability across existing records -- which is exactly why it is a decision and not a
refactor.

**Option 4 -- generalise past caches.** `Package`, `Die`, and `Module` are modeled and
discarded. If locality tiers are the real subject, the projection is arguably
"which is the tightest domain these two share, over all kinds" rather than anything
cache-specific. Largest change; also the one that most directly answers "we have a network,
stop assuming three members".

## Open questions for the session

**Settled by the direction above:** question 1 below (the projection is kept, but as a named
projection over a connectivity model, not as the model); question 6 (the write-buffer note
belongs with question 2's measured tier, since that is the only mechanism that could ever
establish one).

1. ~~Is the single-boundary projection right, or wrong?~~ **Settled: it survives as one
   projection among others, and is documented as such.** A scheduler sharding work does want
   exactly one boundary; the error was letting that answer be the model.
2. If a row names its level, what happens to existing records and to D-28's conclusions?
   Are they re-derivable from what was recorded, or would they need re-measuring?
3. Does the matrix hole in consequence B actually close under a per-level model, on the
   hardware available? That is checkable and should be checked before it is claimed.
4. Should `Die` / `Module` / `Package` participate, or is cache-level generality enough?
5. Does a measured (as opposed to firmware-reported) locality tier belong in
   `windows-topology-sys` at all, given `Provenance` exists?
6. Where should the note about write buffers being outside the OS surface live?

## Status of dependent work

- **SH-16.5 is blocked on this session.** The contradiction it reports is real and still
  unfixed; `windows-placement-probe` still refuses a partially-covering level that
  `windows-topology-sys` deliberately permits.
- No other M16 item is affected. The other six findings from that round are fixed and
  committed.
