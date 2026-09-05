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

## M1: decisions that shape everything after

- [x] **PT-1.1** -- **Name the crate**, and record the reasoning. It measures what a producer/consumer
  handoff costs as a function of where the two threads run, which is broader than queues and narrower
  than "topology". Candidates to weigh rather than a foregone answer: `windows-placement-probe`,
  `windows-handoff-cost`, `windows-locality-report`. Check availability on crates.io before settling.
  **Named `windows-placement-probe`.** It says what the thing is (a probe, not a library to build on)
  and what it measures (placement), and it matches the `probe-` binary naming already in this
  workspace. `windows-handoff-cost` was rejected as too narrow -- the tool already reports topology and
  NUMA hops, which are not handoffs -- and `windows-locality-report` as understating that it *measures*
  rather than summarises. All five candidates were confirmed free on crates.io before choosing.
  Created as a workspace member with `publish = false`, because PT-5.3 has not decided crates.io yet
  and `false` is the setting that cannot publish something by accident.

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

**Executes after M2, despite the number.** The code it changes lives in
[crates/windows-platform-probes](crates/windows-platform-probes) until the move, and doing this work in
its final home keeps the move a pure relocation with its provenance trail intact. The "before" in the
heading is about the *machine*, not about M2.

**This is the blocker that would waste the opportunity.** A large multi-socket host -- the kind that is
the entire point of this tool -- has more than 64 logical processors, so Windows presents it as
**multiple processor groups**, each numbering from zero.

- [x] **PT-1B.1** -- **Carry `(group, number)` as a processor's identity.** `ProcessorPlace` keys on a
  bare `u8` number and `places_from_topology` discards the group outright (`for (_group, number)`), so
  every group's processor 5 collides on one map key. **The result is not a crash.** Numbers stay below
  64 within a group, so `assert!(cpu < 64)` never fires: the tool runs, pins to whichever processor
  won the collision, and prints a confident placement table describing a topology it silently
  collapsed. That is the same defect class as the omitted SMT row, on the machine we would get one
  attempt at.

- [x] **PT-1B.2** -- **Pin with `SetThreadGroupAffinity`.** `SetThreadAffinityMask` takes a mask
  within the caller's current group and cannot express a processor in another one, so it is not a
  matter of widening the mask. Keep the existing failure discipline: pinning that does not land must
  abort the run rather than fall back to an unpinned measurement.

- [x] **PT-1B.3** -- **Verify against a synthetic multi-group topology**, since no host here has more
  than one group. `places_from_topology` is a pure conversion and already testable; a fixture with two
  groups whose numbers overlap must produce distinct processors, and the sabotage is to key on the
  number alone and watch the count halve.
  **Nine tests added, and they found two real defects rather than confirming the change.** `classify`
  compared `core` without comparing `group`, so two processors in different groups whose core ids
  collided were reported as **SMT siblings** -- attributing a shared L1 that cannot exist, since a core
  cannot span a group. And the fallback core id was derived from the number alone, which is what made
  those collisions possible.
  **The planned sabotage could not be performed, which is the strongest available result.** Keying the
  conversion on the number alone no longer *compiles*: the maps are keyed `(u16, u8)`, so the collapse
  is unrepresentable rather than merely tested against. The sabotage that does compile -- dropping the
  group comparison from `classify` -- was performed and is caught.
  **One test asserted a promise the code never made** and was rewritten rather than the code bent to
  fit: `representative_pairs` returns one pair per placement *category*, and "in a different group" is
  not a category, so requiring a pair from every group was wrong. Its fixture also gave both groups the
  same cache-domain ids, describing a cache shared across groups, which no machine does.

- [x] **PT-1B.4** -- **Refuse loudly if groups are present and unsupported.** Whatever remains
  unimplemented when a large machine is offered, the tool must say so and stop. A refusal costs one
  message; a collapsed topology costs a wrong answer nobody can detect from the output, on hardware
  that is not coming back.

## M1C: direction and memory placement, before a NUMA machine is spent

**Raised by the engineer asking why the hop count was the edge count rather than twice it.** It should
be twice it, and answering that exposed a second defect underneath.

- [x] **M1C.1** -- **Measure both directions of a node pair.** `node_pairs` is undirected, with the
  reasoning that `0 -> 1` and `1 -> 0` "traverse the same link". That conflates the *link*, which is
  symmetric, with the *workload over it*, which is not: the producer **writes** slots and
  release-stores `tail`, the consumer **reads** slots and release-stores `head`, and a remote write
  needs exclusive ownership and invalidation where a remote read does not. Swapping the ends is a
  different measurement, not a repeat.
  Doubles the hop count, so state the cost plainly: `n*(n-1)` rather than `n*(n-1)/2`. On a four-node
  host that is 12 hops instead of 6, and the runtime estimate must follow.
  **Keep the two directions distinguishable in the record.** Reporting a mean of them would destroy
  exactly the asymmetry this item exists to measure.

- [x] **M1C.2** -- **Control and record which node the ring's memory is on.** `Ring::new` runs on the
  calling thread, which is never pinned, so under first-touch the ring lands on whatever node the
  *orchestrating* thread happened to occupy -- possibly neither the producer's nor the consumer's.
  **On a multi-socket machine there are three positions, not two**, and the third is currently
  uncontrolled and unrecorded. Two runs could differ solely because the main thread migrated, with
  nothing in the output to say so.
  This is not a refinement; it is what makes a NUMA number mean anything. A hop measured with the
  memory on an unknown third node is not a measurement of that hop.
  **Decided: measure both endpoints as separate rows.** The memory goes on the producer's node in one
  row and the consumer's in another, and **the memory node is recorded beside the two processor
  nodes** in every row.
  This measures remote-write and remote-read cost independently, which is the pair of quantities the
  asymmetry in M1C.1 is actually about: with memory on the producer's node the producer writes locally
  and the consumer reads remotely, and swapping the memory reverses exactly that.

  **The cost, stated plainly.** Four configurations per undirected edge -- two directions times two
  memory placements -- so `2*n*(n-1)` hop measurements rather than today's `n*(n-1)/2`. On a four-node
  host that is 24 rather than 6, and at two strategies and three repetitions it is 144 timed handoffs
  for the hops alone. Under two minutes at the worst per-item cost measured so far, which is
  affordable for hardware this scarce. **PT-4.2's estimate must be updated with it**, or the tool will
  under-promise the wait on precisely the machines that take longest.

  **The design carries its own consistency check, which is worth keeping rather than optimising
  away.** Of the four configurations per edge, two are "producer-local" and two are "consumer-local",
  differing only in which physical node each role sits on. On a symmetric interconnect each pair
  should agree; **if they disagree, the interconnect is asymmetric, and that is a finding** rather
  than noise. Averaging the pairs, or measuring only one of each, would discard it.

- [x] **M1C.3** -- **Say what the placement label means once direction exists.** A row currently reads
  as a pair of positions; it must read as producer-here, consumer-there, memory-somewhere. The
  existing `Placement` names are direction-free and will quietly under-describe a directed run, which
  is the "table with right labels and wrong pairs" failure in a new place.

## M2: the move

- [x] **PT-2.1** -- Move `fingerprint`, `core_affinity` and `peer_index_cache` into the new crate, and
  make `windows-platform-probes` depend on it. This inverts today's direction deliberately: the
  published crate owns the measurement, the internal grab-bag borrows it. A **pure relocation** with
  the provenance trail the repository requires for a split -- commit trailers and per-file headers --
  because these modules carry a session's worth of hard-won reasoning in their comments and blame must
  survive.

- [x] **PT-2.2** -- Keep `queue_contention` and every unrelated probe where they are. The new crate is
  not a home for "measurement code in general"; it is one tool with one question, and admitting a
  second unrelated probe is how it becomes the grab-bag it was extracted from.

- [x] **PT-2.3** -- Verify the move changed no behaviour: the three probe binaries (or their
  replacements per PT-1.3) produce the same numbers on this host as recorded in
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.4, and the full sabotage set still fails
  where it should.
  **Verified three ways.** Git recorded all five files as **100% renames**, so this was a pure
  relocation and `--follow` and blame both carry through without the provenance headers a *split*
  would need -- a split copies and leaves the source behind, a move does not. Test counts add up
  exactly: 58 in the new crate plus 25 in the old is the 83 that existed before. And
  `probe-core-affinity` reproduces its pre-move results (siblings 1.8x WINS at batch depth ~85,
  cross-cache 0.54x LOSES at ~1.9), which is the check that matters, because compiling proves the
  names resolved and nothing more.
  **One defect surfaced, unrelated to the move and pre-existing:** `queue_contention` still imported
  `windows_waitable_queues::mpsc`, stale since the `slotwise_mpsc` rename. It went unnoticed because
  nothing had rebuilt that crate since, and the move is what forced the rebuild. Fixed here rather
  than left for the release.

## M3: the submission record

Ordered so each item's prerequisites land first: the two identity fields are decided before the record
that carries them is written.

- [x] **PT-3.1** -- **A linearly increasing integer schema version that cannot silently drift, guarded
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

- [x] **PT-3.2** -- **Stamp the exact build, and say loudly when it is not an official one.** The
  record carries the git commit, whether the working tree was dirty when it was built, the crate
  version, and whether it came from CI or a local build.
  **This is the same problem as `Provenance` one layer up, and takes the same shape**: an official
  CI-built binary from a clean tree is the trusted case, and everything else -- a local build, a dirty
  tree, an unknown commit -- must be visibly marked so a result that arrives from one is not silently
  pooled with the rest. Default to the untrusted reading when the answer cannot be established, for
  the same reason `Provenance::Synthetic` is `Default`: forgetting must be safe.
  A [build.rs](crates/windows-placement-probe/build.rs) reads the commit from an environment variable when CI sets one, falls back to `git`
  when there is a repository, and records *unknown* otherwise -- which is exactly what a `cargo
  install` from a crates.io tarball will produce, and is the honest answer there.

- [x] **PT-3.3** -- Emit **one** machine-readable record per run, carrying: the schema version
  (PT-3.1), the build identity (PT-3.2), the topology **provenance**, a UTC timestamp, the host
  fingerprint, every placement measurement, and every node-hop measurement.
  **Build identity is the load-bearing field.** Results will arrive over months from different builds,
  and a measurement that does not say which build produced it is an unlabelled number -- the exact
  failure this workspace spent [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md)
  `D-12` fixing one layer down. There is currently **no** version stamped in any probe output.

- [x] **PT-3.4** -- Keep the human-readable report as well, and derive both from the same measured
  values so they cannot disagree. The reader running the tool should be able to see, in prose, the
  same conclusion the record encodes -- otherwise nobody notices when a run is nonsense.

- [x] **PT-3.5** -- **The terminal output is the submission.** Collection happens by asking people to
  paste a run into a GitHub Discussions thread on this repository, so the paste is the channel and the
  whole record must survive it.
  **This reverses this item's original reasoning, and the reversal is the point.** It previously said
  to write a file *because* copying terminal output invites truncated and reflowed submissions. That
  risk is real and does not go away by choosing a different channel -- it has to be *mitigated* rather
  than avoided:
  - **Everything needed is on screen.** The record prints to stdout, not only to a file. A submission
    that requires the sender to find and attach a file will sometimes arrive without it.
  - **A self-check the reader can run.** A short checksum over the record, printed beside it, so a
    truncated or reflowed paste is *detectable* rather than silently half-ingested. This is the same
    principle as the schema golden: detect corruption instead of trusting the channel.
  - **Paste-safe formatting.** GitHub renders Discussions as markdown, so the output must survive a
    fenced code block and must not depend on colour, cursor control, or overlong lines that wrap.
  - **Tell the runner exactly what to do**, in the output itself: which thread, and to paste inside a
    fenced block. An instruction that lives only in a README is an instruction half of them will not
    have read.
  **The target is select-all, copy, paste, done.** Every extra step is a submission that does not
  arrive, so the tool emits its own markdown fences: a runner who has never thought about markdown
  pastes the whole thing and it renders as a code block anyway. Instructions caught inside the fence
  are trivial noise next to a paste that renders as mangled prose.
  A file is still written, because it costs nothing and someone will prefer to attach one -- but it is
  a backup, never a required step, and the run must be complete and submittable without it.

- [x] **PT-3.6** -- Read the three machine-description fields PT-1.2 settled, each of which needs a
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

- [x] **PT-4.1** -- **One entry point.** A single binary that runs everything and produces one record.
  "Run these three and send me all three outputs" is friction for someone doing a favour, and invites
  partial submissions that cannot be compared.

- [x] **PT-4.2** -- **State the runtime before doing the work**, from the discovered topology rather
  than a guess: the hop matrix alone is `n*(n-1)/2` hops times two strategies times three repetitions
  times two million items, on top of the placements. On a four-node machine that is a materially
  longer run than on this one, and the person deserves to know before it starts.

- [x] **PT-4.6** -- **Set up the Discussions thread people paste into, and link it from the tool.**
  The tool's output names where to send a result, so that destination has to exist before the tool
  ships, not after -- an instruction pointing at a thread that is not there is worse than no
  instruction. Pin it, and state in the first post what is collected and what a submission is used
  for, so a reader who arrives from a search rather than from the README still sees it.
  **Done: [discussion 55](https://github.com/MikeGrier/windows-threadpool-sys/discussions/55), "Please
  share data from `windows-placement-probe`".** The tool points at that thread rather than at the
  discussions index, so a runner lands on the reply box instead of a list they have to search -- a
  link that costs an extra navigation is one more place a submission stops. A test asserts the URL
  still ends in a discussion number, which catches the plausible later mistake of trimming it back to
  the index during a tidy-up.

- [x] **PT-4.3** -- **Say exactly what is collected and what is not**, in the tool's own output and in
  its README, and make it verifiable by reading the record. Collected, per PT-1.2: core/cache/NUMA
  shape, timings, CPU model, OS build, and the virtualisation hint. **Not** collected: hostname, user
  name, file paths, environment variables, serial numbers, or anything about installed software --
  and that list is a commitment, not a description of the current implementation. **The tool makes no
  network connections**; the person sends the file themselves, deliberately. Mention the model
  suppression flag here, where someone deciding whether to run it will actually see it -- alongside
  the honest limit from PT-1.2, that the flag does not make confidential hardware safe to submit,
  because the topology describes the part regardless.

- [x] **PT-4.4** -- Pin the thread-pinning failure behaviour for a stranger's machine. It currently
  panics, which is right for us (a silently unpinned thread measures the scheduler, not the placement)
  but reads as a crash to someone doing a favour. It must fail with an explanation of what could not
  be pinned and why the run cannot continue honestly -- **and must not fall back to an unpinned
  measurement**, which would produce a plausible number that means nothing.

- [x] **PT-4.5** -- **Let the runner see everything before sending it, and decide with the real values
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

- [x] **PT-4.7** -- **Lay the JSON out for a reader rather than a parser.** `to_string_pretty` gives
  every array element its own line, so eight cache domains cost eight lines saying `2` and a
  sixty-four-node host would spend sixty-four lines listing its nodes one integer at a time -- on
  precisely the machine whose submission matters most. Arrays holding no object now collapse onto one
  line, or fill lines up to a width budget, while objects still expand one field per line.
  **Field order is part of the contract, not incidental.** The first draft laid out a
  `serde_json::Value`, whose object is a `BTreeMap`, and every test still passed while the output
  quietly sorted `build` above `schema_version` and made each measurement row open with
  `consumer_batch`. Caught by reading a run, not by the suite. Order is now preserved by walking an
  order-preserving tree; `serde_json`'s `preserve_order` feature is deliberately **not** used, because
  cargo unifies features and four other crates in this workspace share `serde_json`.

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
## M36 -- Redact the secondary metadata by default

- [x] **M36.1** -- **Floor the record's timestamp to the minute, in UTC.** Done 2026-09-04. A
  second-precision stamp links two submissions from one host to each other even after every
  identifying field is withheld, and nothing in the analysis needs finer -- these measure a machine's
  shape, not an ordering of events. UTC with no local offset, because an offset narrows the submitter
  to a band of longitudes for no gain. `recorded_at_subsecond_millis` is untouched: it is
  `serde(skip)` and exists only so two runs in one second get distinct file names.

- [x] **M36.2** -- **Redact the secondary metadata by default, with an opt-in to include it.**
  Done 2026-09-04, with a single `--include-metadata` as recommended; `--no-cpu-model` survives as a
  subtraction from it, because the confidential-part case it was built for is not covered by the
  general opt-in and passing it alone is redundant rather than wrong. Suppression is recorded for
  every newly redactable field: `os_build_suppressed` and `recorded_at_suppressed` beside their
  `Option`s, and a `VirtualisationHint::Suppressed` variant rather than a flag, since that enum's
  other variants are all claims about what was observed. The backup file's name drops the stamp with
  the record, so the withheld minute cannot escape through a file a runner attaches. See
  [DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md) -> "The measurement is not
  redactable; the context is, and is withheld by default".
  Engineer's decision, 2026-09-04. The secondary metadata is the timestamp, the OS build, and the
  hypervisor name/hint -- everything in `MachineDescription` and the `recorded_at*` fields that is
  *context* rather than *measurement*. The topology is excluded from this by construction: it is the
  measurement, and the README already says so plainly.
  **Default flips to redacted.** `--no-cpu-model` becomes one case of a general rule rather than the
  only switch. Decide whether the opt-in is one flag or per-field; a single `--include-metadata` is
  the smaller surface and is the recommendation unless a per-field need appears.
  **Suppression must stay distinguishable from absence**, which the existing `model_suppressed` flag
  already does for the model: a field withheld by the runner and a field the host would not answer are
  different facts, and a collector that cannot tell them apart will read one as the other. Every newly
  redactable field needs the same treatment.
  **No `SCHEMA_VERSION` bump**: the freeze starts at the first release and this crate has not had one.

- [x] **M36.3** -- **Say in the README what redaction costs.** Done 2026-09-04, as a
  "What redaction costs" section. Built on the asymmetry `PT-1.2` already established -- withheld
  context cannot be recovered later, while over-collection can be corrected going forward -- then
  what each of the four fields buys, ordered by explanatory value rather than by sensitivity, with
  the minute named as the weakest of them. Two guards against mis-reading the new default: redaction
  does not make a submitter anonymous, because the topology is always sent and is the most
  identifying thing in the record; and a redacted submission is still a good submission, because
  sending nothing is by far the worse outcome. There is real value in correlating
  metadata anomalies with specific platform versions -- a defect that shows up only on one OS build,
  or only under one hypervisor, is exactly what the secondary metadata is for. A reader choosing to
  include it should understand they are helping, and a reader choosing not to should understand what
  they are withholding. State the trade rather than presenting redaction as free.

- [x] **M36.4** -- **On `Coherence::Disagreed`, ask for the unredacted record privately.** Done
  2026-09-04. **The dependency below was mis-stated and is corrected here**: `Coherence` was *not*
  reachable from the record. `topology_provenance` is carried and `Fingerprint` is built from the
  topology, but the fingerprint carries only the provenance, so the record had no way to know its
  two sources had disagreed. The record gained `topology_coherence`, carrying the whole `Coherence`
  including the processor lists -- a boolean would have made the ask hollow, since the record a
  maintainer is offered has to contain what they would look at. It is a field of the *record* and
  deliberately not of the `Fingerprint`, which is compared for equality to catch a spliced record
  and would then discard a good measurement over a difference in no shape at all.
  The wording is informative rather than coercive, per the engineer's direction: it reports what was
  detected, says the measurements are unaffected, names both possible causes -- inconsistent
  platform metadata *or* a defect in this tool -- as undecidable from the runner's machine, offers a
  way to help, and closes with "None of that is required." A test asserts the release is present and
  that no pressure word appears. See
  [DESIGN-NOTES.md](crates/windows-placement-probe/DESIGN-NOTES.md) -> "A disagreement is reported
  where it happens, and the ask attached to it is an offer".
  The report
  emits extra text when the topology's two sources disagreed past the retry: say that the metadata was
  inconsistent, and ask the runner to contact the `windows-threadpool-sys` maintainers through the
  discussions or issues boards and share an **unredacted** probe file **privately**, so the
  inconsistency can be verified -- or the probe fixed -- and a bug logged with Windows.
  **This is the point of the whole design.** Redaction is the default because most records do not need
  the context; the one case where the context matters most is a disagreement, which
  [D-17](crates/windows-topology-sys/DESIGN-NOTES.md#d-17) attributes to prerelease hardware,
  defective firmware tables, or a feature landing in one enumeration before the other -- the
  bug-worthy cases. So the request is made exactly there, and privately, rather than by collecting
  everything from everyone against the possibility.
  Depends on M36.2 (there must be something to un-redact) and on `Coherence` being reachable from
  the record, which it is: `topology_provenance` is already carried, and `Fingerprint` is built from
  the topology.
