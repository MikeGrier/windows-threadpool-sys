# Checklist: a shareable placement-cost tool

**Goal.** A small, publishable Windows tool that a stranger can install, run once, and send back a
single structured result -- so that this workspace can collect placement and NUMA-hop measurements
from hardware it does not own. **The motivating gap is concrete: every host available here has exactly
one NUMA node**, so the entire `cross NUMA node` row and the whole inter-node hop matrix are
unmeasured, and no amount of local work will change that.

**The gate applies to crates.io publication only, and not to the GitHub binaries.** An earlier revision
of this paragraph gated the whole file on SH-4.1 and SH-4.3, which was wrong and would have delayed the
tool by the length of the entire release sequence -- including M6's stress work -- for no reason.

- **CI-built binaries are compiled from this repository**, so the tool's dependencies resolve through
  `path` and nothing has to exist on crates.io. **PT-5.1 is therefore not gated at all**, and it is the
  distribution that matters: the download is the provenance, per PT-3.2.
- **GATED BY [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md) SH-4.1
  (topology 0.2.0) and SH-4.3 (queues 0.1.0): PT-5.3 only**, publishing the tool to crates.io, where a
  path dependency needs a real published version behind it. **This bullet is the gate of record; when
  those land, edit it to say so and name the two versions.** A gate that has silently lifted is as
  harmful as one that has not.

M1B and M6 are outside all of this and say so where they are defined.

**Gated on shipping [crates/windows-topology-sys](crates/windows-topology-sys) and
[crates/windows-waitable-queues](crates/windows-waitable-queues) first.** Not a preference: the tool
depends on the former, and calibrates against the latter's `spsc`. Both are `0.1.0` and the topology
crate now carries an unreleased breaking change (`feat(topology)!`), so it wants a release before
anything downstream is published against it.

**Why a new crate rather than publishing the existing probes.**
[crates/windows-platform-probes](crates/windows-platform-probes) is `publish = false`, `version =
0.0.0`, and every binary opens by saying it is "an experiment, not a component". That boundary is
deliberate and stays. It also carries ~13 probes irrelevant to this question, which would be public
surface and a maintenance obligation for no benefit.

Related: [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.4 and M-inf.5, both of which are
waiting on numbers only other people's machines can produce.

**M1-M4 and M36 are complete and archived** in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md)
under `Moved 2026-09-09 21:34:00 -04:00`. What remains below is distribution and the two
measurement questions after it.

## M5: distribution

**The CI-built artifact is the canonical way to get this tool**, not `cargo install`. Two reasons, and
the second is the real one: a downloader needs no Rust toolchain, and **the download itself is the
provenance**. A binary attached to a release in this repository is traceable to the commit that built
it, in a way a locally built copy of the same source is not -- which is what makes PT-3.5's "official
build" distinction meaningful rather than decorative.

- [x] **PT-5.1** -- CI builds the tool on tag and attaches the binary to a GitHub release, injecting
  the commit into the environment variable PT-3.5 reads. **Verify the negative case**: a locally built
  binary must produce a record marked as an unofficial build, and a CI-built one must not. A
  distinction nobody has watched fail is a distinction that does not work.
  **Done, and both directions are checked inside the workflow itself rather than trusted.** It builds
  with the stamps and asserts the artifact reports itself official *and names this commit*; then
  rebuilds **without** them and asserts that binary marks itself unofficial; then rebuilds with the
  stamps for release, because the negative check overwrote the artifact and shipping that file would
  attach an `!!UNOFFICIAL!!` binary to an official release.
  A `--version` flag was added for this, printing the whole build identity rather than a version
  number -- CI asserts on it, and a downloader can check the same thing before trusting a binary.
  **`aarch64-pc-windows-msvc` is in the matrix and cannot be verified locally.** The cross-build fails
  on this machine with `unresolved external symbol __imp_GetProcessHeap`, which is a missing local
  ARM64 MSVC library rather than a code fault -- `std` itself uses that symbol, so a real defect would
  break every ARM64 Rust program.
  **The pull request verifies it, which is better than the dispatch this originally called for.** The
  workflow now also triggers on a pull request touching the tool, building and verifying both targets
  without releasing. Two things made that the right answer rather than a convenience:
  - **Nothing else in this repository builds the ARM64 target.** [ci.yml](.github/workflows/ci.yml) cross-compiles only
    `thumbv7em` for `wtf-string`, so without this a tag would be the first time `aarch64` was ever
    attempted -- turning a build failure into a broken release.
  - **`workflow_dispatch` could not have done it.** GitHub only offers dispatch for workflows already
    on the **default branch**, so a workflow still on a feature branch cannot be dispatched at all --
    which is precisely when it needs verifying. The original instruction here was unusable.
  The release job stays guarded on the tag ref, so a pull request publishes nothing however it runs.

- [x] **PT-5.2** -- A README written for someone who has never seen this repository: what question the
  tool answers, why their machine is interesting, where to download it, how to run it, what to send
  back, and what it collects. Assume no context and no obligation. Lead with the download, not with
  `cargo install`.

- [x] **PT-5.3** -- Decide whether to publish to crates.io **as well**, and record the reasoning. It
  costs a semver obligation and yields records whose commit is *unknown* by construction (a crates.io
  tarball carries no repository), which is a strictly weaker submission. The case for it is reach; the
  case against is that the weaker path is also the more discoverable one, and submissions will drift
  towards it.
  **Decided: yes, publish -- but not yet.** Timing is what answers the objection. Publishing *after*
  the download path exists, is documented, and has been walked end to end means the strong path is the
  one a runner meets first, and crates.io becomes the fallback it should be rather than the default.
  Reasoning recorded in
  [DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md), including the rejected
  alternative of baking the commit into the packaged source -- which would let a crates.io build name
  a commit while still not showing that CI built it, and so would have the record's trust section
  claim something it cannot support.
  The publication itself is **PT-5.6** below; it is not part of this item, which was only ever a
  decision.
  **REVERSED 2026-09-02: decided no, never publish to crates.io.** The GitHub release binary is the
  only distribution, and `publish = false` is now permanent. Two reasons, the second of which was not
  known when the above was written:
  1. **The reach premise was backwards.** A released binary needs no Rust toolchain; `cargo install`
     needs a toolchain, a compiler, and a build of the whole dependency tree. crates.io therefore
     reaches a *subset* of the download path's audience -- a convenience for Rust developers, bought
     by making the weakest-provenance path the most discoverable. M5's own preamble had already said
     the download "needs no Rust toolchain" and is "the provenance".
  2. **A published crate cannot use bare `path` dependencies, and cargo enforces the resulting
     `version` pins at every build rather than at publication.** So a pin left stale by any workspace
     bump breaks the entire workspace's resolution. Measured: topology at 0.2.0 against this crate's
     `"0.1.0"` pin failed `cargo metadata` outright. Publishing would have made that a permanent tax;
     not publishing let both pins be deleted.
  Recorded in [DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md), which keeps the
  superseded reasoning because its provenance argument is still why the record marks unofficial
  builds.

- [x] **PT-5.4** -- Package metadata and a statement of what is and is not covered by semver. The
  **record's schema is a compatibility surface** the moment anyone stores one; the internal measurement
  code is not.

- [ ] **PT-5.5** -- Walk the whole path end to end on a machine without this repository checked out:
  download, run, find the record, read the README's instructions for sending it. A path nobody has
  walked is a path that does not work, and the person walking it will be doing a favour rather than
  debugging.
  **Deliberately left open: this cannot be completed from here.** It needs a real release to download
  from and a machine without this checkout, and doing it against a local build would test something
  else while looking like it had passed. The ARM64 development machine is the obvious first walker,
  and it doubles as the check that the unverified `aarch64` artifact from PT-5.1 actually runs.

## M5+: crates.io -- WITHDRAWN, never to be published

**PT-5.3's decision to publish here was reversed on 2026-09-02; this milestone will not be pulled in
and numbered.** Kept as a heading rather than deleted, so that a reader who remembers a plan to
publish finds the reversal instead of a gap. The reasoning is on PT-5.3 above and in
[DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md): the reach premise was backwards,
and a published crate would have owed permanent dependency-pin maintenance that cargo enforces at
every build rather than at publication.

- [x] **PT-5.6** -- **WITHDRAWN: `windows-placement-probe` is never published to crates.io.** Checked
  off as *decided against*, not as done. Its cross-component prerequisites on `SH-4.1` and `SH-4.3`
  are void, and the reciprocal note in
  [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md) has been updated to
  say so -- a prerequisite that outlives the item needing it is how work gets blocked on nothing.
  Two of its three "must not skip" points are void with it: the dependency pins were **deleted**
  rather than corrected (the crate is path-only now), and there is no `cargo install`ed copy to run.
  The third survives on its own merit and is **not** lost: the README should still say that a
  locally built copy produces records marked unofficial, because a runner can still build one from
  source. That is **PT-5.7** below rather than a bullet inside a withdrawn item.

- [ ] **PT-5.7** -- **Say in the README what a locally built copy costs the data.** Rescued from
  PT-5.6, whose withdrawal would otherwise have taken it. The point never depended on crates.io: a
  runner who clones and `cargo build`s gets a binary that marks its records `!!UNOFFICIAL!!` with no
  commit, exactly as a `cargo install`ed one would have. They should learn that from the README
  rather than from their own output, and it is the negative case that makes PT-3.5's "official build"
  distinction mean something to a reader rather than only to CI.
## M6: is a set of "equivalent" processors actually equivalent?

- [ ] **PT-6.1** -- **Give the fingerprint a placement signature, or keep saying it is not canonical.**
  [fingerprint.rs](crates/windows-placement-probe/src/fingerprint.rs) records each partition as a list
  of *sizes* -- processors per cache domain, per efficiency class, per NUMA node -- and never how
  those partitions intersect. Two eight-processor hosts can both render
  `L2[4,4] ec[0:4,1:4] numa[4,4]` while one puts each efficiency class in its own cache domain and the
  other splits both classes across both; only the second can express a same-cache/cross-class pair.
  **The placements available to a run differ while the fingerprint agrees**, so string equality is not
  placement equivalence.
  The claim has been corrected in place, so nothing is currently wrong -- this item is the stronger
  fix, not a bug. It needs a canonical signature of the expressible placements *in* the string, which
  means a serialized field and therefore a schema bump.
  **Deliberately gated on some other reason to bump the schema**, because a summary line is not worth
  a version of its own when every measurement row already names the placement it was taken at, which
  is what a collector needing equivalence should read. Raised by review 5073245942 on pull request
  #56.

- [ ] **PT-6.2** -- **Give the NUMA list an absence marker, so an unreported node set is not read as
  one node.** [fingerprint.rs](crates/windows-placement-probe/src/fingerprint.rs) renders
  `numa_node_sizes` as the nodes the topology *reported*, so a host naming no memory domains renders
  `numa[]` while every processor is still counted and every placement still reports node `0` -- the
  documented single-node default. The node list therefore does not sum to `processors` in that one
  case.
  The asymmetry with the cache list is deliberate, not an oversight: `cache_domain_sizes` fills itself
  with the processor count when no level partitions the host, but it can afford to, because it renders
  `L-` for "no partitioning level" and so `L-[16]` cannot be mistaken for a real single-domain level.
  The NUMA list has no such marker, so `numa[16]` would be indistinguishable from a host that genuinely
  reported one node of 16, and the more useful fact -- that the machine said nothing about NUMA --
  would be lost.
  The behaviour is documented on the field and pinned by
  `a_bare_topology_renders_its_processors_but_claims_no_numa_nodes`, so nothing is currently wrong;
  this item is the stronger fix. A marker is a serialized-field change and therefore a schema bump,
  so like PT-6.1 it is **deliberately gated on some other reason to bump the schema**. Found while
  fixing the processor count raised by review on pull request #56.

**Not gated on the release, unlike the rest of this file.** The work is an extension of the affinity
measurement, which today lives in [crates/windows-platform-probes](crates/windows-platform-probes) and
moves wholesale under PT-2.1. Build it there now; it travels with everything else.

**The assumption under test.** Several designs in this workspace treat a *set* of processors as
interchangeable -- any processor in this cache domain, any processor in this NUMA node -- and place
threads by domain rather than by processor. [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.5
rests on exactly that. **Every measurement taken so far pins to a single processor** (`mask = 1 << cpu`),
so the assumption has never been tested; it has only been assumed while being carefully avoided.

**There is a structural reason to doubt it, before any scheduling subtlety.** A set mask permits
placements a single-processor mask forbids -- including **both threads on one logical processor**,
which turns an SPSC handoff from concurrency into time-slicing, with the spin-wait burning its quantum
before the peer can run. On an SMT host the `same cache domain` set *is* the two siblings of one core,
so this is not a corner case there, it is the common one.

- [ ] **M6.1** -- Derive each processor's **equivalence set** from the topology -- SMT siblings, cache
  domain, NUMA node, efficiency class -- and pin down which sets a given host can express, the same way
  placements already are. A set with one member is not a test of anything and must be reported as
  inexpressible rather than measured.

- [ ] **M6.2** -- Add **affinity mode** as a dimension beside placement and strategy: `Pinned` (today's
  single bit) and `SetWide` (each thread masked to *its own* equivalence set, which preserves the
  placement relation while relaxing the choice within it). For `CrossCacheSameClass` that means the
  producer may use any processor of its cache domain and the consumer any of its own; for
  `SameCacheSameClass` both threads share one set, which is where co-residency becomes possible.
  **In `SetWide` the placement label states intent, not outcome** -- the scheduler may do something
  else entirely, and saying otherwise would be the "asserts its conclusion" defect again.

- [ ] **M6.3** -- Measure the **mechanism**, not only the elapsed time, or the result cannot be read.
  Sample `GetCurrentProcessorNumber` in both loops and report **migration count** (did the thread move
  at all?) and **co-residency fraction** (how often were producer and consumer on the same processor?).
  Co-residency is the killer observable and can only be non-zero in `SetWide`.
  Without these, "the two modes matched" is indistinguishable from "the scheduler never moved
  anything", which is precisely the false-equivalence this milestone exists to rule out -- and is the
  same trap the peer-index probe's read counters were added to escape.

- [ ] **M6.4** -- Run **long enough for the scheduler to act**. The present 2M items is roughly 40 ms
  on an idle host, over which nothing migrates and both modes will look identical for want of any
  reason to differ. Choose the duration from measured migration counts -- long enough that migrations
  are actually observed under load -- rather than from a round number, and record the reasoning.

- [ ] **M6.5** -- **Interference pass one: competing spinners confined to the same equivalence set.**
  Adversarial and controlled: it forces the scheduler to choose *within* the class, which is the
  precise claim under test. Vary the number of competitors relative to set size, since one spinner in a
  four-processor set is a different question from four. Keep it reproducible -- an interference model
  that varies run to run turns every comparison into noise.

- [ ] **M6.6** -- **Interference pass two: a concurrent copy of the real workload.** A second
  producer/consumer pair on the same set, which is what a domain runtime actually looks like when more
  than one queue is live. Pass one establishes whether the scheduler *can* break the equivalence; this
  establishes whether it *does* under load anyone would really generate. **Report both**: a difference
  that appears only under adversarial spinners is a real finding with a narrower consequence, and
  collapsing the two would lose exactly that distinction.

- [ ] **M6.7** -- **FEEDS [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.5, whose premise
  this tests. On completing this, edit M-inf.5 with the answer** -- it does not check M-inf.5 off (that
  item is the domain-local placement work itself), but M-inf.5's 5.6x is a number about *pinned*
  threads until this says otherwise, and leaving that unstated is how a measured caveat quietly becomes
  an assumed fact.
  Report per set kind whether the equivalence holds, in the tool's own words, derived
  from the measurement rather than asserted. **A null result is
  a real result here** -- "the sets behaved equivalently under both interference models, and here are
  the migration counts showing the scheduler was genuinely exercised" retires a long-standing doubt,
  and is worth as much as a difference would be.

## M7: report what Windows contradicts about itself

Opened by [D-17](crates/windows-topology-sys/DESIGN-NOTES.md) in the topology crate, which establishes
that two Win32 topology sources can be **stably** inconsistent -- and that this is expected on hardware
we do not have and on prerelease firmware, rather than being exotic.

The division of labour is deliberate and follows the same facts-versus-policy line the rest of this
workspace uses. **`windows-topology-sys` records the disagreement**; it does not report it, because a
crate that states facts should not be in the business of producing bug reports. **This tool reports
it**, because reporting is what this tool is for, and because the provenance that makes such a report
actionable is identifying and therefore belongs behind the review this tool already applies.

- [ ] **PT-7.1** -- **Surface the topology's recorded inconsistencies**, in the tool's output and in
  the submission record. This is the only place they become visible to anyone: the topology crate keeps
  what each source said, and nothing in the workspace currently looks at it.
  **Partly done by `M36.4` on 2026-09-04, and what remains is narrower than the text below.** The
  **record** half is complete: `topology_coherence` carries the whole `Coherence`, so a submission
  names the disagreeing processors individually. The **output** half is partly done: the report has
  a section that appears only on `Disagreed`, gives the counts on each side and the retry number,
  and says the measurements are unaffected. Two things are still open, and both are decisions rather
  than plumbing:
  (a) **Name the processors in the printed text**, not only in the record -- "what each source
  claimed", which is what this item asks for and what counts alone do not give.
  (b) **Decide whether an inconsistent run is marked in "where this result came from".**
  `is_fully_traceable` is deliberately untouched, so that section currently reads "an official build,
  reading this machine's real topology" directly above the disagreement section. That is not a
  contradiction -- the build *is* official and the topology *was* read -- but a reader may feel one.
  This item's own two-sided framing below is the guidance for settling it: mark it plainly enough
  that neither the runner nor a later reader is left guessing, without dressing up a nuisance as a
  prize.
  Report what disagreed and what each source claimed, not merely that something did -- "incoherent" is
  not actionable, and the point of collecting from strangers' machines is to learn something specific
  about hardware nobody here can buy.
  An inconsistent machine is **still a valid submission**, and should be marked rather than rejected.
  **Its value is genuinely two-sided, and the tool should not pretend otherwise.** In the long run it
  is the more valuable submission -- evidence of something no local run can produce, and potentially a
  bug report against a firmware table. In the short run it is an **annoyance**: a run whose numbers a
  reader must qualify, from a machine whose description cannot be taken at face value.
  So mark it plainly enough that a runner is not left wondering whether their machine is broken or
  their run is wasted, and plainly enough that a reader of the submission knows which parts to trust --
  without dressing up a nuisance as a prize.

- [ ] **PT-7.2** -- **Add the firmware provenance an inconsistency report needs to be actionable** --
  mainboard and BIOS version at minimum -- suppressible by the runner, with the suppression recorded
  rather than merely absent, exactly as `MachineDescription`'s model handling already does.
  **Weigh it against the existing honesty about what suppression buys.** This checklist already
  establishes that the flag "must not be oversold", that a pre-release part "is identified at least as
  well by its topology", and that the tool "cannot make an NDA-covered machine safe to submit from, and
  must not imply that it can". Firmware provenance sits under that same caveat and arguably deepens it:
  a BIOS version can pin a specific board revision more precisely than a CPU model names a part.
  So the honest framing is unchanged rather than weakened -- if the hardware is confidential, the right
  answer remains not to send it -- but the README's list of what is collected must grow to match, per
  `PT-4.3`, and the runner must still see the real values before deciding, per `PT-4.5`.
