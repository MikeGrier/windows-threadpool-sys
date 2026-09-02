# Checklist: ship the topology and queue crates

**Goal.** Get `windows-topology-sys` 0.2.0 and `windows-waitable-queues` 0.1.0 released, so the
placement tool in [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) has something to build
against and other people can run it on hardware this workspace does not own.

## Before checking anything off in this file

Items here are cross-linked to the other plans, and **a cross-reference is an instruction, not a
footnote**. Every marker names its counterpart and states what completing this item obliges, because
the reciprocal edit is the step that gets skipped: the work feels finished, the box gets ticked, and
a second plan silently keeps describing a world that no longer exists.

The markers, and what each obliges when its item completes:

- **MIRRORS / MIRRORED BY** -- the same work seen from two plans. **Check both boxes in the same
  commit**, and cite both IDs in the message. Neither is done alone.
- **GOVERNS / GOVERNED BY** -- this item decides something *about* the other; it never completes it.
  Write the decision onto the governed item, and leave its box alone.
- **UNBLOCKS / LIFTS THE GATE ON / GATED BY** -- edit the gated file's gate paragraph so it states the
  new reality. A gate that has silently lifted is as harmful as one that has not.
- **FEEDS / FED BY** -- write the *answer* into the fed item. It does not check that item off.

Every marker in this repository's root checklists is reciprocal: if you follow one and find no
counterpart at the other end, that is a defect to fix, not a link to ignore.

**Deliberately redundant with [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md).** That file plans the
*design*; this one plans the *release*, and a release has its own failure modes that a design checklist
will not surface. Where the two overlap -- M31.8 in particular -- this file states why the item is
release-blocking rather than restating the decision itself.

## The state this starts from, verified rather than assumed

- `windows-topology-sys` **0.1.0 is published**. The `provenance` field added on this branch is a
  breaking change to a struct with public fields, so the next release is **0.2.0**, not 0.1.1.
- `windows-waitable-queues` **is not published**. First release, and its packaging is already complete:
  description, keywords, categories, README, documentation link and a workspace license are all
  present. Packaging is not a blocker; do not re-investigate it.
- `windows-ioring-sys` **0.2.0 is published and depends on `windows-topology-sys = "0.1.0"`.**
- This branch is **54 commits ahead of `main` with no pull request**, and release automation runs on
  `main`. Nothing ships until it merges.

## M1: settle the public surface before it is public

- [x] **SH-1.1** -- **MIRRORS [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M31.8 -- one piece of
  work seen from two plans. Check both off in the same commit; neither is done alone.**
  **Decide M31.8 (merge-or-delete for `slotwise_mpsc` and `reserving_mpsc`) before the first
  publish, not after.** This is the highest-leverage item in the file and it is release-blocking for a
  mechanical reason: the decision may *delete a public type*. Doing that before 0.1.0 costs nothing;
  doing it after means a breaking release, a yank-and-migrate for anyone who adopted it, and a
  permanent line in the changelog explaining why a shape existed for one version.
  The measurement is already done and agrees across both architectures -- see M31.5 and M31.7 in
  [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) -- so this needs a decision, not more work.

- [x] **SH-1.2** -- **GOVERNS [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M31.6 -- this is not
  that item and does not complete it.** It decides only whether M31.6 blocks SH-4.3. If the answer is
  "it gates", record that on M31.6 and SH-4.3 cannot proceed until M31.6 is done; if "it does not",
  record that too, so a later reader does not mistake a considered choice for an oversight. **Checking
  this off never checks off M31.6.**
  **Decide explicitly whether M31.6 (loom verification) gates 0.1.0**, and record the
  answer either way rather than letting it drift into "not yet".

  **Decided: it does not gate 0.1.0. It gates 1.0, and the gap is disclosed in the crate's own
  documentation rather than left for an adopter to discover.** Recorded as D-31.

  Three findings drove it, and the second was not expected:

  - **Loom would close the demonstrated gap.** The sabotage sweep showed a weakened `Acquire` on the
    producer's load of `head` survives the whole suite. That defect lives in queue code, which is
    exactly what loom models well.
  - **Loom would *not* close the gap where a real bug actually occurred.** The doorbell's correctness
    is the interleaving of an `AtomicBool` mirror with real `SetEvent`/`ResetEvent` syscalls. Loom
    models the atomics and cannot model the syscalls; stubbing them tests a *model* of `SetEvent`
    rather than `SetEvent`. D-15's lost wakeup -- the only ordering bug this crate has actually had --
    was found by sabotage, and loom would not have found it. So loom is valuable and is **not** the
    thing standing between this crate and confidence about its hardest part.
  - **The risk loom addresses is mostly regression risk**, and that risk is lowest now. The orderings
    are believed correct and were reasoned about at the time; sabotage *introduced* the weakening to
    prove the suite was blind to it. Regression risk rises with contributors, changes, and consumers
    -- all of which start after publication, not before.

  Against that, gating would block 0.1.0, and through it the placement tool and the NUMA measurements
  from other people's machines that this whole sequence exists to obtain. Loom is invasive work: every
  atomic in the crate goes behind a `cfg` shim across four modules.

  **The disclosure is what makes this a decision rather than a punt**, and it is not optional: the
  crate documentation states what is verified, states that stress testing here is *known* not to catch
  ordering defects and cites the measurement showing it, and says loom is planned before 1.0. An
  adopter then makes their own call with the same information we have. `0.x` carries the rest.
  The reason it deserves a deliberate answer rather than a default: the sabotage sweep demonstrated
  that weakening the producer's `Acquire` load of `head` to `Relaxed` left **all twenty tests green**,
  while every logic defect injected beside it was caught. So this is not an untested-by-omission gap,
  it is a gap this workspace has *evidence* the existing tests cannot close. Publishing a lock-free
  queue with it open is a defensible choice; making it unknowingly is not.

- [x] **SH-1.3** -- **Qualify both MPSC shapes by name.** `mpsc` beside `reserving_mpsc` made one
  canonical by implication -- which contradicts this crate's own "no shape is the canonical one", and
  after SH-1.1 is simply false. Renamed to `slotwise_mpsc`, which names its claim protocol: it claims
  slot by slot with no shared counter. `sequence_mpsc` was considered and rejected for inviting the
  reading that it alone preserves FIFO order, which both shapes do. Recorded as D-30.
  **Belongs in M1 for the same reason SH-1.1 does**: it is a public-surface change, free before the
  first publish and a breaking rename with a deprecation path afterwards.

- [x] **SH-1.4** -- **State the algorithms' pedigree and why an existing crate is not used.** A public
  concurrent-queue crate has to answer both questions or a reader assumes the worst: that the
  algorithms are homegrown, and that the author did not look at the alternatives.
  Neither is true, and the honest answers are load-bearing. The algorithms are *published designs*
  chosen deliberately, because a concurrent queue is a bad place to be original -- the failure mode is
  a reordering that appears on one machine, under load, months later. And the reason no channel crate
  fits is structural rather than dismissive: **on Windows, waiting is a kernel-object operation**, so a
  queue whose readiness is not a `HANDLE` cannot join a `WaitForMultipleObjects` alongside an I/O
  completion, a process handle, or a cancellation event -- however good its own blocking receive, and
  however rich its own `select`, which can only select over its own channels.
  Written into both the crate docs and the README, because docs.rs shows one and crates.io the other.

- [x] **SH-1.5** -- **Bound `Reserving::Reservation<'a>` so a generic caller can redeem what it claims.**
  Done: the `Claim` trait carries `send` and `is_disconnected`, `Reservation<'a>` is bound on it, and
  both reservation types implement it as forwarders. 87 lines across five files, no concrete signature
  changed, `slotwise_mpsc` untouched because it does not implement `Reserving` at all. Mutation-tested
  rather than assumed: 62 mutants over the whole reservation surface report 0 missed, and the first
  run found a real gap -- `is_disconnected` stuck at `false` survived on `spsc`, because the connected
  case was asserted there and the disconnected case only on the other shape.
  The associated type is declared with no bound at all, so a caller generic over
  [`Reserving`](crates/windows-waitable-queues/src/traits.rs) can call `reserve()` and then do
  nothing with the result except drop it. `reserve` is `#[must_use]` precisely because a held claim
  withholds capacity from every other producer -- and the one operation that discharges it, `send`,
  is inherent to each shape's concrete type and unreachable through the trait. The trait cannot
  express the operation it exists for.

  **The two implementors already agree exactly, so this is additive**: both
  `spsc::Reservation<'a, T>` and `reserving_mpsc::Reservation<T>` already have
  `send(self, item: T) -> Result<(), Disconnected<T>>`, `is_disconnected(&self) -> bool`, and a
  `Drop` that returns the slot. No concrete signature changes; nothing to migrate.

  Add a `Reservation` trait carrying `send` and `is_disconnected`, bound the associated type on it,
  and implement it for both types. `is_disconnected` is included rather than deferred for the reason
  the `Reserving` docs give at length -- a caller needs to learn the stream ended *before* doing the
  work the claim was taken for -- and because `reserving_mpsc`'s reservation is `Send`, so it may be
  redeemed on a thread holding no producer handle to ask instead. Adding it later is the same
  breaking change, merely deferred.

  **Why this blocks rather than waits.** Adding a bound to an associated type is a breaking change
  to the trait: every implementor must then satisfy it. It is free while the crate is unpublished
  and a major bump with a migration afterwards, and this is the milestone that exists to settle
  exactly that -- see SH-1.1 and SH-1.3, both landed on the same "free before the first publish"
  reasoning. D-3 already makes this argument ("the trait *shape* is fixed now so signatures stay
  compatible"); this is the same reasoning applied to a piece it missed. Pull request #56 is what
  puts these traits in front of consumers, so the window closes when it merges.

  **How it surfaced**, recorded because the route is the useful part: not from review and not from a
  failing test, but from a `cargo mutants` run showing that nothing exercised the capability traits
  at all, and then from being unable to write the obvious generic test for `Reserving` -- the test
  in [traits/tests.rs](crates/windows-waitable-queues/src/traits/tests.rs) is scoped to claim-and-release
  and says so. A contract gap presenting as an untestable API is a signal worth keeping.

  Extend that test to claim-and-redeem through the trait as part of this item, since it is the
  check that would have caught the gap in the first place.

## M2: repair the release plumbing before relying on it

- [x] **SH-2.1** -- **Add `windows-waitable-queues-v*` to the tag trigger list in
  [.github/workflows/publish-crate.yml](.github/workflows/publish-crate.yml).** It is missing. The
  crate *is* registered with release-please, so release-please will happily raise the release PR and
  push the tag -- and then nothing will publish it, with no error, because no workflow matches the tag.
  **This is a silent failure, which is why it is its own item**: the symptom is a tag that exists, a
  changelog that looks right, and a crate that never appears on crates.io.
  **Done, in both the tag trigger and the `workflow_dispatch` choices** -- the second matters because
  without it the manual escape hatch could not publish the crate either, so there would have been no
  way to recover from the first failure by hand.
  **And the drift is now checked rather than remembered.** This defect existed because two files had
  to agree and nothing compared them; it was found by a reviewer, not by CI.
  [tools/check-publishable.ps1](tools/check-publishable.ps1) asserts that every crate release-please
  manages has a publish trigger, and runs as its own CI job. The comparison is deliberately
  one-directional -- a crate may be publishable without being release-managed, which is what
  `windows-placement-probe` is today. Verified by removing the trigger again and watching the check
  fail with the crate named.

- [ ] **SH-2.2** -- Plan the **`windows-topology-sys` 0.2.0 ripple**. `windows-ioring-sys` is published
  and pins `windows-topology-sys = "0.1.0"`, so the breaking bump obliges updating that dependency and
  releasing `windows-ioring-sys` too. Decide the order and whether ioring's release is part of this
  push or follows it -- but decide it, because a workspace that builds locally via `path` dependencies
  will not reveal this and the first symptom is a consumer unable to resolve the two together.
  **Three crates pin `"0.1.0"`, not one**, and they carry different obligations. Swept the workspace's
  manifests rather than trusting the one that prompted this:
  - `windows-ioring-sys` -- published, so the pin obliges a release, as above.
  - `windows-placement-probe` -- to be published later
    ([CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) -> `PT-5.6`). Its pin obliges no
    release now, but must be corrected before that publication or it would ship depending on a
    topology version it was never developed against. This is the nastiest of the three: the `path`
    entry means it keeps building perfectly all the way to the moment of publish.
  - `windows-platform-probes` -- never published, so its pin is inert. Update it with the others
    anyway rather than leaving a manifest that misstates what it was built against.

- [x] **SH-2.4** -- Clear the **eight rustdoc warnings** in `windows-waitable-queues` before it is
  published: an unresolved link to `MIN_CAPACITY`, six links from public documentation to private
  items (`Shared::len`, `Doorbell::clear`, `Doorbell`, `BOUNDS`), and one redundant explicit link
  target.
  Ordinarily out of scope for the item that found them, and in scope here for one reason: **docs.rs is
  the face of a first release.** A link that silently resolves to nothing in a workspace build renders
  as a dead or missing reference to the first person who ever reads these docs, and a link to a private
  item points at a page they cannot open.
  **Correction: they did not pre-date this branch, and they were not warnings.** This item said so, on
  the reasonable assumption that documentation nobody had touched could not have broken. `main` is
  green and the branch is red, so the branch broke them -- most likely the `mpsc` -> `slotwise_mpsc`
  rename, which moved every item these links named. And CI denies `broken_intra_doc_links` and
  `private_intra_doc_links`, so they were **errors failing every run on the pull request**, not
  warnings deferred until publication. Nothing here was blocked on the release; the release was
  blocked on this.
  **Done.** `MIN_CAPACITY` never existed anywhere -- the prose promised a constant that was never
  written -- so that sentence now states the rule itself. The private-item links are delinked rather
  than repointed, because a public page cannot link to a page that is not generated. Fixed alongside
  five more in `windows-placement-probe` and one in `windows-thread-ambient-sys` that the same job was
  failing on; the whole workspace now passes `cargo doc --workspace --all-features` under CI's exact
  `RUSTDOCFLAGS`.

- [ ] **SH-2.3** -- Dry-run both publishes (`cargo publish --dry-run`) from the merge commit, and read
  the packaged file list rather than only the exit code. A crate that builds in a workspace can still
  fail to package -- excluded files, a path dependency without a version, a README that is not in the
  package.

## M3: land the branch

- [ ] **SH-3.1** -- Open the pull request, and **review it as a diff rather than as a memory of having
  written it**. 54 commits across the topology crate, the queue crate and the probes is more than fits
  in a session's recollection, and the branch contains at least one deliberate breaking change plus
  several documented reversals of earlier conclusions.

- [ ] **SH-3.2** -- Run the full gate on the merge result, not merely on the branch tip: `cargo fmt
  --check`, `cargo clippy --all-targets`, `cargo check --all-targets` in **both** debug and release,
  and the in-scope test suites including doctests. Release-mode warnings differ from debug ones, which
  is why the milestone discipline names both.

- [ ] **SH-3.3** -- Run the `windows-waitable-queues` sabotage sweep on a clean tree and confirm every
  entry still behaves as declared. It is the crate about to become public and the sweep is what has
  caught its real defects -- including a lost wakeup that only surfaced because a *baseline* run hung
  once in an otherwise green suite.

- [ ] **SH-3.4** -- Merge to `main`, and confirm release-please raises a release PR proposing
  **0.2.0** for the topology crate. If it proposes 0.1.1, the breaking-change marker did not take and
  the version would silently understate the break -- fix the marker rather than editing the version by
  hand, or the next break will do the same thing.

## M4: release

- [ ] **SH-4.1** -- Release `windows-topology-sys` 0.2.0 and confirm it appears on crates.io and builds
  on docs.rs. Docs.rs builds under its own configuration, so a crate that documents locally can still
  fail there.
  **UNBLOCKS half of [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) PT-5.3 -- publishing
  the tool to crates.io -- which needs both releases. On completing this, update that file's gate
  bullet to record that topology has shipped**; the gate lifts only when SH-4.3 lands too, and a
  half-lifted gate that reads as lifted is how work starts against a dependency that is not there yet.
  **It does not gate the GitHub binaries**, which CI builds from this repository through `path`
  dependencies.

- [ ] **SH-4.2** -- Update `windows-ioring-sys` to depend on the published 0.2.0 and release it, per
  the order settled in SH-2.2.

- [ ] **SH-4.3** -- Release `windows-waitable-queues` 0.1.0, with SH-2.1's fix in place. Confirm the
  tag triggered a publish rather than assuming it did.
  **LIFTS THE GATE ON [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) PT-5.3 only. On
  completing this, edit that file's gate bullet to say the gate is lifted and name the two published
  versions**, so a reader arriving there later does not have to reconstruct whether it still applies.
  The tool's GitHub binaries never waited on this.
  Blocked by SH-1.1, and by M31.6 as well if SH-1.2 decided that it gates.

## M5: verify from outside the workspace

- [ ] **SH-5.1** -- In a scratch project **outside this repository**, depend on both crates from
  crates.io and build something that uses each. This is the first exercise of the crates as
  *dependencies* rather than as path members, and it is where a missing `version` on a path dependency,
  an unexported type, or a feature that only resolves inside the workspace will show up.

- [ ] **SH-5.2** -- Confirm the published `windows-topology-sys` still reports `Provenance::Measured`
  from `discover()` when consumed as a dependency, and that a `Topology::default()` is `Synthetic`.
  The provenance rules are the newest thing in the crate and the least exercised outside it.

## M6: long-running validation

**Placed last so the numbering does not churn, and it gates SH-4.3 all the same.** `windows-waitable-queues`
0.1.0 does not publish until this milestone is done. The reasoning is in SH-1.2 / D-31: the crate ships
without machine-checked orderings, and long-running validation is part of what it owes instead.

**What a pass here does not mean, stated once and repeated in the tool's own output.** Hours of green
stress says nothing about memory orderings. That is measured, not cautious: weakening the producer's
`Acquire` to `Relaxed` left the whole suite green. A stress tool that omits this becomes false comfort
-- someone points at a long clean run and concludes the orderings are fine, which is exactly the claim
D-31 says cannot be supported.

- [ ] **SH-6.1** -- **The wraparound scenario, which is the one reachable correctness gap.**
  `reserving_mpsc` packs its position into 32 bits, so it wraps after 2^32 pushes -- **between 37
  seconds and about four minutes** at this crate's own measured rates, and reachable in production
  within hours. The range is [D-26](crates/windows-waitable-queues/DESIGN-NOTES.md#d-26)'s isolated
  table read as a wrap time: 8.6 ns/push with one producer is 116M/s, so 2^32 is 37 s; 28.0 ns with
  two is 35.7M/s, so 120 s; 56.9 ns with thirty-two is 17.6M/s, so 244 s. An earlier version of this
  item said only "about two minutes", which is the two-producer figure quoted as though it were the
  whole story.
  **CORRECTION (review round nine).** An earlier version of this item said `spsc` and
  `slotwise_mpsc` "use `usize` positions and cannot be driven there at all", and that is **false on a
  32-bit target**, where `usize` *is* 32 bits. The claim was written from a 64-bit reading and never
  re-checked against the 32-bit support the crate otherwise takes seriously enough to have a
  dedicated `BOUNDS_MAX` derivation and a `const` assertion for. `slotwise_mpsc` reaches the same
  wrap on such a target; `spsc` does too, but has no compare-exchange claim to be raced, so wrapping
  alone does not expose it. See SH-14.1 and SH-14.2, which are about the *correctness* hole this
  testing gap was hiding.
  What exists today is *ring* wraparound (positions cycling through slots) and the packing arithmetic
  checked at the boundary; what does not is the queue actually crossing 2^32 end to end. **Tracking
  every item is impossible at that count**, so the invariants are the cheap ones: per-producer sequence
  numbers strictly increasing in consumption order, and an exact total count. O(producers) memory
  rather than O(items).

- [ ] **SH-6.2** -- **Diagnostic history, merged by position rather than by a clock.** Unseeded is
  correct here: the scheduler is the source of variation, not the PRNG, so a seed would make the inputs
  reproducible while the interleaving that caused the failure stays unreproducible -- the appearance of
  determinism with none of the substance. What is needed is **reconstructability**.
  Each thread keeps a small lock-free ring of recent records: thread, operation, position, value,
  outcome. **The merge needs no clock and no global counter**, because the queue under test already
  carries a total order -- its positions -- so records sort by position after the fact. A global
  sequence number would give a true order and perturb the hot path it is trying to observe; a
  timestamp costs a clock read per operation. Both were considered and neither is needed.
  The one case positions do not order is a *refused* push, which has no position; record the position
  it attempted and mark it refused.

- [ ] **SH-6.3** -- **Detect the failures worth detecting**, and dump the history on any of them: item
  loss or duplication, per-producer order violation, a panic in any thread, and **no progress**. The
  last needs a watchdog thread against a progress counter, and is the case that most needs history and
  is least served by a seed -- a hang leaves no assertion behind, only a stuck process.

- [ ] **SH-6.4** -- **Cover all three shapes and the doorbell.** The doorbell is the point: SH-1.2
  established that a model checker *cannot* cover it, because its correctness is an atomic mirror flag
  interleaving with real `SetEvent`/`ResetEvent` calls. Stress is one of the few instruments that
  exercises that at all. D-15's lost wakeup surfaced because a baseline run hung **once**; more hours
  of running is the only lever we have on that class.
  Include the parking path, not just the polling one -- a consumer that never parks never exercises
  the doorbell protocol that D-9 and D-15 are about.

- [ ] **SH-6.5** -- **Ship it as a tool, not only as a test.** A binary with duration and concurrency
  knobs, so a user can stress *their* hardware. That matters concretely: x64 and ARM64 have already
  disagreed once about this crate's behaviour, and no test we run here covers a machine we do not own.
  Keep a short in-suite smoke run over the same engine so the code cannot rot, and keep it out of the
  fast unit suite, which must stay under a second.

## M7: PR #56 automated-review round

The findings an automated review raised against the pull request that lands this work, verified against
the source before being accepted. Each item names what was checked, so a later reader can tell a real
repair from a reviewer's guess that was taken on trust.

- [x] **SH-7.1** -- **`reserving_mpsc` reports `Full` from a claim word that was never current.**
  `push` and `reserve` load the claim word relaxed, then test room with
  `has_room_beyond_reservations(position, reserved)`, which computes
  `position.wrapping_sub(head)`. If other producers claim and publish past `position` and the consumer
  drains them while this thread is between the load and the room check, `head` passes the stale
  `position` and the subtraction wraps to near `u32::MAX` -- so the queue reports `Full` (and records a
  refusal) at the moment it is empty, and `reserve` returns `None` for the same reason. The compare-and-
  swap that would have caught the staleness is never reached, because both paths return before it.
  Re-read the claim and retry when it moved; report no room only from a word still current.

- [x] **SH-7.2** -- **The NUMA cross-check compares a count against a highest identifier.**
  `windows-platform-probes`'s `Observation::cross_check` compares `numa_domains` (a count of memory
  domains) with `GetNumaHighestNodeNumber() + 1`. Windows documents that value as the highest node
  *number*, and does not guarantee node numbers are dense -- nodes 0 and 2 give a count of 2 and a
  highest of 2, and the probe then reports a parsing regression on correct hardware. Memory domains
  already carry the node number in `Domain::id`, so compare highest against highest.

- [x] **SH-7.3** -- **A cache level is called a partition without checking that it is one.**
  `cache_partitions_at_level` deduplicates by equal processor set, which is exactly right for the
  measured case it was written for (L1i and L1d over identical sets). It does not establish a
  *partition*: `Topology` is deliberately constructible by hand and by deserialization (D-12), so
  distinct-but-overlapping sets reach `outermost_partitioning_cache`, which returns them as domains a
  consumer then double-counts. Require the distinct sets to be pairwise disjoint before a level
  qualifies as partitioning.

- [x] **SH-7.4** -- **`windows-waitable-queues` cannot build its documentation on docs.rs.** The crate
  is Windows-only and imports `std::os::windows::io` unconditionally, but its manifest omits the
  `[package.metadata.docs.rs]` target block that every other published Windows-only crate here carries,
  so docs.rs would build it for its default Linux target and fail. Add the same block.

- [x] **SH-7.5** -- **The mutant injector replaces every occurrence on the line, not the first.**
  `tools/inject-mutant.ps1` calls the *static* `[regex]::Replace(input, pattern, replacement, 1)`, whose
  fourth parameter is `RegexOptions` -- `1` is `IgnoreCase`, not a replacement count, and no static
  overload takes a count at all. The tool therefore does precisely what its own header comment says it
  exists to avoid. Fix the replacement, refuse a line whose pattern occurs more than once unless a
  column disambiguates it, verify the baseline is green before trusting a "caught", run with all
  features so a feature-gated mutation is not reported as surviving, perform the mutating write inside
  the guarded region so a failed write still restores, and route its output through one sink.

- [x] **SH-7.6** -- **A spike that fails to run is reported as a finding about the machine.**
  `tools/run-numa-spikes.ps1` checks the exit code of `cargo build` but not of `cargo run`, then decides
  vacuity by searching the output for `VACUOUS`. A crashed spike prints no such line, so the summary
  says "**NOT vacuous -- this runner has more than one NUMA node**" and the script exits 0. That is the
  instrument breaking while claiming a result, which the script's own documentation says is the one
  thing worth failing over.

- [x] **SH-7.7** -- **Two tools write output from several sites, and two hazards remain in the
  sabotage/mutation harness.** `tools/check-publishable.ps1` and `tools/inject-mutant.ps1` each call
  `Write-Host` from several places, against the repository's one-output-sink rule.
  `tools/run-sabotage.ps1` performs its patching write before entering the `try` whose `finally`
  restores the file, so a write that throws part-way leaves the clean source damaged.
  `tools/run-mutants.ps1` derives a deterministic output directory per package or file, so a second run
  of the same scope overwrites the analysis the parameter documentation promises to preserve.
  The placement probe's tests name scratch directories without the process id, so two concurrent test
  processes -- which the documented `-j 2` mutation workflow creates -- delete each other's fixtures.

- [x] **SH-7.8** -- **Reply to every thread and resolve the ones that are addressed**, including the one
  finding that was checked and found not to hold: `GetSystemDirectoryW` returning exactly the buffer
  length is unreachable (success excludes the terminator, failure includes it and so exceeds the
  buffer), though the guard is widened anyway so the next reader need not redo the analysis.

## M8: PR #56 third review round (suppressed findings)

The reviewer generated no new inline comments in these rounds and instead listed **suppressed** findings in
the review body, so none of them arrived as a resolvable thread. They are recorded here because a finding
that produces no thread is otherwise invisible to the "are all comments resolved?" check that gates merge.

- [x] **SH-8.1** -- **The contention probe times thread creation, and lets early producers run alone.**
  All five timed runs in `windows-platform-probes`'s `queue_contention` start the clock *before*
  `thread::scope` spawns anything, and every worker begins pushing the moment it is spawned. At 50,000
  pushes each, an early producer can finish a large uncontended prefix -- or finish outright -- while the
  last threads are still being created, so a row labelled 16 or 32 producers may never have had 16 or 32
  contenders. The measured interval also includes spawn cost. This is not a cosmetic inaccuracy: the
  module's own header says these numbers decide whether two speculative queue shapes get written at all
  and whether the two shipped shapes merge. Hold every participant -- producers *and*, in the drained
  runs, the consumer -- at a start barrier, and start the clock when it releases.

- [x] **SH-8.2** -- **A failed backup write leaves a truncated file under the canonical name.**
  `write_backup_to_new_file` reserves the name with `create_new` and then `write_all`s through `?`, so a
  disk-full or quota failure returns an error while leaving a zero-length or partial `.json` behind. That
  file is indistinguishable from a real record to whoever collects it, and the next run's collision
  suffix steps politely around it. Publish by rename: write the bytes to an exclusively-created temporary
  in the same directory, flush, and move it onto the reserved name only once the write has succeeded.

- [x] **SH-8.3** -- **`places_from_topology` drops processors and invents NUMA membership.**
  Two defects in one conversion, both reachable only through a hand-built or deserialized `Topology` --
  which is exactly the input this seam exists to accept (D-12).
  It iterates `class_of`, which is populated only from `DomainKind::Core` domains, so an online processor
  with no core domain is **silently absent from the result** -- and the documented core-id fallback
  beneath it, written to keep group 1's cpu5 distinct from group 0's, is unreachable dead code as a
  direct consequence.
  It then defaults absent NUMA membership to `unwrap_or(0)`. That is the right answer only when the
  topology names no memory domain at all; when it names nodes 1 and 2, it **fabricates node 0** and files
  a processor under a node the machine does not have -- the precise failure this crate's own rule
  ("a seam that only moves data is safe; a seam that lets fabricated labels reach real hardware is not")
  exists to prevent.
  Iterate the online processors so every one is placed, and refuse a topology that names memory domains
  but not this processor's, rather than inventing one.

## M9: PR #56 fourth review round

- [x] **SH-9.1** -- **Both bounded shapes could report a length larger than their capacity.**
  `len` reads the producer-side position and then `head`, which are two instants; a consumer draining
  past the sampled position makes the wrapping subtraction yield a number near the integer maximum. The
  comment beside it claimed the overestimate was "safe in the direction that matters for a backpressure
  gauge", which is true of a *bounded* overestimate and not of `usize::MAX`. Both are now clamped to the
  capacity, so the skew still resolves towards full -- the safe direction -- while the impossible value
  is gone.

- [x] **SH-9.2** -- **`reserving_mpsc` inherited a `remaining()` that counted reserved slots as room.**
  `Bounded::remaining` defaults to `capacity - len`, and this shape's `len` excludes reservations by
  design, so an empty queue of four holding one reservation answered four while only three items fit --
  promising room for a push guaranteed to be refused. Overridden on both handles and both trait impls,
  reading the packed claim word **once** so the position and the reservation count cannot be sampled at
  different instants; `is_full` is now defined in terms of it rather than restating the rule.

- [x] **SH-9.3** -- **The pull request description described the release plumbing, not the product.**
  The body framed the change as CI and provenance work and mentioned `windows-waitable-queues` only
  under release tracking, while the majority of the diff is that crate's public API and its three
  lock-free queue implementations. Rewritten to lead with the shipped surface.

## M10: PR #56 fifth review round

- [x] **SH-10.1** -- **`BOUNDS_MAX` does not compile on a 32-bit target.** `reserving_mpsc`'s maximum
  was a flat `1 << 31`, derived from the packed position's width alone. On a 32-bit target the
  crate-wide `WRAPPING_MAX_CAPACITY` is `usize::MAX / 2`, which is `2^31 - 1` -- *narrower* than the
  packing -- so the const assertion that no shape may exceed it fails the build outright, for every
  capacity including the small valid ones. Now the narrower of the two limits, kept a power of two so
  the value stays one a caller could actually pass. Verified in both directions against a real
  `i686-pc-windows-msvc` check: the old constant fails with `E0080`, the new one compiles.

- [x] **SH-10.2** -- **The backup's final name was visible empty for the whole write.** The previous
  round reserved the destination with `create_new` and renamed onto it, which fixed the truncated-file
  case and left a worse one: an empty file under the record's own name for the duration of the write,
  and permanently if the process was killed in that window -- contradicting the absent-or-complete
  guarantee its own doc comment claimed. Publication is now a single atomic no-replace `MoveFileExW`
  from a fully-written temporary. `std::fs::rename` cannot express this: on Windows it always passes
  `MOVEFILE_REPLACE_EXISTING`, so it would clobber a record a concurrent run had placed.

- [x] **SH-10.3** -- **The tool discovered the topology three times.** The plan used one reading, the
  fingerprint another, and `core_affinity::measure` a third, so a processor going offline mid-run could
  have the announced plan, the recorded host, and the measured rows describing different machines with
  nothing saying which. The plan and the fingerprint now derive from one `Topology::discover`.
  `measure` still discovers its own, and deliberately so: its documentation refuses a
  `measure_with(places)` seam because a supplied list's processor *numbers* stay valid on the real host
  while its node labels need not, so every pin would succeed and real timings would be filed under
  fabricated labels. Its rows carry their own places, so each row states what it measured.

- [x] **SH-10.4** -- **`spsc` had the same `remaining()` defect, and it was missed.** The previous round
  corrected `reserving_mpsc` and stopped there, but `spsc` implements `Reserving` too -- so reserving
  every slot left it reporting the full capacity as available while both `push` and `reserve` refused.
  Its `Bounded` impls now override `remaining` on the producer *and* the consumer, its `len` is clamped
  to the capacity like the other two shapes', and `is_full` is defined in terms of `remaining` rather
  than restating the rule. The trait's default now documents that a `Reserving` shape must override it,
  so the next shape to reserve does not inherit the same wrong answer silently.

- [x] **SH-10.5** -- **The high-water depth could record a peak the queue never reached.**
  `reserving_mpsc`'s `publish` sampled the depth from its own position and a relaxed load of `head`,
  ungated and unclamped. `slotwise_mpsc`'s twin is bounded by construction -- its producer's acquire
  load of the slot's sequence synchronizes-with the consumer freeing that slot, so `head` cannot be
  older than `position - capacity + 1` -- but this shape has a second entry point with no such edge:
  `Reservation::send` redeems without a room check, so the only `head` its thread is ordered against is
  the one *`reserve`* read, which may be arbitrarily old by the time the reservation is redeemed. The
  sample is now gated on tracking (parity with the twin), read before publication, and clamped to the
  capacity.
  `Observable::high_water`'s contract is corrected to match what all three shapes actually deliver: an
  **upper bound** on the true peak, never below it and never above the capacity, with the reason the
  cheap sample is preferred to an exact count. Counting exactly would put a read-modify-write on a line
  shared by every producer and the consumer into every push and every pop -- the line this crate pads
  its positions apart to keep out of the hot path.

## M11: PR #56 sixth review round

Three findings against `places_from_topology`, all of the same shape, plus three against the
mutation wrapper. The conversion's three silent fallbacks are replaced by one rule.

- [x] **SH-11.1** -- **Three fallbacks each invented an answer that reads as a real one.**
  `places_from_topology` accepted a topology whose domains do not cover every processor, and filled
  each gap with a value indistinguishable from a measured one. A processor absent from every core
  domain was given a synthetic core id derived from its group and number, which can equal a real
  core domain's id -- `classify` then reports two processors as SMT siblings when one's core is
  merely unknown. Its efficiency class became `0`, which is also a genuine Windows class, so
  `within_class_pair` reports a same-class pair against a real class-0 core. Its cache domain became
  `None`, which the type already means "no cache level partitions this machine" -- so two processors
  omitted from an incomplete partition compare equal and serialize a confident same-cache
  measurement.
  The three share one cause: an absence was read as a value. The rule now distinguishes *uniform*
  absence from a *gap*. A machine that reports no core domains at all, or no partitioning cache
  level, has told us something true about itself and still converts. A machine that places every
  other processor but not this one has told us nothing about this one, and the conversion refuses:
  `places_from_topology` returns `Err(UnplacedProcessor)` naming the processor and, in a new
  `MissingPlacement` field, which of core / cache domain / NUMA node was missing.
  `MissingPlacement` is `#[non_exhaustive]`.
  Core and efficiency class are two spellings of one rule -- `Topology::cores()` filters to
  `DomainKind::Core`, so a processor's class is known exactly when its core is -- and an
  `EfficiencyClass` variant written for the second was removed on discovering it is unreachable.
  Sabotage confirms the pair behaves that way: removing either refusal alone leaves the suite green,
  because the other still fires; removing both fails two tests.

- [x] **SH-11.2** -- **The mutation wrapper's output directory could collide.** The stamp has
  one-second resolution, so two runs launched in the same second -- a script starting several scopes
  at once, which is exactly the case that wants separate output -- selected the same directory and
  interleaved their results. A short random suffix now follows the stamp, which still sorts
  chronologically.

- [x] **SH-11.3** -- **The wrapper terminated fault handlers it did not start.** Cleanup matched
  `WerFault` / `WerFaultSecure` / `vsjitdebugger` by name across the whole session, so a crash report
  the user was reading or a debugger attached to an unrelated process was killed by a mutation sweep.
  The wrapper now records the ones already running at startup and skips them.

- [x] **SH-11.4** -- **The `mutants.out` nesting finding does not hold; documented in place.**
  The report was that `Join-Path $OutputDirectory 'mutants.out'` doubles a path cargo-mutants already
  appends. It does not: cargo-mutants treats `--output` as the parent and creates `mutants.out`
  inside it. Verified on disk -- a run with `--output .scratch\mutants-encoding-<stamp>` produced
  `<stamp>\mutants.out\caught.txt` with 22 lines, matching the 22 caught the wrapper reported. A
  comment now records the evidence, since the path reads like a duplication and has been challenged
  once already.

- [x] **SH-11.5** -- **A hard-killed run could block the next one's backup entirely.** The temporary
  was named for the record plus this process's id and nothing else, and created with `create_new`. A
  run killed mid-write leaves that file behind, and Windows reuses process ids -- so a later run
  issued the same id found the corpse under the only name it would ever try. The resulting
  `AlreadyExists` left `write_temporary` *before* the caller's suffix loop was reached, so the whole
  backup failed rather than landing under a next-best name. The temporary now carries its own
  attempt counter, matching the final name's budget; a stale file is stepped around rather than
  overwritten, since it belongs to whatever left it.

- [x] **SH-11.6** -- **Both tools bypassed their own single-output-sink contract.** `run-mutants.ps1`
  emitted its per-category summary directly to the success stream, and `run-sabotage.ps1` did the
  same for the `-List` output, every blank line, the result table, and the injected patch text --
  each contradicting the `Write-Report` doc comment directly above them. All now route through the
  sink, which gained pipeline binding so a formatted table can flow into it. Verified: the `-List`
  path emits zero objects to the success stream.

- [x] **SH-11.7** -- **The sabotage harness silently narrowed its own sweep.** Found while checking
  SH-11.6's output: the harness prepends the `test` subcommand to a manifest's `testArgs`, but nine
  of the eleven manifests already begin with `test`. The result was `cargo test test -p ...`, in
  which the second word is not a subcommand but a TESTNAME filter -- a sweep claiming to run a
  package's suite while running a subset of it, the same false-green this tool exists to prevent.
  The vector is now normalised so either manifest spelling produces one `test`.
  **No prior verification was weakened**: every crate swept so far keeps its tests under a
  `mod tests`, so the accidental filter matched all of them, and all fourteen recorded baselines
  report "0 filtered out". The defect was latent, and would have appeared on the first crate laid
  out differently.

## M12: PR #56 seventh review round

- [x] **SH-12.1** -- **The banner undercounted the machine it was about to measure.**
  `Fingerprint::processors` is documented as the logical-processor count but was summed over
  core-domain membership, which agreed with that meaning only while every processor was guaranteed to
  sit in a core domain. SH-11.1 stopped guaranteeing it: `places_from_topology` now explicitly accepts
  a topology naming no cores and places every online processor. The banner consequently read
  `0p/0c` for a machine the measurement was about to use four processors on -- a defect this branch
  created rather than inherited.
  The count is now read off `topology.processors` with the same `online` filter the placement applies,
  so the summary counts exactly what the measurement will use. `cores` deliberately still counts core
  domains: zero there is the honest report that the topology named none.
  Sabotage-verified. Two new tests pin both directions -- an uncored processor is still counted, an
  offline slot is not -- and each asserts equality against `places_from_topology`'s own output rather
  than a literal, so the two cannot drift apart again.

- [x] **SH-12.2** -- **Two consequences of that fix, found by sweeping it rather than reported.**
  `cache_domain_sizes` fills itself with the processor count when no cache level partitions the host,
  so the bare-machine render silently improved from `L-[0]` to `L-[4]`.
  `numa_node_sizes` did not, and now does not sum to `processors` in that one case: it reports the
  nodes the topology *named*, and a bare topology names none, while every placement still reports the
  documented node-`0` default. Keeping it that way is deliberate -- the cache list can afford to fill
  itself because `L-` marks the absence, and `numa[4]` would be indistinguishable from a host that
  genuinely reported one node of four. The behaviour is now documented on the field and pinned by a
  test that asserts the whole render, including the `!!SYNTHETIC!!` provenance marker. The stronger
  fix needs a marker, which is a serialized field and so a schema bump, tracked as
  [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) `PT-6.2`.

## M13: PR #56 eighth review round

- [x] **SH-13.1** -- **A record could splice two machines together.** The tool announced a shape read
  at one instant while `core_affinity::measure` discovered again at another, so a processor going
  offline -- or moving group or node -- between them produced a record whose `host` described one
  machine while every row was measured on a different one. Nothing in the file said so, and the host
  is precisely what a reader interprets row sets *through*.
  `measure` now reports the shape it actually ran on, as `Observation::host`, which is the fix that
  keeps the anti-synthetic boundary intact: the measurement still discovers for itself and no seam
  accepts a fabricated shape from outside. `SubmissionRecord::new` refuses when the announced and
  measured hosts differ, so the splice is unrepresentable rather than merely avoided at the one
  current call site; the tool checks first anyway and reports the disagreement in terms a runner can
  act on. Refusing rather than silently recording the measured shape, because the notice is what the
  runner consented to.

- [x] **SH-13.2** -- **The tool wrote from 54 independent print sites.** The repository's
  one-output-sink rule requires an output abstraction at the *first* output site so the storage
  target and the formatting stay separable from the call sites that compose content. This binary had
  none, which is why its collection notice -- a disclosure a runner reads before agreeing to publish
  facts about their machine -- could only be exercised by running the process and capturing stdout.
  A `Sink` trait now carries the two streams the tool genuinely has, `print_collection_notice` and
  `print_plan` became `render_*` functions returning a `String` (matching the idiom the record report
  already used), and `main` is the only place that names the real streams.
  Verified as a pure refactor by comparing the built binary's output before and after: `--preview`
  and `--help` are **byte-identical**, and `--version` differs only by the build identity correctly
  reporting the working tree as `DIRTY`. Eight new tests cover what was previously unreachable,
  including that the notice shows the model rather than describing it, that a withheld model reads
  differently from one the host would not report, and that the two streams cannot satisfy each
  other's assertions.

- [x] **SH-13.3** -- **The two new probes wrote from 94 independent print sites between them.** Same
  rule as SH-13.2, in [core_affinity.rs](crates/windows-platform-probes/src/bin/core_affinity.rs)
  (67 sites) and [doorbell_cost.rs](crates/windows-platform-probes/src/bin/doorbell_cost.rs) (27).
  A `Report` sink now lives in [report.rs](crates/windows-platform-probes/src/report.rs), shared by
  both. One stream, not two: unlike the placement probe these have only ever written to stdout, and
  inventing a diagnostic stream they do not use would be adding a distinction the tools do not make.
  Each `main` is now three lines -- measure, render, emit -- and is the only place naming the real
  stream.
  One find during the conversion that the mechanical part would have missed: `render` called
  `fingerprint::print_banner()`, which writes to stdout *itself*. Left alone it would have put the
  identifying line on the terminal while leaving it out of the returned report, so a captured report
  would be missing the one line saying which machine produced it -- and the `!!SYNTHETIC!!` taint
  marker with it. `banner_line()` already existed for exactly this and is now used.
  Verified as a pure refactor by running both probes before and after and comparing with numerals
  masked (their output is timing-dependent, so byte equality is not available): 38 lines and 50 lines
  respectively, **structurally identical** both times.

- [ ] **SH-13.4** -- **The other twelve probes still print directly, and now there is a sink to
  adopt.** `probe-peer-index-cache` (55 sites), `probe-request-cost` (45), `probe-topology` (32),
  `probe-queue-contention` (27), `probe-ioring` (24), `probe-completion-port` (22),
  `probe-worker-context` (22), `probe-device-map` (21), `probe-cancel-io` (19),
  `probe-pool-growth` (16), `probe-handle-state` (14), `probe-error-mode` (10) -- 307 sites.
  Deliberately **not** done in the review round that introduced the sink: those probes predate it and
  are outside that round's scope, and each conversion needs its own before/after comparison against
  the probe's real output, which is what makes it a refactor rather than a rewrite.
  Queued rather than left as a note precisely because a half-adopted abstraction is the state most
  likely to be forgotten -- the next probe author will see twelve neighbours printing directly and
  reasonably conclude that is the house style.

## M14: PR #56 ninth review round -- an ABA hole the wrap test would not have caught

Two findings, both verified by derivation against the source. They are **not** the wrap gap SH-6.1
already tracked: that item is about *testing* the wrap, and its planned stress would not have found
either of these, because neither requires the wrap alone -- each requires a producer to remain
stalled **across** it, inside a window a few instructions wide.

**The shared shape of both.** A producer decides a slot is writable, is suspended, and resumes after
the claim counter has made a full lap back to the bit pattern it read. Its compare-exchange then
succeeds against a value that is numerically equal but logically a whole generation later, and the
decision it is about to act on was made against the *earlier* generation. The exchange protects the
counter; nothing protects the decision.

- [ ] **SH-14.1** -- **`reserving_mpsc` can overwrite a live slot after 2^32 pushes, on every
  target.** `POSITION_BITS` is 32 by construction -- the claim word is a `u64` split into a 32-bit
  reservation count and a 32-bit position -- so this does **not** depend on a 32-bit `usize` and is
  not a 32-bit-only concern.
  The sequence: a producer reads `word = (reserved, position)` and calls
  `has_room_beyond_reservations`, which reads `head` and returns room. It stalls. Other producers
  claim 2^32 positions, wrapping `position` back to the value it read; with `reserved` at its steady
  state (commonly zero), the *whole word* recurs. The stalled producer's
  `compare_exchange_weak(word, ...)` now succeeds, and it publishes into a slot whose room was
  decided against a `head` that has since advanced -- so the slot may hold an item the consumer has
  not taken. The SAFETY comment above `publish` ("no other producer can also have claimed [this
  position]") remains true and is not the property that fails; the failing property is that the slot
  was free.
  2^32 pushes is **37 seconds to about four minutes at this crate's measured rates** (SH-6.1 carries
  the derivation), so the window is not exotic -- it needs an unlucky stall, not an unreachable one.
  Two producers is the relevant figure at two minutes, since the hazard needs a second producer to
  advance the counter while the first is held; the 37-second single-producer number is the ceiling on
  how fast this counter can be driven at all, not a rate at which the bug can fire.

- [x] **SH-14.2** -- **`slotwise_mpsc` had the same hole on a 32-bit target.** Its positions were
  `AtomicUsize`, which is 32 bits there. A producer that has observed `sequence == position` -- the
  slot is free -- and then stalls across a full lap resumes to find the same `tail` bits, succeeds
  at `compare_exchange_weak(position, position + 1)`, and writes a slot that may now hold a live
  item from the previous lap of the ring. The sequence observation that made the write safe is never
  re-checked, and the exchange covers only `tail`.
  This is the finding that also falsified SH-6.1's claim that this shape "cannot be driven there at
  all" -- corrected in place.
  **Fixed by widening the counter rather than by narrowing the platform.** Positions and slot
  sequences are now a named `Position = u64` on every target, so the lap needs 2^64 claims and cannot
  be reached. On 64-bit this is exactly what `usize` already was; on 32-bit the exchange becomes a
  64-bit one. Verified on a real 32-bit target rather than argued: the suite passes under
  `i686-pc-windows-msvc` (290 tests), and a probe there reports `target_has_atomic = "64"` with
  `AtomicU64` **lock-free** -- so the fix costs a `cmpxchg8b`, not a hidden mutex, which is the
  outcome that would have made this a bad trade.
  `producers` stays `AtomicUsize`: it is a handle refcount, not a position, and nothing compares it
  against one.

- [ ] **SH-14.3** -- **Decide the fix, which is a design decision rather than a patch.** Recorded
  here so the options are not re-derived, with what each costs:
  1. **Widen the counter so it cannot lap.** For `slotwise_mpsc` this is `AtomicU64` instead of
     `AtomicUsize`, which is free on 64-bit and costs a `cmpxchg8b`-class operation on 32-bit x86.
     For `reserving_mpsc` there is **no room**: 32 position bits plus 32 reservation bits is exactly
     the 64-bit word, and `MAX_RESERVED` must cover `BOUNDS_MAX` (2^31), so no generation field can
     be carved out without narrowing the capacity the shape offers.
  2. **Narrow `reserving_mpsc`'s capacity to buy generation bits.** A smaller `BOUNDS_MAX` frees
     bits in both halves. This does not *eliminate* the lap, it lengthens it -- any finite field
     wraps -- so it is a mitigation whose adequacy has to be argued rather than a fix.
  3. **Re-validate after the claim.** Cheap to say, hard to do: once the exchange succeeds the
     position is claimed, so there is no safe way to back out without a second protocol.
  4. **Drop 32-bit support explicitly** -- resolves SH-14.2 only, and leaves SH-14.1 untouched
     because that one is target-independent. This narrows the platform, so per the repository's
     platform-integrity rule it is the engineer's decision and not one to take in passing.
  Whatever is chosen, the property is not observable from a test that merely crosses the wrap: it
  needs a producer *held* between its decision and its exchange, which means a deliberate seam --
  the crate's existing race hooks (`ARM`, `CLEAR`, `CLAIM`) are the shape of what is needed.

- [x] **SH-14.4** -- **Every statement of the wait protocol was missing its last step.** Five findings,
  one cause. `blocking::recv` has always had four steps -- pop, `arm`, **check disconnection and take
  one last time**, wait -- but the trait contract, all three shapes' `arm` docs, two worked examples,
  and the README each described the three-step form. A caller following any of them waits forever at
  the end of the stream: the last producer's drop rings the doorbell **once**, `arm` clears precisely
  that ring, and with no producer left nothing rings it again.
  The root defect is a contract that overstated itself. `arm` answers exactly one question -- can a
  later *push* be missed -- and on a producerless queue the answer is trivially no, so it returns
  `true`. Documented flatly as "safe to wait", that is an invitation to hang. It now says what it
  measures, and the four-step protocol is stated on `Waitable::arm` with the other statements
  pointing at it rather than paraphrasing it again.
  Pinned by `arm_reports_safe_to_wait_on_an_empty_disconnected_queue`, which asserts both halves --
  `arm` returns `true`, *and* the doorbell is dark afterwards -- so the exception is bound to
  observable behaviour rather than to prose. The final `pop` is likewise not belt-and-braces: a
  producer may push *and then* drop between the drain and the check, which is what `Parked::finish`
  exists for.
  The README is now compiled as a doctest (`cfg(doctest)`, matching three sibling crates). It carries
  no code today, so this compiles nothing -- it is there so the first example somebody adds is
  compiled rather than trusted, this round being the demonstration that prose nothing executes rots.

## M15: the claim protocol, prototyped rather than argued (SH-14.3's decision)

**This milestone exists to answer SH-14.3 with a measurement.** SH-14.1 is a real correctness hole
and SH-14.3 lists four ways out, all of which either lengthen the lap or narrow the platform. Prior-art
research (recorded in the design note item below) found a fifth shape that removes the hazard by
construction, and a sixth that additionally changes the queue's progress condition. Neither can be
chosen on reasoning alone, because [D-26](crates/windows-waitable-queues/DESIGN-NOTES.md#d-26)
already measured that the single shared line is what collapses under contention -- so an "obviously
cheaper" claim protocol that touches two shared lines instead of one may well be slower.

**Built as duplicated paths, per the repository's platform-integrity rule.** Neither arm modifies
`reserving_mpsc`. The shipping shape keeps working and keeps its tests green while the speculative
ones are proven or discarded, and the merge-or-delete decision is SH-15.6 rather than something that
happens by drift.

**The principle both arms are instances of**, stated once so it is not re-derived: *the atomic
operation that authorizes the write must cover everything the decision depended on.* Today's protocol
decides "there is room" from a separately-read `head` and then compare-exchanges only the claim word,
so a full recurrence of the 32-bit position field revalidates nothing. Every fix below closes that
gap; the options in SH-14.3 instead make the recurrence harder to reach.

- [x] **SH-15.1** -- **Record the prior-art research as a design note before it is lost.** The survey
  is the reason this milestone exists and none of it is currently written down. It must capture: that
  `crossbeam-queue::ArrayQueue`, `concurrent-queue` and `thingbuf` all use our protocol shape and none
  re-validates after its compare-exchange; that all three are saved only by putting the whole counter
  in one `usize`, so all three carry the identical exposure on a 32-bit target; that Nikolaev's SCQ
  (DISC 2019, open access, DOI `10.4230/LIPIcs.DISC.2019.28`, section 3 "ABA safety") states the width
  assumption the field relies on and states it for **CPU-word width**, which a 32-bit subfield does not
  satisfy; that DPDK's `rte_ring` is the closest published twin of our exact protocol and its published
  justification (Programmer's Guide 6.5.4) covers modular *arithmetic* only, not lap recurrence; and
  that SCQ and CRQ both fix it structurally by making the counter an unconditional fetch-and-add and
  moving the authorizing compare-exchange onto the cell.
  Also correct the exposure figure, which is currently wrong in both SH-6.1 and SH-14.1: at the crate's
  own measured 8.6 ns/push the wrap is **37 seconds**, not "about two minutes". Two minutes is the
  two-producer figure and roughly four the 32-producer one; since the hazard needs at least two
  producers the headline is defensible, but the range and its basis belong in the text.

- [x] **SH-15.2** -- **Amend [D-18](crates/windows-waitable-queues/DESIGN-NOTES.md#d-18), whose stated
  rationale no longer holds.** It refuses a 128-bit compare-and-swap because it "would lift the 2^31 cap
  and nothing else", which was written before SH-14.1 was known -- a 64-bit position would also collapse
  the recurrence, so the decision denies the existence of its main benefit.
  **Checked against the pinned toolchain rather than against documentation**, because two of its three
  supporting costs turned out to disagree with it. `rustc 1.98.0 --print cfg` reports, per target:
  `x86_64-pc-windows-msvc` emits `target_feature="cmpxchg16b"` **and** `target_has_atomic="128"`;
  `aarch64-pc-windows-msvc` emits `target_has_atomic="128"` with no target feature required;
  `i686-pc-windows-msvc` emits `target_has_atomic="64"` and **no** `"128"`.
  So: the claim "`x86_64-pc-windows-msvc` does not enable the target feature by default" is **false**
  on 1.98 -- there is no floor to raise and no runtime detection to pay. The claim "there is no usable
  `AtomicU128`" is **true** and verified (still unstable, rust-lang/rust#99069), so the dependency cost
  stands. And the decisive new fact D-18 never had: **a 128-bit claim word cannot work on `i686` at
  all**, so that option is not "widen the word", it is "widen the word *and* drop 32-bit support" --
  which collapses SH-14.3 option 1 into option 4 and makes it the engineer's call under the
  platform-integrity rule. Amend rather than reverse: the refusal may well stand, but every reason
  currently given for it is either wrong or incomplete.

- [x] **SH-15.3** -- **Arm A: the central-permit claim, as a duplicated shape.** Admission becomes a
  single atomic on one `permits` counter initialised to the capacity, and the position degrades to a
  pure ticket (`fetch_add`, which has no predicate and therefore cannot be revalidated wrongly). A
  producer holding a permit and taking ticket `p` has `p - head <= capacity - 1` by counting, so its
  slot is provably free and the position may wrap freely. This satisfies `reserving_mpsc`'s own stated
  requirement -- "two independent claimants on one resource must synchronise on one location" -- with
  the permit counter as that location, so it strengthens the existing argument rather than contradicting
  it. Reservations map directly: a reservation is a permit held across time, still taking no position,
  so an outstanding one reduces capacity without head-of-line blocking the stream.
  **Not claimed to be non-blocking.** A preempted ticket-holder still stalls the consumer at its
  position; this arm fixes the ABA hole and nothing about the progress condition.

- [x] **SH-15.5** -- **Measure arm A against the shipping shape in `probe-queue-contention`.** The
  probe deliberately measures the real shapes rather than stand-ins ("a stand-in would only measure
  itself"), so the arm must be a real module in the queue crate for this to mean anything. Report both
  regimes: isolated for the claim cost alone, drained for what the shared line costs when a consumer is
  writing it. The question this answers is narrow -- does removing the room-decision race cost
  throughput, given that arm A touches two shared lines on the push path where today's shape touches
  one plus a read.

- [ ] **SH-15.5.1** -- **Settle why the two shapes' refusal counts differ by orders of magnitude,
  because SH-15.6 cannot be decided without it.** In the drained regime `permit_mpsc` recorded
  roughly 460,000 refusals at eight producers where `reserving_mpsc` recorded 0, and the counts are
  unstable across runs (`reserving_mpsc` itself recorded 0 and then 2,363 for the same
  configuration). Two candidate explanations, which the current harness cannot separate: the permit
  shape is genuinely faster, so it attempts more pushes against a full queue and is refused more
  often as a consequence; **or** its optimistic overdraw refuses near-full more readily than the
  shipping shape's re-read of the claim does, in which case adopting it would change how eagerly a
  caller sees backpressure.
  The distinction matters and is not cosmetic. `reserving_mpsc` re-reads the claim and retries before
  reporting `Full`, so it refuses only when the queue was genuinely full at an instant it observed.
  If the permit shape refuses more eagerly, that is a **behavioural change to a public contract**,
  and per D-34's own criterion it must be stated rather than discovered by a caller.
  Measure refusals per *attempt* rather than per run, at a fixed attempt count with the consumer's
  drain rate pinned, so throughput and refusal rate are separated. A test that admits exactly
  `capacity` items from N concurrent producers into an initially empty queue would also settle the
  narrow question of whether an overdraw can refuse while a slot is provably free.

- [ ] **SH-15.6** -- **Decide: merge, or delete.** The duplicated path exists so the speculative work
  could proceed without disturbing a working shape; leaving it to become permanent by inattention is
  the failure mode the duplication rule warns about. On the evidence from SH-15.5, either adopt arm A
  into `reserving_mpsc` (closing SH-14.1 and SH-14.3) or delete it and take one of SH-14.3's original
  options, recording why. Whichever way it goes, SH-14.1's hazard must be either fixed or documented as
  an accepted limitation with its exposure stated -- it may not simply stay open.
  **Gated on SH-15.5.1**, not on SH-15.5: the throughput question is answered
  ([D-35](crates/windows-waitable-queues/DESIGN-NOTES.md#d-35) -- 2.7x faster at 16-32 producers,
  1.45x slower at one), but adopting a claim that reports backpressure more eagerly would be a
  behavioural change to a public contract, and that is not yet known either way.
  Note also what the measurement did **not** cover, so adoption does not quietly assume it: the
  permit shape has no `Waitable`/`Observable`/`Reserving` trait impls, no `Options`/disposal
  integration, no high-water tracking, no race hooks, and no 32-bit run. Merging means writing all of
  those, so the merge is a milestone rather than a rename.

- [ ] **SH-15.7** -- **Build the stall seam that can actually witness the bug.** SH-14.3 already notes
  the property is invisible to a test that merely crosses the wrap: it needs a producer *held* between
  its room decision and its exchange. The crate's existing race hooks (`ARM`, `CLEAR`, `CLAIM`) are the
  right shape. Without this, every arm above is argued rather than demonstrated, and the fix that is
  adopted has no regression test that would go red if it were reverted.

## M-inf: parked, ungated

- [ ] **SH-inf.1** -- **The per-cell cycle claim (SCQ's shape), which is the non-blocking one.** The
  shared counter becomes an unconditional fetch-and-add authorizing nothing, and the reuse decision plus
  the write are validated together by a compare-exchange on the slot's own `{cycle, safe}` word --
  removing the shared decision rather than moving it, and making the queue genuinely lock-free rather
  than merely ABA-free.
  **Parked deliberately, and the reason is a real constraint rather than scheduling.** In-order delivery,
  inline item storage, and non-blocking progress are over-constrained together: storing `T` in the ring
  forces a producer to claim a slot and then write it, so a preemption between the two necessarily stalls
  in-order consumption. SCQ escapes this only because it queues *indices* -- the payload is written
  outside the queue protocol -- which is a different data structure from the one this crate offers. So
  this is not an improvement to today's shapes but a fourth shape with different semantics, and it is
  worth having only alongside a decision that some caller wants non-blocking progress more than it wants
  inline storage.
  Related: the whole family this crate belongs to is **technically blocking**, which is worth stating
  plainly somewhere public. wCQ (Nikolaev and Ravindran, SPAA 2022) says so of exactly this shape --
  queues that "require a thread to reserve a ring buffer slot prior to writing new data ... are
  technically blocking since one stalled (e.g., preempted) thread in the middle of an operation can
  adversely affect other threads" -- and names DPDK's ring as a case "erroneously dubbed as 'lock-free'".
  This crate should not repeat that error by implication; see SH-15.1, which records the survey.
