# Checklist: a shareable placement-cost tool

**Goal.** A small, publishable Windows tool that a stranger can install, run once, and send back a
single structured result -- so that this workspace can collect placement and NUMA-hop measurements
from hardware it does not own. **The motivating gap is concrete: every host available here has exactly
one NUMA node**, so the entire `cross NUMA node` row and the whole inter-node hop matrix are
unmeasured, and no amount of local work will change that.

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

- [ ] **PT-1.2** -- **Decide what the submission record carries about the machine beyond the
  fingerprint**, specifically the CPU model name. The fingerprint deliberately omits model names
  because "a fingerprint that changes when the answer does not is a fingerprint nobody can compare" --
  correct for comparing placements, and a real loss when a stranger sends a result you cannot ask
  follow-up questions about. Likely answer is that the canonical string stays clean and the submission
  record carries the model *separately*, but that is a decision with a privacy dimension and is made
  here, once, explicitly. Whatever is decided, the tool must be able to state plainly what it collects.
  **This must be settled before the first submission arrives, because the asymmetry is brutal.** A
  record cannot be regenerated: whatever a field the tool did not collect, every result gathered before
  the omission was noticed lacks it *permanently*, and the machines are other people's. Under-collecting
  is unrecoverable; over-collecting is a privacy cost that can at least be corrected going forward by
  collecting less. That asymmetry argues for erring towards more context in the record -- but it is an
  argument to weigh against what a stranger will consent to, not a licence, and the two must be
  answered together rather than by defaulting to whichever is easier to implement.

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

## M4: the runner's experience, and their trust

- [ ] **PT-4.1** -- **One entry point.** A single binary that runs everything and produces one record.
  "Run these three and send me all three outputs" is friction for someone doing a favour, and invites
  partial submissions that cannot be compared.

- [ ] **PT-4.2** -- **State the runtime before doing the work**, from the discovered topology rather
  than a guess: the hop matrix alone is `n*(n-1)/2` hops times two strategies times three repetitions
  times two million items, on top of the placements. On a four-node machine that is a materially
  longer run than on this one, and the person deserves to know before it starts.

- [ ] **PT-4.3** -- **Say exactly what is collected and what is not**, in the tool's own output and in
  its README, and make it verifiable by reading the record: core/cache/NUMA shape, timings, and
  whatever PT-1.2 decides -- **not** hostname, username, paths, or environment. **The tool makes no
  network connections**; the person sends the file themselves, deliberately.

- [ ] **PT-4.4** -- Pin the thread-pinning failure behaviour for a stranger's machine. It currently
  panics, which is right for us (a silently unpinned thread measures the scheduler, not the placement)
  but reads as a crash to someone doing a favour. It must fail with an explanation of what could not
  be pinned and why the run cannot continue honestly -- **and must not fall back to an unpinned
  measurement**, which would produce a plausible number that means nothing.

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
