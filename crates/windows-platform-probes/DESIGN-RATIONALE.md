# Design rationale: windows-platform-probes

Tier 2. **How decisions were reached** -- the investigations behind them, the
alternatives weighed, and the reasoning that has since been superseded. The
current decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), which is
authoritative; where the two disagree, Tier 1 wins.

Split from DESIGN-NOTES.md at ba786ce.

Read this for "why did it end up like this", never for "what is it now". Several
sections below describe code that no longer exists -- the prose-reading oracle
and its instruments were retired by M3.4 and M3.5 -- and they are kept because
the reasoning is what a future reader needs when the same question comes round
again, not because the code is still there.

---

## The defects that survived were correspondence failures, and no instrument here could see them

<a id="d-correspondence-failures"></a>

**The diagnosis here is refined by [The encoded row is the contract; the prose is
not](DESIGN-NOTES.md#d-encoded-row-is-the-contract).** What each instrument could not see is
unchanged and is still the reason this component has an oracle at all. What this
section got wrong is WHERE the two defects lived: both were defects in the
ENCODED ROW, not in the relation between two renderings of a consistent state.

This probe was reviewed twenty-eight times before it opened as a pull request,
by two independent readers per round on different models, with `cargo-mutants`
reporting **zero surviving mutants** on both of its modules. A review on the
pull request then found, in code none of that had touched, a state where the
renderer printed

```
BUG IN THIS PROBE: the topology crate named L3 as the outermost
partitioning cache and this survey carries no summary for it. Nothing
below about cache partitioning can be trusted.
```

while `cross_check` had no branch for that state at all, so `verdict()` could
return `Agree` for the same run and print `=> agree` two paragraphs below. A
second finding in the same review had the same shape: one fact rendered twice
in one report -- `efficiency classes: [0]` in prose, `"efficiency_classes":1`
in the NDJSON -- in two shapes a consumer cannot reconcile, where the numeral
happens to read as a plausible class *label*.

Neither is a bug inside a function. Every function involved was correct on its
own terms, and each had been read repeatedly and found so. The defect lived in
the **relation between two artifacts**, and that is a place none of the
instruments in use could look.

### Why each instrument was structurally incapable, not merely unlucky

**Mutation testing cannot find absent code.** `cargo-mutants` perturbs what is
written and asks whether a test notices. A missing branch has no mutants, so
the missing `SummaryMissing` check did not lower the score -- it was invisible
to it. The 180/0 result was true and said nothing about the gap. A perfect
mutation score is compatible with an entirely missing feature, and this
component is the proof.

The same run also shows the weaker half of what a mutation score means. A test
existed asserting `"efficiency_classes":2`, so every mutant of that line died.
It was pinning the wrong shape faithfully. **Mutation testing measures whether
behavior is pinned by tests; it is silent on whether the pinned behavior is
right.** Both halves were over-read here for many rounds as though they were
evidence of correctness.

**Exhaustiveness checking protects `match` expressions, not concepts.**
`PartitioningCache` exists precisely to force a decision -- its own doc says a
renderer or serialiser "cannot emit the absent case without having decided
which absent case it is" -- and it worked, in the two consumers that wrote a
`match`. It bought nothing in the two that did not: `domain_counts` reached the
same information through `outermost_partitioning_cache`, a second accessor
returning `Option`, which launders five states into two; and `cross_check`
never asked. A type can only compel a consumer that consults it.

**Per-artifact review finds per-artifact defects.** Two readers checking each
function against its own documentation will confirm both sides of a
contradiction, because each side is locally true. Worse, the readers were
answering questions posed in a prompt, and across rounds that prompt
accumulated focus areas and "already verified, do not re-litigate" facts. The
shared prompt correlated the readers far more strongly than their differing
models decorrelated them; the instrument was being shaped to agree with its
author. Removing that framing in the final round is what got a reader to trace
`simultaneous_multithreading` out of this crate into `windows-topology-sys` and
check it against the Win32 `LTP_PC_SMT` contract.

The single sentence that covers all three: **every instrument in use verified
properties of things that exist.** Tests assert existing behavior, mutation
perturbs existing code, reviewers check written claims. A correspondence
failure is a property of a *pair*, and an absent branch is not a thing at all.

### Integration-level analysis was absent, which is where these live

At the time of the pull request the crate had one integration test, asserting
that a probe writes something to stdout. Of twenty-five `report()` calls in the
suite, **none rendered from a real host's `measure()`** -- every one used a
synthetic `Observation` built by hand. A hand-built fixture can only contain
states its author already imagined, and each assertion checked one local fact
about it. Nothing anywhere rendered the artifact a consumer actually reads and
asked whether it was self-consistent.

### What to do instead: a sparse matrix to explore with, an oracle to keep

The obvious response -- tabulate every state against every consumer and fill
the grid -- is wrong, and was proposed and rejected during this analysis. Such
a table grows combinatorially, most of its cells are meaningless, and a version
of it committed beside the code would be a second copy of the code's structure
that nothing verifies. It would rot exactly as every restatement in this
component rotted, and a stale "all cells covered" table is more dangerous than
no table.

The division that does work:

- **The matrix is a transient, exploratory instrument.** Draw it for one type
  at one boundary to find out which correlations exist. It is expected to be
  **sparse**; most cells are empty and discovering that is cheap. Correlations
  cannot be derived -- which is why twenty-eight rounds of reading produced
  none -- so populating it is exploration, not specification.
- **An oracle is the durable artifact.** Only cells that turn out to mean
  something graduate into it. It stays small because discovery, not
  enumeration, fills it.

`windows-file-watcher`'s `ContractChecker` is this repository's worked example
of the oracle half: a shared executable definition of the rules, owned by the
crate that owns the contract, that the producing crate's own tests and every
consumer's test doubles all bind to. It already existed while this probe was
being written, and was not reached for.

Every correlation admitted here is one the report already renders twice, with
nothing relating the two -- a property of the artifact rather than of anyone's
intuition about it. The ones this decision was written against are:

1. an alarm in the report implies the verdict is not `agree`;
2. a fact rendered twice must agree across its renderings;
3. an uncaveated hardware claim implies `!parse_in_doubt`;
4. the banner names the same machine the body describes.

That list is the seed, not the census: the admission RULE is what governs, and
the authoritative set is the `Correspondence` enum in
[src/report_oracle.rs](src/report_oracle.rs). An earlier version of this
paragraph said "three correlations" and listed the first three while the enum
already had the fourth -- the count was wrong when written and would have
rotted again at the next addition, so it is stated as a rule here instead.

The replacement rule was then itself overstated, as "known to be real because it
was violated" -- which excludes the correspondences found by the M2.4 matrix,
where no defect had occurred and walking every NDJSON field against the prose is
what showed the fact rendered twice with nothing comparing it. Both routes are
admissible; what is not is inventing a correspondence between things the report
does not actually render twice. Corrected the same day it was written, after a
review noticed it contradicted `check_structured_pairs`' own history.

What makes an oracle different from more tests is where it is invoked: if every
test renders *through* it, every existing call site inherits the checks and so
does every future one. A test added beside them checks one case; an oracle
checks every case anyone ever writes. (Also stated without a number on purpose
-- this said "all twenty-five existing call sites" and there are now 35.)

**Record the vacuous findings too.** "We examined whether X and Y must
correspond, and they need not" is a result, and it is the half that normally
evaporates -- without it the next person re-explores the same empty cells.

An oracle is a forcing function for correlations already discovered. It will
not find a new one. The discipline that makes it compound is that each newly
found cross-artifact contradiction adds an invariant to the oracle rather than
a one-off test.

Whether this generalises to `Coherence`, `BracketOutcome`, `Verdict` and the
sibling probes is **an open question, deliberately not answered here.** The work
this decision implies is queued as M2 in [CHECKLIST.md](CHECKLIST.md); this
section schedules nothing on its own.

## The oracle exists, and what it deliberately refuses to know

<a id="d-oracle-refuses-to-know"></a>

**Superseded by [The encoded row is the contract; the prose is
not](DESIGN-NOTES.md#d-encoded-row-is-the-contract), and the code it describes no
longer exists.** Everything below is a record of what was built and why, in the
past tense whatever its grammar says: M3.4 deleted the prose correspondences,
the `Correspondence` enum and the twenty-three extraction helpers, and M3.5
replaced the fact-accounting instrument. `report_oracle` today is a
well-formedness check on the row and nothing more.

This paragraph read "the rest of this section ... still describes what is in the
tree and still holds", which was true when it was written -- before M3.4, three
commits earlier on the same branch -- and was carried through the Tier 1 / Tier 2
split unchanged. Found by a review. It is the same drift this component keeps
paying for, and it is worth leaving the correction visible rather than quietly
deleting the sentence.

M2.1 built it: [src/report_oracle.rs](src/report_oracle.rs), admitting only
correlations the report already renders twice. The defect that forced
it is the section above.

**It reads the rendered artifact, never the state behind it.** Checking state
would miss precisely this defect class -- in the original finding the state was
consistent and the two *renderings* of it were not.

That last sentence is the superseded one, and it is wrong about its own
evidence. Re-checked against the code: the alarm has no NDJSON key, and
`cross_check` does -- so the original finding was a run whose ENCODED ROW said
`agree` while the probe had detected its own bug, and published nothing about
that bug. The state was not consistent; the row was wrong. See
[#d-encoded-row-is-the-contract](DESIGN-NOTES.md#d-encoded-row-is-the-contract).

**It relates two things already visible in the report, and re-derives nothing.**
A second implementation of the rendering rules would be a check of the copy
rather than of the contract, and would drift the moment either moved. So the
alarm rule compares an alarm line against a verdict line, the double-rendering
rule compares prose against NDJSON, and the gating rule compares a claim against
the report's own published evidence of doubt.

That last one is the interesting boundary. `CrossCheck::parse_in_doubt` is
`!disagreements.is_empty() || !parse_incomplete.is_empty()`, and the NDJSON
publishes `parse_incomplete` as a **list of conditions** rather than the
predicate -- so the oracle reads whether that list is empty, together with the
`disagree` verdict, and those are the two visible shadows of that definition. The
coupling is deliberate, and confirming it still holds is what M2.2's sabotage
check is for when the call sites are bound.

(This said "as a **count**", which M3.1 made false when the three diagnostic
fields began publishing their conditions. The shape of the argument is
unchanged -- the row still renders a shadow of the predicate rather than the
predicate -- but the shadow is now a list, and an emptiness test rather than a
comparison against `0`.)

**Half the tests assert acceptance**, following
[../windows-file-watcher/src/contract.rs](../windows-file-watcher/src/contract.rs)'s
`ContractChecker`: an alarm beside a non-agreeing verdict is legal and is what
the fix produced, a caveated claim under doubt is legal and is what the renderer
emits on every heterogeneous host with a short parse, and a prose-only report is
silence rather than violation. Over-constraining is the same defect as
under-specifying and fails in the more expensive direction, because noise trains
a reader to ignore the instrument.

### The failure mode that would look exactly like success

An oracle whose prose labels do not match the renderer reads nothing, finds
nothing, and passes everything. So the labels were confirmed against a real
`probe-topology` run, and a test corrupts each double-rendered value in turn and
requires a violation -- if a label ever drifts, that test fails rather than the
oracle going quietly blind.

**The first attempt at that injection silently did nothing**, and is worth
recording because it nearly produced the opposite conclusion. The anchor used
was `cross-check:`, which does not occur -- the real text is `cross-check
against independently read Win32 counters:` -- so the "defective" report was
identical to the clean one, the oracle correctly reported no violation, and the
reading was almost "the oracle is blind". A sabotage that fails to apply is
indistinguishable from an instrument that fails to fire, unless the injection
asserts it changed something. It now does.

**The mirror-image hazard: a RESTORE that fails to rebuild.** Sabotage work in
this crate is a loop -- break it, run it, put it back, run it again -- and the
put-it-back step has its own way of lying. On Windows, PowerShell's `Copy-Item`
preserves the source file's `LastWriteTime`, so restoring a file from a backup
taken earlier gives it an mtime OLDER than the artifacts built from the
sabotaged version. Cargo fingerprints by mtime, decides nothing has changed, and
reruns the previous binary. Measured here: a restored, correct oracle reported
the fixed defect as still present, and the reading was almost "the fix does not
work" -- the conclusion was only avoided by printing the intermediate values and
finding that the function returned the right answer while the test insisted it
did not.

After restoring a file by copy, set its timestamp forward
(`(Get-ChildItem <path>).LastWriteTime = Get-Date`) or rewrite it through a
read-then-write, which stamps it as a matter of course. The general rule is the
same one as above, pointed the other way: **a green result proves nothing until
you know the code you are testing is the code you just wrote.**

### The real-host test, and the guard that stops it passing for nothing

[tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs)
composes the report the way `probe-topology` does and applies the oracle
explicitly.

**Why it has to exist.** The oracle's unit tests pin it against fixtures, and a
fixture is a report somebody wrote down -- so a fixture-bound oracle checks
correspondences over states its author already imagined, and the defect it
exists for was a state nobody had imagined. More narrowly, a fixture cannot
notice the *renderer* drifting away from the prose labels the oracle reads:
both sides would still agree with each other. Only the real artifact disagrees.

Some unit tests in this crate do call `measure()` and so do read this host.
What none of them does is run the **oracle** over a report rendered from that
reading, which is the gap this test closes. On CI it runs across the hosted
runner fleet, a survey of shapes no fixture anticipates.

**It asserts nothing about this machine, deliberately.** A test expecting a
processor count, a cache level or a verdict would fail on the next runner shape
rather than on a defect, and would have to be loosened until it asserted
nothing. What it checks is that whatever this host produced, the report's parts
agree with each other -- a property every host must satisfy, including one whose
topology cannot be read at all.

#### The primary assertion can pass having checked nothing

On a host whose report the oracle cannot parse, every lookup returns `None`,
every comparison is skipped, and
`a_report_rendered_from_this_host_agrees_with_itself` passes having checked
exactly zero correspondences. That is why the second test corrupts each
double-rendered fact in a report **this host really produced** and requires the
oracle to report a violation **naming that fact** -- and asserts first that the
corruption changed the text at all, for the reason recorded above.
Eight facts rather than one, because corrupting a single field would leave the
others unguarded: the renderer could drift away from the oracle's other prose
labels and the test would still pass on the strength of the one that remained.

**"Naming that fact" is load-bearing, and took three attempts to get right.**
The guard first required only that the violation list was non-empty. That was
strengthened to require a `ProseAndNdjsonDisagree`, with a comment correctly
observing that corrupting the NDJSON `processors` count also trips the
cross-check counter rule -- and then not acting on the observation, because that
counter rule emits precisely that variant. So the strengthened guard still
passed while the reader it was written to protect was blind.

Measured, by blinding one `DOUBLE_RENDERED` prose label at a time: under the
variant-only assertion, `processors` and `groups` both stayed **green**, masked
by their counter rules, while `packages` went red because nothing else reads it.
Two of the four facts the guard names as required were unchecked by the guard
whose entire purpose is to establish that they are checked. Matching on the
`fact` string closes it, because no neighbouring rule can supply another rule's
fact name.

The general lesson, which this crate has now paid for four times: **a guard
against vacuity is itself a claim, and is subject to the same discipline as any
other.** Each of the three earlier versions was written to fix the previous
one's vacuity and introduced the next-narrower version of it. The only thing
that has ever settled the question is sabotaging the mechanism the guard is
supposed to protect and watching the guard go red -- never reading the guard and
judging it sufficient.

### The fact set is derived, because the gaps were never in the rules

Six unread double-renderings were found on this branch by six different
reviewers, and none by the oracle's own coverage. Each individual gap was real,
and fixing each one by hand was correct -- but the pattern is the finding, and
the pattern is structural: **a rule is added per fact, so the SET of facts is
what drifts.** Nothing derived that set from
[src/topology_report.rs](src/topology_report.rs), so a field added to the
renderer was unread until a person happened to notice.

The clearest demonstration is gap 6. It was created BY the hand-written rule
that closed gap 2: M2.11 added a comparison for the partitioning discriminator,
that arm renders a level number as well, and nothing compared the number. Closing
a gap added a fact, and the new fact was unread by exactly the mechanism that
produced the previous five.

`every_fact_the_renderer_publishes_is_accounted_for` in
[tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs)
is the answer, and its shape matters more than its code. Both halves come from
places that cannot fall out of step:

- **The set of facts is enumerated from the artifact** -- the NDJSON line of a
  report this host really rendered. Not a list in the test. A new field appears
  in the enumeration by itself, without anyone maintaining anything.
- **Read-or-unread is measured, not declared** -- each value is corrupted in turn
  and the oracle is asked. A rule that quietly stops reading a fact is caught
  even though no list changed.

Only the classification of each key is written down, because which of the three
kinds a key belongs to is a judgement: compared, conditional on its prose being
rendered, or having no second rendering at all. **A key in no list fails**, and
that is the whole point -- adding a fact to the renderer forces the judgement to
be made deliberately instead of being discovered by the seventh reviewer.

This is the same discipline the oracle itself rests on, turned on the oracle:
relate two things that already exist rather than restate one of them. A second
list of "facts the rules look at" would have been another copy to drift, which
is the defect rather than the fix.

**The limit, stated because a reader will otherwise assume it is closed.** The
enumeration is keyed to the machine-readable line, because that is the side with
an enumerable structure. A fact rendered only in PROSE, with no field beside it,
is invisible here. Prose is not enumerable without parsing English, so that half
remains a thing only a reader notices -- and saying so is better than implying a
coverage that does not exist.

### The instrument is code too, and it is where the defects were

Eleven review rounds across six models ran over this branch. Classifying every
finding by what would have caught it earlier is more useful than the findings
themselves, because the classes are very unevenly sized.

**The largest class by far is shape blindness**, and it has one root: every
instrument here was validated against a single artifact, the developer machine's.
This host produces exactly one shape -- measured, `agree`, the `Level`
partitioning arm, non-empty caches, classes `[0]`, zero anomalies, zero
`not_compared`, x86_64 -- and every defect in that class lived in the complement
of it. The architecture uncompared on an unmeasured report, `efficiency_classes`
comparing contents so a scalar and a one-element list were identical, the anomaly
count unread under two of three verdicts, the expected fact name differing on the
`SummaryMissing` arm, an empty container that could not be corrupted: none of
them can occur here, and each was found only because a reviewer imagined a shape
by hand. [CHECKLIST.md](CHECKLIST.md) -> M2.12 is the structural answer, and it
is M2.10's move one level up: derive the set of SHAPES from the renderer's
branches, as M2.10 derives the set of FACTS from the artifact.

**The second observation is the one worth carrying to other crates.** Most of
these defects were not in the oracle. They were in the guard that checks the
oracle, the table of required facts, the fact name the guard expects, the
declaration of when silence is legitimate. The thing under test came through the
last rounds clean; the things doing the testing did not.

An instrument feels like it sits outside the system under test, so it escapes the
discipline applied to production code -- and then it fails on a CI runner
reporting a defect in the renderer that is really a defect in the instrument,
which is worse than no check at all because it sends the reader to the wrong
place. Instruments need what production code gets: derivation instead of
restatement, sabotage before they are believed, and a stated boundary.

**And a fix is new code.** Twice on this branch the fix for one gap created the
next: M2.11's discriminator rule gave the level a second fact name, which broke
the guard that expected one; M2.10's accounting inherited a silence assumption
that only holds under an `agree` verdict. After a fix, re-run the derivation or
the sweep -- not only the test that prompted it.

### Mutation testing is how the instruments got checked, and what it cannot reach

Every defect on this branch was found by a person reading code -- reviewers,
mostly, and this file records how often they found the same shape. Late on, the
obvious question got asked: is there a mechanical way to ask whether a test
establishes anything, rather than trusting that it does?

There is, and it was already installed. `cargo-mutants` changes the source and
asks whether anything notices, which is the sabotage loop this crate has been
running by hand all along, done exhaustively. It answers a stronger question than
coverage: not *was this branch executed* but *does anything DETECT a change to
it*. On a branch whose recurring defect is a test that runs code without
establishing anything about it, that difference is the whole point.

Six sweeps, run through [tools/run-mutants.ps1](../../tools/run-mutants.ps1). The
first three predate the M3 rewrite and are kept for the arithmetic note below;
the last three cover the three modules M3 created, none of which any sweep had
reached:

| file | tested | caught | unviable | survivors |
|---|---|---|---|---|
| `topology_report.rs` | 28 | 28 | 0 | 2, then none |
| `report_oracle.rs` | 143 | 138 | 5 | 6, then none |
| `topology.rs` | 186 | 180 | 6 | none, first run |
| `row.rs` | 25 | 19 | 6 | none, first run |
| `topology/invariant.rs` | 26 | 21 | 2 | 3, then one equivalent |
| `topology/diagnostic.rs` | 28 | 23 | 5 | **12**, then two prose, then none |

**The three later sweeps are the argument for running them at all**, and each
made a different case. `row.rs` -- the crate's only defence against caller text
reaching the mined artifact -- came back clean on its first run, which no amount
of review could have established. `topology/invariant.rs` gave up a real gap that
five review rounds across four models had not: relaxing `online_processors > 0`
to `>= 0` survived, because the test that NAMES that boundary asserts on
`cross_check` and so covered only one of the two deliberate copies of the
condition.

**`topology/diagnostic.rs` is the one that mattered.** It survived 12 of 26 --
46% -- and the survivors were the row's own payloads: `NotCompared::code` could
be replaced wholesale with `""`, every arm of `published_anomaly` deleted, and
two arms of `anomaly_code` deleted so that a real buffer overrun would publish as
`unclassified`, whose documented meaning is the opposite. Measured separately:
rewriting the `count` helper so every published count was wrong left 218 tests
green.

This is the field-labelling defect [src/row.rs](src/row.rs) exists to make
unrepresentable, reappearing one level down. `row.rs` pairs a name with its value
so position cannot mislabel them; these functions then hand-pair names with
values INSIDE each entry, and nothing was watching. The tests that looked like
they covered it built their expectation from `code()` and compared it against a
row the writer had built from `code()` -- both sides moving together, which is
the tautology class this branch deletes elsewhere.

Closed with goldens in [src/topology/diagnostic/tests.rs](src/topology/diagnostic/tests.rs),
written as literals on purpose: a code is a SCHEMA, not a predicate, and a schema
is not derivable from the thing that emits it. Completeness is compiler-checked
where the enum belongs to this crate, via an exhaustive `match` in the test.

**The last two survivors were both `Display` impls, and how they were closed is
the part worth keeping.** Blanking either left the suite green, and a reader
would have got `     - ` with nothing after the dash. Both obvious fixes were
wrong: pinning the sentences would make the prose machine-checked, which
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `d-encoded-row-is-the-contract` deliberately
does not do, and leaving them ships a blank line.

The engineer's question -- "a blank line is perhaps wrong, perhaps it should be
called out?" -- named a third answer better than either. **A diagnostic's WORDING
is a review obligation; its PRESENCE is not.** The renderer now routes every
entry through `described`, which substitutes an explicit `BUG IN THIS PROBE` line
for a rendering that is empty or blank, and a test asserts that no diagnostic
renders as that line. That pins THAT each entry describes itself without pinning
WHAT it says.

Both improvements fall out of the same branch. A reader gets a stated defect
instead of a blank -- which is indistinguishable from a rendering bug, from a
finding with genuinely nothing to say, and from a stray newline -- and the
mutants become catchable, because blanking a `Display` now produces the callout
the test forbids. The sweep went to **zero survivors**, including the two mutants
`described` itself introduced.

The `assert_holds` survivor is equivalent, for the same reason `assert_corresponds`
survived below, and the argument is now recorded at the function rather than left
for the next sweep to rediscover.

The unviable column is why a caught-count does not equal a tested-count: those
mutants did not compile, so they say nothing either way. An earlier version of
this table gave only the caught figures, and the commit that wrote it summed them
to a total that matched neither -- 357 were tested and 346 caught, and it claimed
352. Stated in full here so the arithmetic is checkable rather than asserted,
which is the same rule this branch keeps having to relearn about numbers.

**The survivor worth remembering is `assert_corresponds`.** Replacing its body
with `()` survived, because every instrument that would notice goes THROUGH it --
both renderers are bound to it, the real-host test calls it, the corpus reaches
it by rendering -- so a no-op assertion makes all of them pass together. Nothing
asserted that the assertion asserts. That is a blind spot no amount of adding
tests *through* an instrument can find, and it is exactly what a tool that
attacks the code rather than the tests is for.

The others were smaller and of one kind: guards that only fire on malformed
input, which every fixture was too well-formed to reach. A `> 0` that stops an
empty value being reported as a value; the `&&` that makes `!!...!!` a shape
requiring both ends; a string branch whose only caller always passes an array.

**What it cannot reach, and this matters here.** `cargo-mutants` mutates `src/`,
not `tests/`. The accounting table, the shape corpus and the fact-name guard all
live in `tests/`, so the tool validates the code they check and says nothing
about THEM. The instruments remain exactly as good as the hand-sabotage that
built them -- which is where several of this branch's defects were found, and
where the next one will be. A clean sweep is evidence about the oracle, not about
the things measuring it.

## Why the crate reports observations instead of verdicts

Recorded for [D-observations-not-verdicts](DESIGN-NOTES.md#d-observations-not-verdicts).

The rule was earned, not designed. The queue-contention note had carried the
claim that re-apportioning a queue's position/reservation bits was **free** --
that 16/48 and 8/56 "track the default within noise" -- and therefore that buying
twenty years of counter headroom cost nothing. Two independent defects sat under
that sentence.

The first was arithmetic-shaped: the table directly beneath it showed 1.21x and
1.13x, against a noise floor the same document put at 2-6%. The prose
contradicted its own evidence, in adjacent lines, and survived several review
passes anyway -- because "within noise" reads as a conclusion rather than as a
claim about a measured quantity, so nobody checked it against the number.

The second was deeper. The 2-6% floor had itself been obtained by comparing **two
runs**, which cannot measure a spread at all. Re-running the probe seven times
put the same-configuration spread at 7-61% depending on producer count. So the
floor every "within noise" judgement in the section had been made against was off
by roughly an order of magnitude, and the judgements were not recoverable by
adjusting it.

What made the repair possible was already in the probe's output. `reserving_mpsc`
and `reserving(32/32)` are the same code at the same layout, measured twice per
run, so their ratio is an *empirical* answer to "what does no difference look
like here" -- 0.68-1.27x across seven runs. That is a control the instrument
derives rather than a floor the prose asserts, which is
[D-derived-not-restated](DESIGN-NOTES.md#d-derived-not-restated) applied to a
measurement instead of to a fact.

**The tempting repair was to invert the claim**, since the seven-run medians put
the re-apportionments at 1.23-1.30x at high producer counts. That would have been
the same error with the opposite sign: one host, one microarchitecture, a single
NUMA domain, against a control whose own excursions reach 1.12x. The claim was
withdrawn in both directions instead, and the section now says which
configuration is worth measuring locally rather than what the answer is.

This generalises to where the crate draws its line. Coarse claims that follow
from how the hardware works *and* are backed by observation -- "the buffers
should be in the same memory domain as the executor" -- are worth making, and
portable enough to be useful. Fine-grained topological and layout choices are
not: they depend on parameters the capture does not record and the reader's
machine does not share. The shipping queue takes its layout as a type parameter
precisely so that choice belongs to the client; a design note that quietly picks
one on their behalf takes it back.

**On the capture parameters themselves.** Windows exposes no NUMA distance table,
so "how far apart are these nodes" is unanswerable on this platform. The analog
the crate uses is the processor-to-node assignment carried in the banner's
`numa[...]` field, with device-to-node mapping available on the same footing. It
answers the same-domain question, which is what most placement decisions turn on,
and it is why `numa[16]` on the measurement host is worth stating plainly: a
single domain means the queue figures say nothing about cross-domain behaviour at
all.
