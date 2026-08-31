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

## M2: repair the release plumbing before relying on it

- [ ] **SH-2.1** -- **Add `windows-waitable-queues-v*` to the tag trigger list in
  [.github/workflows/publish-crate.yml](.github/workflows/publish-crate.yml).** It is missing. The
  crate *is* registered with release-please, so release-please will happily raise the release PR and
  push the tag -- and then nothing will publish it, with no error, because no workflow matches the tag.
  **This is a silent failure, which is why it is its own item**: the symptom is a tag that exists, a
  changelog that looks right, and a crate that never appears on crates.io.

- [ ] **SH-2.2** -- Plan the **`windows-topology-sys` 0.2.0 ripple**. `windows-ioring-sys` is published
  and pins `windows-topology-sys = "0.1.0"`, so the breaking bump obliges updating that dependency and
  releasing `windows-ioring-sys` too. Decide the order and whether ioring's release is part of this
  push or follows it -- but decide it, because a workspace that builds locally via `path` dependencies
  will not reveal this and the first symptom is a consumer unable to resolve the two together.

- [ ] **SH-2.4** -- Clear the **eight rustdoc warnings** in `windows-waitable-queues` before it is
  published. They pre-date this branch and were found while doing SH-1.1: an unresolved link to
  `MIN_CAPACITY`, six links from public documentation to private items (`Shared::len`,
  `Doorbell::clear`, `Doorbell`, `BOUNDS`), and one redundant explicit link target.
  Ordinarily out of scope for the item that found them, and in scope here for one reason: **docs.rs is
  the face of a first release.** A link that silently resolves to nothing in a workspace build renders
  as a dead or missing reference to the first person who ever reads these docs, and a link to a private
  item points at a page they cannot open.

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
  **UNBLOCKS half of [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md), which is gated on both
  releases. On completing this, update that file's gate note to record that topology has shipped** --
  the gate lifts only when SH-4.3 lands too, and a half-lifted gate that reads as lifted is how work
  starts against a dependency that is not there yet.

- [ ] **SH-4.2** -- Update `windows-ioring-sys` to depend on the published 0.2.0 and release it, per
  the order settled in SH-2.2.

- [ ] **SH-4.3** -- Release `windows-waitable-queues` 0.1.0, with SH-2.1's fix in place. Confirm the
  tag triggered a publish rather than assuming it did.
  **LIFTS THE GATE ON [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md). On completing this,
  edit that file's opening gate paragraph to say the gate is lifted and name the two published
  versions**, so a reader arriving there later does not have to reconstruct whether it still applies.
  Blocked by SH-1.1, and by M31.6 as well if SH-1.2 decided that it gates.

## M5: verify from outside the workspace

- [ ] **SH-5.1** -- In a scratch project **outside this repository**, depend on both crates from
  crates.io and build something that uses each. This is the first exercise of the crates as
  *dependencies* rather than as path members, and it is where a missing `version` on a path dependency,
  an unexported type, or a feature that only resolves inside the workspace will show up.

- [ ] **SH-5.2** -- Confirm the published `windows-topology-sys` still reports `Provenance::Measured`
  from `discover()` when consumed as a dependency, and that a `Topology::default()` is `Synthetic`.
  The provenance rules are the newest thing in the crate and the least exercised outside it.
