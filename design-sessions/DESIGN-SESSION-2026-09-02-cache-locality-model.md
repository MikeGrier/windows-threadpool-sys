# Design session: the cache-locality model

**Status: OPEN. No decisions taken yet.** This file records the question, the evidence
gathered while framing it, and the design space. It deliberately stops short of choosing,
because the choice affects measurement output and the conclusions already drawn from it.

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

1. Is the single-boundary projection *right for the probe's purpose* and merely
   under-documented, or is it wrong? A scheduler sharding work does want exactly one
   boundary; a probe characterising a machine may not.
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
