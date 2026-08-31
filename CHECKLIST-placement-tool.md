# Checklist: a shareable placement-cost tool

**Goal.** A small, publishable Windows tool that a stranger can install, run once, and send back a
single structured result -- so that this workspace can collect placement and NUMA-hop measurements
from hardware it does not own. **The motivating gap is concrete: every host available here has exactly
one NUMA node**, so the entire `cross NUMA node` row and the whole inter-node hop matrix are
unmeasured, and no amount of local work will change that.

**GATED BY [CHECKLIST-ship-topology-and-queues.md](CHECKLIST-ship-topology-and-queues.md) SH-4.1
(topology 0.2.0) and SH-4.3 (queues 0.1.0). This paragraph is the gate of record: when those land,
edit it to say the gate is lifted and name the two published versions.** Leaving it as-is after the
releases is the failure mode -- a reader arriving here should never have to reconstruct whether the
gate still applies. M6 below is deliberately *outside* this gate and says so.

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

## M1: decisions that shape everything after

- [ ] **PT-1.1** -- **Name the crate**, and record the reasoning. It measures what a producer/consumer
  handoff costs as a function of where the two threads run, which is broader than queues and narrower
  than "topology". Candidates to weigh rather than a foregone answer: `windows-placement-probe`,
  `windows-handoff-cost`, `windows-locality-report`. Check availability on crates.io before settling.

- [x] **PT-1.2** -- **Decide what the submission record carries about the machine beyond the
  fingerprint**, specifically the CPU model name. The fingerprint deliberately omits model names
  because "a fingerprint that changes when the answer does not is a fingerprint nobody can compare" --
  correct for comparing placements, and a real loss when a stranger sends a result you cannot ask
  follow-up questions about.
  **This had to be settled before the first submission arrives, because the asymmetry is brutal.** A
  record cannot be regenerated: a field the tool did not collect is missing *permanently* from every
  result gathered before the omission was noticed, and the machines are other people's.
  Under-collecting is unrecoverable; over-collecting is a privacy cost that can at least be corrected
  going forward by collecting less.

  **Decided: collect the CPU model, the OS build, and a virtualisation hint. The canonical fingerprint
  string stays clean; all three live in the record beside it.**

  The reasoning, in the order it actually holds:
  - **A CPU model is not personal data.** It is a hardware characteristic shared by millions of
    machines. The things that would be sensitive -- hostname, user name, file paths, domain membership,
    serial numbers, installed software -- are not collected and must not be. That is the primary
    argument; it stands whether or not the model could be inferred.
  - **Withholding it gains nothing anyway**, because a detailed topology plus cache geometry narrows
    the field to a small class of parts. This is the supporting argument, and it is deliberately *not*
    treated as a principle: "it could be inferred, so collect it" would justify almost anything, and
    the test remains whether the field is sensitive on its own merits.

  **Two fields ride along by the same reasoning, and both are more explanatory for this dataset than
  the model is:**
  - **The OS build.** Placement cost is a scheduler behaviour, and the scheduler changes between
    Windows builds. Two results that disagree are otherwise indistinguishable from two builds
    disagreeing, and that is unrecoverable after the fact.
  - **A virtualisation hint.** This workspace has already established that **VM slices flatten
    topology** -- the EPYC slice reports one L3 domain and one NUMA node for silicon that has eight and
    two -- which is precisely why the interesting rows are unmeasured here. Being able to separate bare
    metal from VM submissions is therefore not incidental: it is the distinction that decides whether a
    submission can supply the missing rows at all. **Record it as a hint and label it as one**;
    hypervisor detection is not reliably decidable from user mode, and a field that overstates its
    confidence is worse than an absent one.

  **The runner can suppress the model**, with a flag, and the tool says so where it lists what it
  collects. Not because the field is sensitive in general, but because the one case where it might be
  is real and narrow -- an engineering sample or unreleased part would leak a name that is not yet
  public -- and because "here is what I collect, and you may turn this off" is a materially stronger
  thing to say to someone doing a favour than "trust me". The field is optional in the record, so a
  suppressed submission stays valid rather than becoming unparseable.
  **Suppression is recorded, not merely absent.** A field that is missing because the runner withheld
  it and a field that is missing because the host would not answer are different facts, and a
  collector that cannot tell them apart will eventually read one as the other -- the same reason an
  inexpressible placement is reported rather than skipped.

  **And the flag must not be oversold, which is the more important half.** For the engineering-sample
  case it addresses the *smaller* leak. A pre-release part is identified at least as well by its
  **topology** -- an unusual core count, a novel cache arrangement, an unreleased NUMA layout -- and
  the topology is the entire point of the submission, so it cannot be suppressed without making the
  record worthless. **The tool therefore cannot make an NDA-covered machine safe to submit from, and
  must not imply that it can.** Say so plainly in the README: if the hardware is confidential, the
  whole output describes it, and the right answer is not to send it. That is worth more to the
  audience most likely to own a multi-socket machine than any reassurance would be.

- [x] **PT-1.3** -- **Decide the fate of the three existing probe binaries** (`probe-topology`,
  `probe-core-affinity`, `probe-peer-index-cache`) once their modules move. Keeping them as thin
  wrappers preserves the internal workflow; deleting them removes a second way to run the same
  measurement and a second place for output to drift. **Do not decide by taste -- the risk being
  weighed is two renderings of one measurement disagreeing**, which this investigation has already hit
  three times.
  **Decided: keep them, and move the *rendering* into the library so there is only one of it.** The
  two stated worries turn out not to be in tension, because they are about different things. The
  engineer's -- that a combined binary accretes flags and modes until it is the grab-bag this crate was
  extracted from -- is about **entry points**. The drift worry is about **renderings**. Sharing the
  render code kills the drift risk outright, after which extra entry points cost nothing.
  So: the shared tool is **one binary, one run, one record**, because a stranger doing a favour must
  not be asked to run three things and collate them. The internal probes stay **separate and thin**,
  because running one measurement in isolation is the whole point of a development loop. Every binary
  becomes an entry point only; measurement *and* rendering live in the library and are called, never
  reimplemented. A binary that formats its own output is the defect, not a binary that exists.

## M1B: processor groups, before a large machine is ever offered

**This is the blocker that would waste the opportunity.** A large multi-socket host -- the kind that is
the entire point of this tool -- has more than 64 logical processors, so Windows presents it as
**multiple processor groups**, each numbering from zero.

- [ ] **PT-1B.1** -- **Carry `(group, number)` as a processor's identity.** `ProcessorPlace` keys on a
  bare `u8` number and `places_from_topology` discards the group outright (`for (_group, number)`), so
  every group's processor 5 collides on one map key. **The result is not a crash.** Numbers stay below
  64 within a group, so `assert!(cpu < 64)` never fires: the tool runs, pins to whichever processor
  won the collision, and prints a confident placement table describing a topology it silently
  collapsed. That is the same defect class as the omitted SMT row, on the machine we would get one
  attempt at.

- [ ] **PT-1B.2** -- **Pin with `SetThreadGroupAffinity`.** `SetThreadAffinityMask` takes a mask
  within the caller's current group and cannot express a processor in another one, so it is not a
  matter of widening the mask. Keep the existing failure discipline: pinning that does not land must
  abort the run rather than fall back to an unpinned measurement.

- [ ] **PT-1B.3** -- **Verify against a synthetic multi-group topology**, since no host here has more
  than one group. `places_from_topology` is a pure conversion and already testable; a fixture with two
  groups whose numbers overlap must produce distinct processors, and the sabotage is to key on the
  number alone and watch the count halve.

- [ ] **PT-1B.4** -- **Refuse loudly if groups are present and unsupported.** Whatever remains
  unimplemented when a large machine is offered, the tool must say so and stop. A refusal costs one
  message; a collapsed topology costs a wrong answer nobody can detect from the output, on hardware
  that is not coming back.

## M2: the move

- [ ] **PT-2.1** -- Move `fingerprint`, `core_affinity` and `peer_index_cache` into the new crate, and
  make `windows-platform-probes` depend on it. This inverts today's direction deliberately: the
  published crate owns the measurement, the internal grab-bag borrows it. A **pure relocation** with
  the provenance trail the repository requires for a split -- commit trailers and per-file headers --
  because these modules carry a session's worth of hard-won reasoning in their comments and blame must
  survive.

- [ ] **PT-2.2** -- Keep `queue_contention` and every unrelated probe where they are. The new crate is
  not a home for "measurement code in general"; it is one tool with one question, and admitting a
  second unrelated probe is how it becomes the grab-bag it was extracted from.

- [ ] **PT-2.3** -- Verify the move changed no behaviour: the three probe binaries (or their
  replacements per PT-1.3) produce the same numbers on this host as recorded in
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.4, and the full sabotage set still fails
  where it should.

## M3: the submission record

Ordered so each item's prerequisites land first: the two identity fields are decided before the record
that carries them is written.

- [ ] **PT-3.1** -- **A linearly increasing integer schema version that cannot silently drift, guarded
  by an archived schema rather than a hash.** The counter itself is easy for a consumer to compare
  (`schema >= 2`); the hazard is forgetting to bump it when the record's shape changes, which no amount
  of care reliably prevents. So derive rather than restate, per this repository's own rule -- but
  derive into something that survives.
  **A hash was considered first and rejected, because it does not survive its own history.** With a
  table of `version -> hash`, only the *current* version's hash can ever be recomputed; every earlier
  row is a frozen constant nobody can verify. The hash function then becomes an unversioned contract --
  change the traversal, the digest, or how key paths are canonicalised, and every historical row
  silently becomes wrong, with nothing to detect it. A digest is also opaque: it reports *that* the
  shape moved and never *what* moved, so a review cannot see whether a change was additive or breaking.
  **Archive the shape itself.** One golden file per schema version, listing the record's key paths
  (sorted, recursively) as text. A test generates the current shape and asserts it equals the golden
  for the current `SCHEMA_VERSION`; a change fails the test and the diff *shows what changed*. Bumping
  means adding the next golden, deliberately.
  This buys three things a hash cannot: a stored submission can be **validated against the schema it
  declares**, years later; the version-to-version diff is **reviewable**; and there is **no hash
  function to keep stable**, so no way for history to rot.
  **Golden files are append-only and a published version is never redefined** -- the same discipline as
  [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Once a record exists in the wild claiming schema N,
  N's meaning is fixed, because the record cannot be regenerated. Verify by sabotage: add a field,
  confirm the test fails and names the difference; bump, add the golden, confirm it passes.

- [ ] **PT-3.2** -- **Stamp the exact build, and say loudly when it is not an official one.** The
  record carries the git commit, whether the working tree was dirty when it was built, the crate
  version, and whether it came from CI or a local build.
  **This is the same problem as `Provenance` one layer up, and takes the same shape**: an official
  CI-built binary from a clean tree is the trusted case, and everything else -- a local build, a dirty
  tree, an unknown commit -- must be visibly marked so a result that arrives from one is not silently
  pooled with the rest. Default to the untrusted reading when the answer cannot be established, for
  the same reason `Provenance::Synthetic` is `Default`: forgetting must be safe.
  A `build.rs` reads the commit from an environment variable when CI sets one, falls back to `git`
  when there is a repository, and records *unknown* otherwise -- which is exactly what a `cargo
  install` from a crates.io tarball will produce, and is the honest answer there.

- [ ] **PT-3.3** -- Emit **one** machine-readable record per run, carrying: the schema version
  (PT-3.1), the build identity (PT-3.2), the topology **provenance**, a UTC timestamp, the host
  fingerprint, every placement measurement, and every node-hop measurement.
  **Build identity is the load-bearing field.** Results will arrive over months from different builds,
  and a measurement that does not say which build produced it is an unlabelled number -- the exact
  failure this workspace spent [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md)
  `D-12` fixing one layer down. There is currently **no** version stamped in any probe output.

- [ ] **PT-3.4** -- Keep the human-readable report as well, and derive both from the same measured
  values so they cannot disagree. The reader running the tool should be able to see, in prose, the
  same conclusion the record encodes -- otherwise nobody notices when a run is nonsense.

- [ ] **PT-3.5** -- Write the record to a **file** by default, named predictably, and tell the user
  exactly where it is and what to do with it. Asking someone to copy terminal output invites truncated
  and reflowed submissions.

- [ ] **PT-3.6** -- Read the three machine-description fields PT-1.2 settled, each of which needs a
  source this crate does not currently use. **Every one of them is optional in the record**, so a host
  that will not answer produces a record missing a field rather than a failed run or a fabricated
  value.
  - **CPU model** -- the registry's `ProcessorNameString` under
    `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0` is the pragmatic source and works on both
    x64 and ARM64, unlike the CPUID brand string.
  - **OS build** -- the reported version must be the real one. The Win32 compatibility shims lie to
    unmanifested processes about the major version, so verify against a known build rather than
    trusting the first API that returns a number.
  - **Virtualisation hint** -- record confidence honestly. There is no user-mode call that decides
    this, so whatever signal is used, the field says "hint" and a negative means *not detected* rather
    than *bare metal*.

## M4: the runner's experience, and their trust

- [ ] **PT-4.1** -- **One entry point.** A single binary that runs everything and produces one record.
  "Run these three and send me all three outputs" is friction for someone doing a favour, and invites
  partial submissions that cannot be compared.

- [ ] **PT-4.2** -- **State the runtime before doing the work**, from the discovered topology rather
  than a guess: the hop matrix alone is `n*(n-1)/2` hops times two strategies times three repetitions
  times two million items, on top of the placements. On a four-node machine that is a materially
  longer run than on this one, and the person deserves to know before it starts.

- [ ] **PT-4.3** -- **Say exactly what is collected and what is not**, in the tool's own output and in
  its README, and make it verifiable by reading the record. Collected, per PT-1.2: core/cache/NUMA
  shape, timings, CPU model, OS build, and the virtualisation hint. **Not** collected: hostname, user
  name, file paths, environment variables, serial numbers, or anything about installed software --
  and that list is a commitment, not a description of the current implementation. **The tool makes no
  network connections**; the person sends the file themselves, deliberately. Mention the model
  suppression flag here, where someone deciding whether to run it will actually see it -- alongside
  the honest limit from PT-1.2, that the flag does not make confidential hardware safe to submit,
  because the topology describes the part regardless.

- [ ] **PT-4.4** -- Pin the thread-pinning failure behaviour for a stranger's machine. It currently
  panics, which is right for us (a silently unpinned thread measures the scheduler, not the placement)
  but reads as a crash to someone doing a favour. It must fail with an explanation of what could not
  be pinned and why the run cannot continue honestly -- **and must not fall back to an unpinned
  measurement**, which would produce a plausible number that means nothing.

- [ ] **PT-4.5** -- **Let the runner see everything before sending it, and decide with the real values
  rather than a promise.** This is a stronger privacy property than any suppression flag, and cheaper:
  the record is a text file, so the honest instruction is "open it and read it -- if you are not happy
  with something in there, do not send it."
  Two things make that instruction usable rather than theatre:
  - **A fast preview of the machine-description fields**, available without running the measurement.
    The full run takes minutes and grows with node count; nobody should have to spend that to discover
    what the tool would learn about their machine. Someone can then look, decide, and only then commit
    to the run.
  - **A record a human can actually read** -- field names that mean something without a schema in hand,
    and no opaque blobs. A file that must be decoded to be checked cannot honestly be described as
    inspectable.

## M5: distribution

**The CI-built artifact is the canonical way to get this tool**, not `cargo install`. Two reasons, and
the second is the real one: a downloader needs no Rust toolchain, and **the download itself is the
provenance**. A binary attached to a release in this repository is traceable to the commit that built
it, in a way a locally built copy of the same source is not -- which is what makes PT-3.5's "official
build" distinction meaningful rather than decorative.

- [ ] **PT-5.1** -- CI builds the tool on tag and attaches the binary to a GitHub release, injecting
  the commit into the environment variable PT-3.5 reads. **Verify the negative case**: a locally built
  binary must produce a record marked as an unofficial build, and a CI-built one must not. A
  distinction nobody has watched fail is a distinction that does not work.

- [ ] **PT-5.2** -- A README written for someone who has never seen this repository: what question the
  tool answers, why their machine is interesting, where to download it, how to run it, what to send
  back, and what it collects. Assume no context and no obligation. Lead with the download, not with
  `cargo install`.

- [ ] **PT-5.3** -- Decide whether to publish to crates.io **as well**, and record the reasoning. It
  costs a semver obligation and yields records whose commit is *unknown* by construction (a crates.io
  tarball carries no repository), which is a strictly weaker submission. The case for it is reach; the
  case against is that the weaker path is also the more discoverable one, and submissions will drift
  towards it.

- [ ] **PT-5.4** -- Package metadata and a statement of what is and is not covered by semver. The
  **record's schema is a compatibility surface** the moment anyone stores one; the internal measurement
  code is not.

- [ ] **PT-5.5** -- Walk the whole path end to end on a machine without this repository checked out:
  download, run, find the record, read the README's instructions for sending it. A path nobody has
  walked is a path that does not work, and the person walking it will be doing a favour rather than
  debugging.

## M6: is a set of "equivalent" processors actually equivalent?

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
