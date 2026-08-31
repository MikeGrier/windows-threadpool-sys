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

- [ ] **PT-1.3** -- **Decide the fate of the three existing probe binaries** (`probe-topology`,
  `probe-core-affinity`, `probe-peer-index-cache`) once their modules move. Keeping them as thin
  wrappers preserves the internal workflow; deleting them removes a second way to run the same
  measurement and a second place for output to drift. **Do not decide by taste -- the risk being
  weighed is two renderings of one measurement disagreeing**, which this investigation has already hit
  three times.

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

- [ ] **PT-3.1** -- Emit **one** machine-readable record per run, carrying: a **schema version**
  (separate from the tool version -- a collector needs to know whether it can parse the file at all),
  the **tool version**, the topology **provenance**, a UTC timestamp, the host fingerprint, every
  placement measurement, and every node-hop measurement.
  **The tool version is the load-bearing field.** Results will arrive over months from different
  builds, and a measurement that does not say which build produced it is an unlabelled number -- the
  exact failure this workspace spent [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md)
  `D-12` fixing one layer down. There is currently **no** version stamped in any probe output.

- [ ] **PT-3.2** -- Keep the human-readable report as well, and derive both from the same measured
  values so they cannot disagree. The reader running the tool should be able to see, in prose, the
  same conclusion the record encodes -- otherwise nobody notices when a run is nonsense.

- [ ] **PT-3.3** -- Write the record to a **file** by default, named predictably, and tell the user
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

## M5: publishing

- [ ] **PT-5.1** -- A README written for someone who has never seen this repository: what question the
  tool answers, why their machine is interesting, how to install and run it, what to send back, and
  what it collects. Assume no context and no obligation.

- [ ] **PT-5.2** -- Package metadata, and a statement of what is and is not covered by semver. The
  **record's schema is a compatibility surface** the moment anyone stores one; the internal
  measurement code is not.

- [ ] **PT-5.3** -- Release, and confirm a clean `cargo install` from crates.io on a machine without
  this repository checked out. An install path nobody has walked is an install path that does not work.
