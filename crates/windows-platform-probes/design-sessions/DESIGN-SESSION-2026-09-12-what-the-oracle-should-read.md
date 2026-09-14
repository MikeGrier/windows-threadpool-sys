# Design session 2026-09-12: what the oracle should read

Decisions resulting from this session:

- [DESIGN-NOTES.md](../DESIGN-NOTES.md) -> [The encoded row is the contract; the prose is
  not](../DESIGN-NOTES.md#d-encoded-row-is-the-contract) (new, and supersedes the
  artifact-reading rule recorded in [The oracle exists, and what it deliberately refuses to
  know](../DESIGN-RATIONALE.md#d-oracle-refuses-to-know)).

Work queued from it: [CHECKLIST.md](../CHECKLIST.md) milestone M3.

## The question

Held immediately after PR #88 merged. The engineer asked, of the oracle that PR had just
built: **why does it have to be based on the formatted string?**

The recorded answer at that moment was the one in the module doc of
[src/report_oracle.rs](../src/report_oracle.rs) and in Tier 1: it reads the rendered
artifact, never the state behind it, because "in the original finding the state was
consistent and the two renderings of it were not."

The session set out to test that sentence against its own evidence rather than to defend
it.

## What the evidence turned out to be

The oracle's charter names two originating defects, both found by a pull-request review
after twenty-eight rounds of per-artifact review and a zero-surviving-mutant `cargo-mutants`
result had passed over them. They are recorded in [DESIGN-NOTES.md](../DESIGN-NOTES.md) ->
[The defects that survived were correspondence
failures](../DESIGN-RATIONALE.md#d-correspondence-failures).

### Defect 1 -- the alarm beside the agreeing verdict

The renderer printed `BUG IN THIS PROBE: the topology crate named L3 as the outermost
partitioning cache and this survey carries no summary for it. Nothing below about cache
partitioning can be trusted.` while `cross_check` had no branch for that state, so
`verdict()` could return `Agree` and print `=> agree` two paragraphs below.

Checked during the session, in the code as it stands on `main`:

- The alarm is emitted by a `writeln!` into the prose, in `report`'s
  `PartitioningCache::Level` arm of [src/topology_report.rs](../src/topology_report.rs).
- The NDJSON row carries eighteen keys **as of this session** -- it carries nineteen now,
  because the work this session set off added `disagreements` (in `77a83fc`, after
  [4d7544e](../DESIGN-NOTES.md) recorded this). The count is left at eighteen on purpose: a
  review reported it as a stale census, and it is not one. The diagnosis below turns on
  what the row held AT THE TIME, so correcting the number to nineteen would make the next
  bullet -- "there is no key for the alarm" -- read as an error rather than as the finding.
  For the current schema read `MEASURED_ROW_KEYS` in
  [src/topology_report.rs](../src/topology_report.rs), which is the one authority:
  `reason`, `arch`, `processors`, `groups`,
  `packages`, `numa_domains`, `numa_domains_without_processors`, `cores`,
  `efficiency_classes`, `caches`, `outermost_partitioning_cache_level`,
  `outermost_partitioning_cache`, `policies`, `cross_check`, `not_compared`,
  `parse_incomplete`, `enumeration_anomalies`, `numa_domains_only_in_cpu_sets`.
- **There is no key for the alarm.** `cross_check` is there, and on the defective run it
  read `agree`.

That changes the diagnosis. The defect was not a disagreement between two renderings of a
consistent state. **The encoded row was wrong**: it certified a run as `agree` on which the
probe had detected its own bug, and it published nothing at all about that bug. A
fleet-survey pass mining the NDJSON would have counted the host as a clean agreeing
measurement and never known otherwise.

The prose alarm was not the defect. It was the only visible trace that the encoded row was
wrong -- which is exactly why a human reviewer found it and no instrument did.

So the repair that actually addresses defect 1 is not "compare the prose against the
NDJSON". It is **publish the alarming condition structurally, and assert the invariant on
the data**: a run that detected a missing summary cannot also be `agree`. That is
checkable on the observation, before any rendering, with no parser.

### Defect 2 -- `efficiency classes: [0]` against `"efficiency_classes":1`

One fact rendered twice, in two shapes a consumer cannot reconcile: the prose printed the
set of class labels, the NDJSON printed the cardinality, and the numeral `1` reads as a
plausible class *label*.

Both halves were correct derivations from one consistent value, so no struct-level
predicate was violated. At first reading this looks like the strongest case for an oracle
that reads text -- a contradiction that exists only in the representation.

But look at how it was actually repaired. The row now emits
`"efficiency_classes":[0]` -- the class LABELS as a list. **The fix was to change what the row publishes.**
The prose comparison was the route by which a reviewer noticed, not the repair.

### A correction made while writing this up, and it strengthened the case

The first draft of the Tier 1 decision said the alarm's condition is unpublished and
the verdict can therefore still read `agree`. Checked before committing, and the second
half is **no longer true**: `Observation::cross_check` now pushes
`PartitioningCache::SummaryMissing` onto `parse_incomplete`, which forces the verdict away
from `agree`. That was the original repair, and it worked.

Writing the superseding decision on a description of the code as it used to be would have
been the exact defect class this branch has spent fifteen review rounds on. What the check
found instead is a gap that is live, and larger:

The row publishes `not_compared`, `parse_incomplete` and `enumeration_anomalies` as
**counts** -- `check.parse_incomplete.len()` -- where the prose prints each entry's text. A
survey reading `"parse_incomplete":1` cannot tell *the probe detected a bug in itself* from
*a core record contradicted itself* from *cache levels numbered 0* from *this topology was
not measured from a running machine*. Four categorically different facts, one cardinality,
and only the prose separates them.

So the accurate statement of the problem is not that the row disagrees with the prose. It
is that **the row is impoverished relative to the prose**: the artifact that gets mined
carries strictly less than the artifact that gets read. Which is the engineer's point,
sharpened -- the effort went into comparing the two halves when the encoded half was the
one missing content.

### Both defects were defects in the encoded data

That is the session's central finding, and it was not what either the module doc or Tier 1
said. Defect 1 was a wrong value in the row plus a missing field. Defect 2 was the row
publishing a cardinality where the useful fact was the set. Neither required a
prose-against-NDJSON oracle to *fix*; both required the structured output to be made
right.

## The cost that was being paid for the other reading

Counted during the session, in [src/report_oracle.rs](../src/report_oracle.rs): of 38
top-level functions, ten are correspondence rules and four are comparison helpers --
and **twenty-three exist purely to extract values back out of rendered text**
(`fingerprint_tokens`, `ndjson_field`, `ndjson_raw_field`, `balanced_end`, `claim_block`,
`prose_field`, `cache_object`, `cache_rows`, `policy_rows`, `leading_digits`,
`has_line_beginning`, `object_keys`, and the rest).

That ratio is the indictment, and the branch's own review history is the evidence for it. A
large share of PR #88's review rounds were defects in **the reader, not in the thing read**:
a multi-byte panic in `processors_in_banner`, a `p/` substring matching inside an opaque
`io::Error`, `trim_matches` collapsing `[[0]]` and `[0]` to the same value,
`prose_field("  (")` selecting the wrong line when two lines began the same way.

Those are not defects in the probe. They are defects in a hand-written parser of output
this crate had just finished writing -- a defect source the design invented for itself.

## The engineer's framing, which the evidence supports

> There is a data pipeline, and at some point it renders either to a structured format or
> to prose. The prose has to be accurate and readable, but I really do not see value in
> ensuring it is programmatically comparable. The structured format? Yes, absolutely.
>
> Yes, humans read the prose. But we should present prose which you and then optionally I
> will inspect for correspondence, and then once that's done, we'll move along. And there
> may be defects there. But it's the defects in the encoded data that really matter.

This is the decision recorded in Tier 1. The session's contribution is that it is not
merely a preference about where to spend effort -- the two defects that motivated the
oracle in the first place were both defects in the encoded data, so the evidence that was
taken to argue for reading prose argues for the opposite.

## Alternatives considered

**Keep the oracle as it is.** Rejected. It is not wrong, and it does catch real things --
but it aims the expensive machinery at the artifact with the lower stakes, and pays for it
with twenty-three parser functions that are themselves a defect source. The prose is
reviewable by a human; the row is mined by a machine across a fleet and is what this
workspace's designs rest on.

**A typed banner (the previous M2.18).** Dissolved into this decision rather than answered
on its own terms. It was the smallest instance of a general question -- should a report be a
value that a writer renders, or a string the renderer concatenates -- and answering it in
isolation would have fixed one parameter while leaving the same shape everywhere else.

**A structured report checked as a struct, with the oracle moved wholesale onto it.**
Tempting and half right. It closes value-divergence and injection by construction, which is
better than detecting them. But it must not be mistaken for a complete answer: a check on
the struct says nothing about the writer, and the writer is where several of PR #88's
defects lived (`is_attribution_shaped` matching a suffix, the flattening that ate the
disclaimer, the `host: host:` doubling). The conclusion taken was the narrower one -- assert
the invariants on the data, emit the row through one typed writer so injection and
field-order defects are unrepresentable, and keep only a thin check that the row is
well-formed.

**Rejected framing: "the prose does not matter".** Not what was decided, and worth stating
because it is the easy misreading. The prose must be accurate; an overstated finding in
prose propagates into design notes that cite it, which is a live concern in this crate --
M2.9 is an open item about exactly that. What was decided is that prose accuracy is a
**human review** obligation rather than a machine-checked correspondence.

## What survives from PR #88

Worth recording, because the decision reads as a larger reversal than it is:

- The correspondence *rules* survive, relocated. Alarm-against-verdict,
  diagnostics-against-verdict and counters-against-verdict all become invariants on the
  observation. What they lose is the parser in front of them.
- The containment work in [src/topology_report.rs](../src/topology_report.rs) survives and
  matters more under this decision, not less: it stops caller text reaching the row.
- The shape corpus and the fact-accounting instrument survive in shape, re-aimed at the
  row's fields rather than at prose labels.
- What retires is the prose-against-NDJSON family and the extraction helpers that serve
  only it.
