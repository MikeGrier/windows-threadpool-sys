# Design notes: windows-placement-probe

Decisions about the tool that measures what thread placement costs and produces
a record someone can paste into a discussion thread.

## crates.io is a second path, and it must not become the first one

**SUPERSEDED 2026-09-02: there is no second path. This crate is never published
to a registry.** The GitHub release binary is the only distribution, and
`publish = false` now states that permanently rather than temporarily. The
reasoning below is kept because it is still correct about *why* the crates.io
path is weaker, and because the decision it records was reversed on new
information rather than on a change of taste.

**Why the reach argument no longer holds.** It rested on crates.io adding
reach. Measured against the path that actually exists, it subtracts: a released
binary needs **no Rust toolchain at all**, while `cargo install` needs a
toolchain, a compiler, and a successful build of this crate's whole dependency
tree. The audience crates.io adds is therefore a *subset* -- Rust developers who
would rather type a command than click a link. That is a convenience, not reach,
and it is bought by making the weakest-provenance path the most discoverable
one. M5's own preamble had already said the download "needs no Rust toolchain"
and is "the provenance"; the reach premise contradicted a conclusion this
project had already reached.

**And a cost that was not known when the original decision was made.** A
published crate cannot depend on a bare `path`, so every dependency needs a
`version` that must be kept in lockstep with the workspace. That is not a
publication-time chore, as the note below assumed -- **cargo enforces it at
every build**, so a pin left stale by a bump breaks the entire workspace's
resolution. Measured on 2026-09-02: raising `windows-topology-sys` to 0.2.0
while this crate pinned `"0.1.0"` failed `cargo metadata` outright, and would
have landed a broken `main` the moment the release PR merged. Publishing would
have made that a permanent tax on a tool whose value is being re-run and
revised; not publishing removes the pins entirely, which is what was done.

**What is unchanged.** The tool still ships, still stamps its commit, and still
marks non-CI builds `!!UNOFFICIAL!!`. Nothing about the record's provenance
story depended on crates.io -- it only ever weakened it.

---

*Superseded reasoning follows, preserved for the argument it makes about
provenance, which is why the record still marks unofficial builds.*

**Decided: publish to crates.io, but not yet.** The reasoning is about which
path a runner meets first, not about whether the reach is worth having.

The product of this tool is not a binary, it is **data whose provenance can be
checked**. A binary attached to a GitHub release is traceable to the commit that
built it, because CI stamps that commit in and the download itself is the
evidence. A crates.io tarball carries no repository, so a `cargo install`ed
build finds no git metadata and reports its commit as unknown -- by
construction, not by oversight. Records from that path are strictly weaker, and
the tool marks them `!!UNOFFICIAL!!` accordingly.

That is the whole tension: `cargo install <name>` is the more discoverable
route, needs no release page, and is what a Rust developer reaches for first.
Publishing early would make the weakest submission path also the easiest one,
and submissions would drift to it before anyone noticed.

**Timing resolves it.** Publishing *after* the download path exists, is
documented in the README, and has been walked end to end by someone without this
repository means the strong path is the one people meet first, and crates.io
becomes the fallback it should be rather than the default.

**A hard prerequisite, not merely a preference.** Publishing this crate requires
its dependencies to be on crates.io: it depends on `windows-topology-sys` and
`windows-waitable-queues` by path, and a published crate cannot. As of this
writing `windows-topology-sys` 0.1.0 is published and `windows-waitable-queues`
is not, so the earliest possible publication is after that release. The
dependency pins also need to name whatever version is actually published --
during local development the `path` entry is used and the `version` entry is
never exercised, so a stale pin is invisible until the moment it matters.

**Rejected: baking the commit into the packaged source so a crates.io build can
name it.** It is achievable -- generate a file at package time and read it when
git is absent -- and it would answer a different question than the one the
marking exists to ask. "This source came from commit X" is not "this binary was
built by CI from commit X"; only the second makes the artifact independently
checkable, because only the second was produced by something other than the
person submitting the record. Blurring the two would leave the record's
"where this result came from" section saying something it cannot support. The
unknown commit is honest, and
honest is the point.

**What publication will oblige**, recorded so the cost is not rediscovered
later: the record schema becomes a semver surface the moment anyone stores one
(see the package metadata), and the README must say plainly that a crates.io
build produces records marked unofficial, so nobody chooses that path without
knowing what it costs the data.

## The crate version is a date, because semver has nothing here to describe

**`YYYY.MMDD.N`** -- the release date, plus a counter for a second release on
the same day. Leading zeros are illegal in a semver numeric identifier, so
September 2 is `2026.902.0` rather than `2026.0902.0`; that still orders
correctly within a year, since January 5 is `105` and December 25 is `1225`.

Semver would be the wrong scheme, not merely an unhelpful one. Semver's job is
to communicate **compatibility**, and this crate offers nothing for it to
describe:

- It is **never published to a registry**, so no dependency resolver ever reads
  the number.
- Nothing **depends on it as a library**. The package metadata already says the
  measurement code is an instrument rather than a surface to build on, and the
  workspace's own probes now depend on it by `path` with no version at all.
- The one thing that genuinely *is* a compatibility surface -- the record's
  shape -- **has its own version**. `SCHEMA_VERSION` is a linearly increasing
  integer with an append-only golden per version, deliberately independent of
  the crate's. That separation is what frees this field.

A version that describes no compatibility can only be arbitrary, and an
arbitrary version is bumped on a whim or not at all. Neither serves the reader.

**What a reader of a record actually needs from this field is which build
produced it.** The commit answers that exactly and is stamped beside it; the
date answers it *legibly*, telling someone at a glance whether a record in a
discussion thread is from last week or from last year. `0.1.0` answered
neither, and would have gone on answering neither indefinitely, because nothing
would ever have forced it to move.

**Nothing automated fights this.** Release-please does not manage this crate --
it is absent from both `release-please-config.json`'s `packages` map and
`.release-please-manifest.json`, and carries no `x-release-please-version`
marker -- so the version is maintained by hand, which is the only way a date
could be correct anyway. The release workflow's tag check needed no change: it
parses `${GITHUB_REF_NAME##*-v}`, which yields `2026.902.0` from
`placement-probe-v2026.902.0`, and still rejects a stale tag. Verified against
the workflow's own logic rather than assumed.

## The schema freezes at the first release, not before

**Decided: regenerate `schema/v1.txt` in place while the tool is unreleased, and
apply the append-only rule from the first release onward.**

The archived golden exists so that a shape change cannot happen silently, and the
append-only rule on top of it exists for a narrower reason: **a record already
held by someone else cannot be regenerated**, so once a record in the wild claims
schema N, N's meaning is fixed.

That rationale is about other people's data. Until the tool is released nobody
has any, so bumping the version records a shape that never reached a single
reader -- and the first public release then ships already carrying dead numbers,
each with an archived file describing a record that never existed.

This was learned by doing it. During development the schema was raised twice in
one evening, to v3 and then v4, each with a derived golden, on the reasoning that
the tool exists to emit records people paste and some had been produced locally.
Following that consistently would have released a crate whose first version
carried four schema files and three unreachable versions. It was collapsed back
to v1 in one change.

**The latitude ends at the release, and the boundary is deliberately sharp**
rather than a judgement call repeated per change:

- *Before the first release* -- regenerate `v1.txt` from the record. The golden
  still guards every change, because the test compares the file against a freshly
  serialized record either way; what is given up is only the archive of shapes
  nobody received.
- *After the first release* -- never edit a published golden. The next shape
  change raises `SCHEMA_VERSION` and adds the next file beside it.

**Rejected: keeping v1 through v4.** It is the safer-looking option and it is
strictly worse for a reader, who would find four archived shapes and no way to
tell that three of them were never emitted by any build anyone could obtain.

**Rejected: dropping the golden until release.** The guard is what makes an
accidental shape change visible, and that is as valuable during development as
after it -- more so, since that is when the shape actually moves.

## An unobserved cache relationship is a third answer, not a coin flip

`ProcessorPlace::cache_domain` is an `Observed<u32>`, and `Observed` derives `PartialEq`. Comparing two
of them with `==` therefore answers a question nobody asked: `NotObserved == NotObserved` is **true**, so
two processors that were merely both *missing* from the cache partition compare as sharing a cache, and
`NotObserved != Known(0)` is also true, so an unobserved domain against a known one compares as a cache
*crossing*. Either way an unknown is promoted into a finding, in a tool whose entire product is
measurements that other people are asked to trust.

Both directions were live. `classify` filed such pairs under `SameCache*` or `CrossCache*`, and
`within_class_pair` -- which selects the evidence for a `by_class` measurement that claims cache control
-- would choose two unobserved processors as a same-cache pair. The second is the worse of the two: it
does not merely mislabel a measurement, it causes the run to *make* one it cannot describe.

The rule now has one definition, `ProcessorPlace::shares_cache_domain_with`, returning `Option<bool>`;
`classify`, `within_class_pair` and `Slice::same_cache_domain` all ask it rather than restating it. That
last one already had the rule right and was the reason the defect was hard to see: the contract was
stated correctly in one place while two other sites re-implemented it with `==` and got it wrong. A
hand-written second copy of a contract rule is not a check of the contract, it is a check of the copy.

**`Absent` is deliberately not unknown.** It is the platform positively reporting that no cache level
partitions this machine, so two `Absent` processors really do share the single domain. Only
`NotObserved` -- "nothing asked, or no way to ask" -- poisons the comparison. The cheap fix for the
defect above is to treat every non-`Known` alike, which would silently discard a real answer on every
host without a partitioning cache level; `an_absent_cache_partition_is_shared_not_unknown` exists to
stop that.

**Labelled rather than dropped.** `Placement` gained `UnknownCacheSameClass` and
`UnknownCacheCrossClass` instead of the pairs being excluded, because the handoff really was timed
between two named processors and the number is real -- it is only the *cache* relationship that is
unknown, and the efficiency class is still carried. They sit after `CrossNumaNode` rather than beside
their `SameCache`/`CrossCache` counterparts because that ordering runs tightest-to-loosest *coupling*
and an unobserved relationship is not a point on that scale. This follows `CrossNumaNode`'s own
reasoning, quoted from its doc: the merge is silent, and the run that would expose it is the expensive
one.

No schema version bump: the freeze starts at the first release and this crate has not had one.

Raised in the PR #56 review, as two suppressed comments on the same defect.

## The measurement is not redactable; the context is, and is withheld by default

A record holds two kinds of thing, and they earn opposite defaults.

The **measurement** -- the topology, the placements, the timings -- is the reason
the record exists. Redacting it leaves a file that says nothing, and it is also
the part that most identifies unusual hardware, so no switch will ever withhold
it. The README says that plainly rather than implying a redaction story that
does not exist: an unreleased part is identified by an unusual core count and a
novel cache arrangement at least as well as by its name.

The **context** -- the minute the run finished, the CPU model, the OS build, the
virtualisation hint -- explains a measurement without being one. All of it is
now withheld unless the runner passes `--include-metadata`.

**The default flipped, and that is the whole change.** The tool began by
collecting the context and offering `--no-cpu-model` as the single escape hatch,
which asks a stranger doing a favour to recognise in advance which field they
would rather not send. Defaulting to redacted asks nothing of them.

`--no-cpu-model` survives as a *subtraction* from `--include-metadata`, because
the case it was built for is real and is not covered by the general opt-in: a
runner willing to send an OS build and a hypervisor name may still be sitting in
front of a part whose name is not theirs to publish. Passed on its own it
withholds something already withheld, which is redundant rather than an error and
is tested to stay harmless -- a cautious runner who passes both flags must get
the same record as one who passes neither.

**One policy, decided once.** `MetadataPolicy` is built from the flags in
`Options::metadata_policy` and then handed to `MachineDescription::read` and
`SubmissionRecord::new`. Every consumer -- the disclosure notice, the machine
read, the record, the report -- reads that one answer instead of re-deriving it
from the flags, so the notice cannot describe a policy the record does not
implement.

**A withheld field is not read at all**, rather than read and then dropped. The
registry call does not happen, so the module's commitment about what it does not
touch is kept by control flow rather than by a discard a later refactor could
lose. The notice therefore shows a withheld row as withheld instead of previewing
a value that will not be sent; there is nothing for a runner to judge in a value
nobody is asking for.

**Suppression is recorded, never merely absent.** "The runner did not send this"
and "the host would not answer" are different facts, and a collector that cannot
tell them apart will eventually read one as the other. The mechanism differs by
field only because the types do:

- `cpu_model` and `os_build` are `Option<String>` beside a `*_suppressed` flag,
  following the pattern `model_suppressed` already established.
- `virtualisation` gained a `Suppressed` **variant** rather than a flag, because
  every other variant is a claim about what was observed and a withheld hint has
  no honest value to fall back on. `NotDetected` would assert a negative finding
  nobody made -- on the field that decides whether a submission could ever have
  shown NUMA rows -- and `Unknown` would blame the firmware. Carrying it in the
  enum also states the fact once, where a variant plus a boolean could disagree.
- `recorded_at` is `Option<String>` beside `recorded_at_suppressed`, which is
  **redundant by construction today**: a clock cannot decline to answer the way a
  registry key can, so the timestamp is absent only when withheld. It is carried
  anyway, because that is a fact about the implementation and a collector should
  not have to know it -- every other withheld field says so in the data, and a
  hand-assembled record (every field is public) can drop the timestamp without
  meaning to claim anything.

**The file name loses the stamp too.** `submission::file_name` is derived from
the record, so a record with no timestamp yields `placement-probe-v1-250.json`.
Reaching past the record for the clock would put the withheld minute back into a
file name the runner may well attach, which is the one place it could still
escape. Nothing is lost but convenience: the collision *guarantee* was always the
exclusive create and the numbered suffix in the writer, and the milliseconds that
keep that suffix from being needed in practice are still there.

**This supersedes nothing about the minute-flooring**, which still applies to the
value that survives an opt-in. Flooring answers "how precise may a timestamp we
do send be"; this answers "do we send one at all".

No `SCHEMA_VERSION` bump: the freeze starts at the first release and this crate
has not had one. `schema/v1.txt` was regenerated in place, per the decision
above.

**Every guard above is sabotage-verified**, in
[sabotage.json](sabotage.json): six defects -- a withheld hint falling back to
the enum default, each of the two registry reads happening regardless of policy,
a record stamping a timestamp regardless of policy, a file name carrying a stamp
the record withheld, and `--no-cpu-model` ceasing to subtract -- and all six turn
the suite red. The mirror-image claim, that an *opted-in* field is present, is
deliberately not sabotaged there, because it depends on what the host will
answer.

Engineer's decision, 2026-09-04. Queued as `M36.2` in
[CHECKLIST-placement-tool.md](../../CHECKLIST-placement-tool.md); `M36.3` states
in the README what redaction costs, and `M36.4` asks for an unredacted record
privately when the topology's sources disagreed -- the one case where the context
matters most.

## A disagreement is reported where it happens, and the ask attached to it is an offer

`MachineMemoryTopology::discover` reads two independent Win32 sources and
compares which processors they name. When they never agree within the retry
bound, `Coherence::Disagreed` is the *conclusion* that the difference is real
rather than a moment caught mid-change. Nothing in this workspace looked at it.

**The record now carries the whole `Coherence`**, `walk_only` and
`cpu_sets_only` lists included, not a boolean summary. A record saying only
"something disagreed" cannot be investigated, and the report attached to it asks
a runner whether they would help investigate -- so a lossy field would make that
ask hollow. Whatever a maintainer would need to look at, the record they are
offered has to contain.

**It is a field of the record, not of the `Fingerprint`.** Fingerprints are
compared for equality to catch a record spliced from two machines, and coherence
is not a fact about a machine's *shape*: an announced reading that agreed and a
measured one that did not would trip that check and throw away a good
measurement over a difference in no shape at all. Provenance flows through the
fingerprint because it *is* a property of the reading the shape came from;
coherence rides beside it instead, from the same reading.

### Informative, not coercive -- and that is a tested property

This is the only part of the report that asks the reader for anything, and the
wording is a requirement rather than a matter of taste. The person running this
is already doing the project a favour on hardware nobody here can buy, and the
result they are holding is a valid submission whether or not they do anything
else. A section that reads as a demand costs exactly the submission it was
trying to improve.

So the text states what was detected, says plainly that the measurements are
unaffected, names **both** possible causes -- the platform's description of this
hardware may be inconsistent, or this tool may be reading it wrongly -- and notes
that neither can be identified from the runner's machine. Then it offers a way
to help and closes by releasing the reader: "None of that is required."

Naming this tool's own possible defect first-class matters. An ask that implied
the platform must be at fault would be asking the runner to help confirm a
conclusion rather than to help reach one, which is both discourteous and wrong:
this tool has been the defective party before.

`the_disagreement_section_informs_rather_than_pressures` asserts the release is
present, that both causes are offered as undecided, and that no pressure word
("please", "you should", "we need", "make sure") appears. Tone cannot be pinned
completely by a test; these are the parts of it that can be, and they are the
parts an ordinary edit would lose.

### The quiet path stays quiet

`Agreed` and `NotCollected` print nothing at all. A line reporting that the two
sources agreed would appear on every run that ever happens, and a reader learns
to skip whatever is always there -- including on the one run where this section
is the most interesting thing in the file.

### The advice matches what the reader is already holding

The ask is for a record naming the OS build and hypervisor, which after M36.2
means one run with `--include-metadata`. When the record already carries those,
the text says so instead of advising a flag whose output the reader has in hand:
that advice reads as though the flag had not worked, the same failure the
collection notice avoids with `--no-cpu-model`.

**"Privately" is an offer to arrange, not a channel that exists.** Discussions
and issues are public, so the text asks the runner to make contact there and
says the maintainers can arrange a way to share the file that does not post it
publicly. Promising a private channel the project does not have would be a
worse defect than asking for nothing.

The link is the repository rather than the results thread: a disagreement
between the platform's own tables is not a result, and posting it into the
collection thread would bury it among measurements. That is a second URL
constant, because `report` is not gated behind `serde` and `DISCUSSION_URL` is;
a test pins that the two agree about the repository, so the pair cannot drift
into sending a runner to a dead link.

### What this deliberately does not do

**`is_fully_traceable` is untouched.** A disagreement is not a doubt about
provenance -- the build is official and the topology really was read from this
machine -- so the "where this result came from" section still says so, and the
disagreement is reported in its own section below. Whether an inconsistent
machine should additionally be *marked* there, and how to mark it without
dressing up a nuisance as a prize, is `PT-7.1`'s decision and is left to it.

**The printed text gives counts, not processor identities.** The record names
them individually and says so. Naming them in the report is the rest of
`PT-7.1`, which wants what each source claimed; counts are what M36.4 needs to
make "a mismatch" a concrete thing rather than a word.

Engineer's decision on the wording, 2026-09-04. Queued as `M36.4` in
[CHECKLIST-placement-tool.md](../../CHECKLIST-placement-tool.md), completing M36.
Three of the nine sabotages in [sabotage.json](sabotage.json) cover this section:
printing it on every run, dropping its closing release, and advising
`--include-metadata` to a record that already carries it.
