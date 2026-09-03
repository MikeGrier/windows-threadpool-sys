# Checklist: ship the topology and queue crates

**Goal.** Get `windows-topology-sys` 0.2.0 and `windows-waitable-queues` 0.1.0 released, so the
placement tool in [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) has something to build
against and other people can run it on hardware this workspace does not own.

## Where this stands

The release has not happened. PR #56 has been open as a **draft** since 2026-08-31 and is 221 commits
ahead of `main`.

**Milestone numbers are not a running order.** M7 through M15 are *review rounds on PR #56*, so they
happened -- and continue to happen -- **inside M3**, between the pull request opening and a merge
that has not occurred. Reading the file top to bottom puts the review of a pull request after the
merge that closes it, which is backwards. Only M1 through M6 are a sequence.

| Milestone | State | What it is waiting on |
|---|---|---|
| M1 settle the public surface | **done, archived** | -- |
| M2 repair the release plumbing | 1 of 5 open | only SH-2.3, which needs the merge commit |
| M3 land the branch | 4 of 5 open | now gated on M16; SH-3.1.1 runs after the model lands |
| M4 release | open | M3 |
| M5 verify from outside | open | M4 |
| M6 long-running validation | open | gates SH-4.3, so it gates the queue crate's publication |
| M7-M13 review rounds | **done, archived** | -- |
| M14 ninth review round | 1 open | SH-14.1, the ABA defect; disclosed at SH-15.8, fix is M15 |
| M15 the claim protocol | 5 open | SH-15.6 is the decision; gated on SH-15.5.1 |
| M16 tenth review round | 7 done, 6 superseded | its own findings are fixed; the model work moved to `MMT-*` |
| M-inf parked | ungated | not scheduled, deliberately |

**The critical path is M16's locality-model work -> SH-3.1.1 -> SH-3.4 -> M4.** M14 and M15 do not
block it: SH-14.1 ships disclosed rather than fixed
([D-36](crates/windows-waitable-queues/DESIGN-NOTES.md#d-36)) and the disclosure -- which was the
actual release blocker -- landed at SH-15.8, so both conclude after 0.1.0 provided the pull request
**says** that is deliberate. SH-3.1.1 owns saying it.

**M16 is different, and this was decided rather than drifted into.** Its four gated items
(SH-16.5, SH-16.8, SH-16.9, SH-16.10) are one piece of work -- replacing the locality model,
consuming CPU Sets, and collapsing three restatements of one rule -- and the decision is that
**PR #56 does not merge until it lands**. They were briefly listed here as non-blocking; that is
corrected. It is gated in turn on
[DESIGN-SESSION-2026-09-02-cache-locality-model.md](design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md),
which has open questions, so **design concludes before implementation starts**.

What blocks the queue crate specifically, and separately, is M6.

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
- `windows-ioring-sys` **0.2.0 is published and pins `windows-topology-sys = "0.1.0"` -- but as a
  dev-dependency**, so consumers never resolve it and the pin obliges no release. Corrected at SH-2.2,
  which was written on the opposite assumption.
- This branch was **54 commits ahead of `main` with no pull request** when this file was written.
  **As of 2026-09-02 it is 221 commits ahead, and PR #56 has been open (as a draft) since
  2026-08-31.** Release automation runs on `main`, so nothing ships until it merges -- but the
  pull request itself is no longer the thing to create, and the review rounds in M7 onwards all
  happened on it while it sat open.

> **M1 -- settling the public surface before publication -- is complete and archived.** Moved to
> [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) on 2026-09-02.

## M2: repair the release plumbing before relying on it

**Read in file order, not numeric order.** These items are in dependency order, and the numbers are
historical -- SH-2.5 was split out of SH-2.2 and sits beside it, while SH-2.3 is last because it
needs the merge commit. Renumbering would break the references already in commit messages, so the
order is the authority and the numbers are only names.

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

- [x] **SH-2.2** -- Update the three `windows-topology-sys = "0.1.0"` pins when 0.2.0 ships.
  **RE-PLANNED: this item asked for a decision whose premise was false, and the decision it demanded
  does not arise.** It said `windows-ioring-sys` is published against the old topology, "so the
  breaking bump obliges updating that dependency and releasing `windows-ioring-sys` too", and that
  "the first symptom is a consumer unable to resolve the two together". Neither holds.
  **`windows-ioring-sys`'s topology dependency is a dev-dependency.** Cargo reports
  `kind=dev`, and the crate's `src/` never names it -- only four files under `examples/` do. Cargo
  does not resolve dev-dependencies *of* dependencies, so a consumer of `windows-ioring-sys` never
  sees `windows-topology-sys` at all. **Verified rather than reasoned**: a scratch crate depending on
  the published `windows-ioring-sys 0.2.0` resolves `windows-sys`, `windows-link`,
  `windows-threadpool-sys` and `windows-overlapped-io-sys`, and no topology crate. There is no
  resolution conflict to avoid and **no ioring release is obliged**, so the ordering question this
  item existed to settle is empty.
  **RESOLVED 2026-09-02: there are no pins left to update.** All three were deleted rather than
  maintained, once it turned out none of them was doing anything a reader would want:
  - `windows-placement-probe` and `windows-platform-probes` -- decided never to be published to a
    registry, so **every** workspace dependency is path-only now. Eight version fields deleted
    between them, not merely the two topology ones. See those crates' DESIGN-NOTES and `PT-5.3`
    (reversed).
  - `windows-ioring-sys` -- a `[dev-dependencies]` entry, and cargo omits a versionless
    dev-dependency from the published manifest entirely. Deleted as SH-2.5, which also records why
    the pin was never checked by anything.
  **The whole hazard class is gone, verified rather than argued.** Bumping topology to 0.2.0 now
  leaves `cargo metadata` resolving cleanly; before today it failed in three places, and
  release-please could only have fixed one of them.
  **The gate on SH-3.4 is therefore lifted** -- there is nothing here that had to wait for topology's
  manifest to move, because nothing here needs a version at all.
  **One consequence to watch for, not to pre-empt.** The `cargo-workspace` plugin bumps dependents of
  a bumped package. If it treats the (now versionless) dev-dependency as grounds to bump
  `windows-ioring-sys`, an ioring release will happen -- not because one is obliged (it is not; see
  above) but because the tooling produced one. That is acceptable if it occurs; it is only a problem
  if it is mistaken for evidence that the obligation existed after all.

- [x] **SH-2.5** -- **Resolved by deleting the pin, and neither of the two fixes this item proposed
  was needed -- because its premise was wrong.** It asserted that "at `cargo publish` the
  verification build resolves the `version` requirement from crates.io", so a divergence would
  surface as a failed publish. **Publish verification does not build examples or tests.** Measured by
  packaging this crate: the verification step compiled the library and its real dependencies and
  nothing else -- not even `serde_json`, which is a versioned dev-dependency the examples need. So
  the pin was never exercised at the one moment it was supposed to matter, and no publish could ever
  have failed for this reason.
  **What the pin did do was break the workspace**, which is the opposite of the hazard as filed. See
  SH-2.2: cargo enforces `path` + `version` agreement at every build regardless of dependency kind,
  so this pin against a bumped topology failed `cargo metadata` for every crate here.
  **The fix is to delete it, and cargo cooperates**: a versionless dev-dependency is omitted from the
  published manifest entirely -- verified by packaging with the version removed and reading the
  result, where `windows-topology-sys` appears nowhere. Full `cargo package` with verification then
  succeeds. So there is no crates.io requirement left to diverge *from*, and the residual hazard the
  item was really about disappears rather than being managed.
  What is given up is that the examples are not buildable from the packaged tarball, which they were
  only ever incidentally: publish never checked them, and anyone reading an example does it from a
  checkout, where they build as before.
  **This was the last version pin in the workspace that a topology bump could break.** Verified: with
  it gone, `cargo metadata` against a topology bumped to 0.2.0 resolves cleanly.
  **The workspace now holds an invariant worth naming, because it is what makes the hazard stay
  gone:** every crate that still carries a versioned `path` dependency is one release-please manages
  -- `windows-file-enumeration-sys`, `windows-file-watcher`, its example harness,
  `windows-ioring-sys`, `windows-namespace-request-sys`, `windows-thread-ambient-sys` and
  `windows-threadpool-sys`, thirteen pins between them. Those pins are genuinely needed (each is a
  published crate depending on a published crate) *and* the `cargo-workspace` plugin exists to
  rewrite exactly them. Every pin the plugin could **not** see is now gone.
  A pin outside the plugin's `packages` map is the shape that breaks `main`, so if one is ever added,
  it should be questioned rather than maintained. Checking that mechanically would suit
  [tools/check-publishable.ps1](tools/check-publishable.ps1), which already reads both the
  release-please config and the manifests -- **not done here**, since it adds CI surface and this
  item was scoped to the pin.

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

**Read this milestone as interleaved with M7 onwards, not before them.** The file's linear order
implies the review rounds follow the merge, which is backwards and was noticed on 2026-09-02: PR #56
opened on 2026-08-31, nine review rounds arrived while it sat open, and the merge has still not
happened. Review rounds are **reactive** -- they cannot be scheduled after SH-3.4, because merging
ends the pull request they are rounds *of*.

**Which of those rounds gate the merge: M16 does, M14 and M15 do not.** SH-14.1 is a real defect
that ships **disclosed rather than fixed**
([D-36](crates/windows-waitable-queues/DESIGN-NOTES.md#d-36), delivered by SH-15.8), and everything
open in M15 is follow-on work on the fix. What *did* gate the release was the disclosure, and that
landed. So SH-3.4 may proceed with M14 and M15 still open -- but a reviewer must be told that is
deliberate, which is SH-3.1.1's job below.

**M16 is the exception, by decision.** Its locality-model work (SH-16.5, SH-16.8, SH-16.9,
SH-16.10) is a merge blocker: the model it replaces is the one `windows-topology-sys` 0.2.0 would
publish, and shipping a public surface that is already known to be the wrong shape is what the
milestone exists to avoid. So SH-3.4 waits on it.

**Updated 2026-09-03 -- what that work now is, and what discharges the gate.** All six of those items
are superseded into the `MMT-*` plan in
[crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md), so the gate is
discharged by **MMT M2 through M5 landing in this PR**, which is the engineer's direction. Two things
that previously stood in the way are gone:

- **The design session no longer gates it.** The session's open questions were answered as `D-13`
  through `D-21` in
  [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md); the
  `MMT-*` plan is what it produced. Read "the MMT plan concludes" wherever the earlier wording said
  "the session concludes".
- **The reshape no longer waits on the planner.** [D-21](crates/windows-topology-sys/DESIGN-NOTES.md#d-21)
  establishes that `windows-topology-sys` publishes a refined view of what the platform publishes,
  with an adapter absorbing the planner's needs -- so the reshape is self-justified. `topology-planner`
  contributes only planning documents to this PR and is deliberately deferred past it.

- [x] **SH-3.1** -- ~~Open the pull request~~ **-- already open since 2026-08-31 as a draft.** Checked
  off as *superseded by events*, not as done: the item asked for something that had already happened
  by the time anyone read it, and it stated "54 commits" against a branch now **221 commits** ahead.
  Its surviving instruction is **SH-3.1.1** below, which is the part that was never done.

- [ ] **SH-3.1.1** -- **Review the PR as a diff rather than as a memory of having written it, then
  mark it ready.** 221 commits across the topology crate, the queue crate and the probes is far more
  than fits in a session's recollection, and the branch contains at least one deliberate breaking
  change plus several documented reversals of earlier conclusions -- D-18 amended and then
  superseded, PT-5.3 reversed, SH-14.3 absorbed, and a crate's version scheme changed from semver to
  a date.
  **Taking it out of draft is a step this file never named**, and it is the real gate on SH-3.4
  rather than a formality: a draft cannot be merged, and nothing above says who decides it is ready.
  **The description must state what is knowingly unfinished**, so a reviewer does not read open
  milestones as oversight: SH-14.1 ships disclosed per D-36, M15 is follow-on work on its fix, and
  `permit_mpsc` is an experimental non-default module exempt from the crate's semver promise.
  **Gated on M16's locality-model work**, which is in scope for this PR by decision. The description
  cannot be written before then without being wrong twice over: it would omit the largest change in
  the branch, and it would list the locality model among the deferred things when it is not deferred.
  So this item now runs *after* SH-16.5/16.8/16.9/16.10, not before them.

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
  **The gate this used to hold over SH-2.2 is lifted** -- that item is closed, having had nothing
  left to do once the pins were deleted rather than maintained.
  **No longer carries a pin hazard.** An earlier version of this item warned that the PR must not be
  merged until `windows-ioring-sys`'s topology pin read 0.2.0, because a `^0.1.0` requirement against
  a path crate at 0.2.0 fails resolution and would have landed a red `main`. That was true when
  written and is now moot: **all three such pins were deleted on 2026-09-02** (SH-2.2, SH-2.5), and a
  topology bump was re-verified to resolve cleanly with none of them present. Nothing in the release
  PR needs checking beyond the version number itself.

## M4: release

- [ ] **SH-4.1** -- Release `windows-topology-sys` 0.2.0 and confirm it appears on crates.io and builds
  on docs.rs. Docs.rs builds under its own configuration, so a crate that documents locally can still
  fail there.
  **The obligation to [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) is void as of
  2026-09-02.** This item used to half-unblock PT-5.3 -- publishing the tool to crates.io -- and that
  decision was reversed: the tool is never published to a registry, so there is nothing here to
  unblock and no gate bullet to edit. It never gated the tool's **GitHub binaries**, which CI builds
  from this repository through `path` dependencies, and those remain the only distribution.

- [ ] **SH-4.2** -- Update `windows-ioring-sys` to depend on the published 0.2.0 and release it, per
  the order settled in SH-2.2.

- [ ] **SH-4.3** -- Release `windows-waitable-queues` 0.1.0, with SH-2.1's fix in place. Confirm the
  tag triggered a publish rather than assuming it did.
  **The gate on [CHECKLIST-placement-tool.md](CHECKLIST-placement-tool.md) PT-5.3 is void as of
  2026-09-02** -- that decision was reversed and the tool is never published to a registry, so there
  is no gate to lift and no bullet to edit. The tool's GitHub binaries never waited on this.
  Blocked by SH-1.1, and by M31.6 as well if SH-1.2 decided that it gates.

## M5: verify from outside the workspace

- [ ] **SH-5.1** -- In a scratch project **outside this repository**, depend on both crates from
  crates.io and build something that uses each. This is the first exercise of the crates as
  *dependencies* rather than as path members, and it is where a missing `version` on a path dependency,
  an unexported type, or a feature that only resolves inside the workspace will show up.

- [ ] **SH-5.2** -- Confirm the published `windows-topology-sys` still reports `Provenance::Measured`
  from `discover()` when consumed as a dependency, and that a `MachineMemoryTopology::default()` is `Synthetic`.
  The provenance rules are the newest thing in the crate and the least exercised outside it.

## M6: long-running validation

**Numbered last among the release milestones, and it gates SH-4.3 all the same.** (It is no longer
*positioned* last: M14 and M15 were appended afterwards, as review rounds that had to go somewhere.)
`windows-waitable-queues`
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
  dedicated `BOUNDS_MAX` derivation and a `const` assertion for. See SH-14.1 and SH-14.2, which are
  about the *correctness* hole this testing gap was hiding.
  **SUPERSEDED IN PART, and the scope is now narrower than the paragraph above implies.**
  `slotwise_mpsc` **no longer reaches the wrap on any target**: SH-14.2 widened its positions to a
  named `Position = u64` everywhere, so it needs 2^64 claims. `spsc` never had a compare-exchange
  claim to race. So the only shape this item still has to drive across the wrap is
  `reserving_mpsc` -- and if SH-15.6 adopts the permit claim, whose ticket is likewise `u64`, that
  one goes too and this item's subject disappears entirely.
  **A wrap test alone cannot witness SH-14.1**, which is worth stating here because this item reads
  as though it could. Crossing 2^32 exercises the arithmetic; the defect additionally needs a
  producer *held* between its room check and its claim. That seam is SH-15.7's, and the two are
  complementary rather than alternatives.
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

> **M7 through M13 -- seven PR #56 review rounds -- are complete and archived.** Moved to
> [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) on 2026-09-02. M14 and M15 stay below because they
> carry open work.

**Everything from here down happened, and happens, *inside* M3 rather than after it.** These are
rounds of review on PR #56, which opened on 2026-08-31 and has not merged; they are reactive work
that arrives while a pull request is open, so their position at the end of this file is numbering
order and not a schedule. Reading it as a schedule would put the review of a pull request after the
merge that closes it.

**M14 and M15 do not gate SH-3.4; M16 does.** For M14 and M15 that is a decision rather than an
oversight: the defect they concern ships **disclosed rather than fixed**
([D-36](crates/windows-waitable-queues/DESIGN-NOTES.md#d-36)), and the disclosure -- which *was* the
release blocker -- landed at SH-15.8. SH-3.1.1 is responsible for saying so in the pull request
description, so a reviewer does not mistake those open milestones for unfinished business.

**M16's locality-model work is a merge blocker**, by decision rather than by drift -- an earlier
revision of this file listed it alongside the others as non-blocking, and that is corrected here.
The reason it differs: M14 and M15 concern a defect in an *implementation*, which can ship
disclosed, whereas M16 concerns the *shape of the public model* `windows-topology-sys` 0.2.0 would
publish. A disclosed implementation defect can be fixed in 0.2.1; a published model cannot be
reshaped without another break.

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

- [x] **SH-14.3** -- **SUPERSEDED BY SH-15.6, which is the same decision with better information.**
  Checked off as *asked and answered elsewhere*, not as decided: the decision itself is still open,
  and it is open in exactly one place now rather than two.
  This item enumerated four options and asked for a choice. Keeping it open beside SH-15.6 meant two
  live items for one decision, and worse, **this one's option list is now wrong in three ways**:
  option 1 said widening is impossible for `reserving_mpsc`, which is true only of its *own* word --
  [D-37](crates/windows-waitable-queues/DESIGN-NOTES.md#d-37) widens a separate shape instead;
  option 4's "drop 32-bit" turns out to be entailed by option 1 rather than an alternative to it
  ([D-18](crates/windows-waitable-queues/DESIGN-NOTES.md#d-18), amended); and the list has no entry
  for the claim protocol that was since built and measured
  ([D-35](crates/windows-waitable-queues/DESIGN-NOTES.md#d-35)), which is the current front-runner.
  A stale option list competing with a current one is how a decision gets re-litigated from the wrong
  premises. The live list is SH-15.6's.
  Its one surviving contribution is the observation that no test crossing the wrap can witness the
  bug -- a producer must be *held* between its decision and its exchange. That is not lost: it is
  SH-15.7, which owns the seam.

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

## M15: the claim protocol, prototyped rather than argued (absorbs SH-14.3)

**This milestone owns SH-14.1's fix, and SH-15.6 is where it is decided.** It absorbed SH-14.3, whose
four options were stale before they were chosen from. The candidates now on the table are three, not
four: the permit claim (built and measured, SH-15.3/SH-15.5), the wide claim word (planned, SH-15.9),
and doing nothing but the disclosure already shipped at SH-15.8. None could be chosen on reasoning
alone, because [D-26](crates/windows-waitable-queues/DESIGN-NOTES.md#d-26) had already measured that
the single shared line is what collapses under contention -- so an "obviously cheaper" claim protocol
that touches two shared lines instead of one might well have been slower. It was not
([D-35](crates/windows-waitable-queues/DESIGN-NOTES.md#d-35)), which is exactly why it was measured.

**Built as a duplicated path, per the repository's platform-integrity rule.** The prototype does not
modify `reserving_mpsc`: the shipping shape keeps working and keeps its tests green while the
speculative one is proven or discarded, and merge-or-delete is SH-15.6 rather than something that
happens by drift.

**Numbering note: there is no SH-15.4.** It was the second arm -- the per-cell cycle claim -- and it
moved to `SH-inf.1` when in-order delivery, inline storage and non-blocking progress turned out to be
over-constrained together. The number is left vacant rather than reused, so a reference to SH-15.4 in
an older commit still resolves to something.

**The principle the prototype is an instance of**, stated once so it is not re-derived: *the atomic
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
  because SH-15.6 cannot be decided without it.**
  **Spawned by SH-15.5, not a leftover of it.** SH-15.5 is checked because its own action -- taking
  the measurement -- is finished; this is new work that the measurement revealed, which is why the
  pairing of a checked parent and an open child is correct rather than a contradiction. In the drained regime `permit_mpsc` recorded
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

- [ ] **SH-15.6** -- **Decide: merge, delete, or ship as a third peer.** **RE-PLANNED: this item was
  written as a binary and the binary was wrong.** "Merge or delete" presumes one protocol dominates,
  and [D-35](crates/windows-waitable-queues/DESIGN-NOTES.md#d-35) measured that none does -- the
  permit claim wins from four producers upward and loses at one, with no configuration-free winner.
  [D-29](crates/windows-waitable-queues/DESIGN-NOTES.md#d-29) already settled how this crate answers
  that question for the two existing shapes: **both ship, the crate publishes what it measured, and
  the caller decides on its own hardware.** Deleting a shape because no visible consumer wants it is
  what the platform-integrity rule forbids; adopting one because it won most rows would be the same
  error facing the other way.
  So the live outcomes are three, and the third is now the most likely: adopt arm A into
  `reserving_mpsc`; delete it and take one of SH-14.3's original options; or promote it to a named
  peer alongside the other two, with the measurement published so a caller can choose. The
  duplicated path still may not become permanent *by inattention* -- that is what this item guards --
  but becoming permanent *by decision* is a legitimate outcome rather than a failure of the
  duplication rule.
  Whichever way it goes, SH-14.1's hazard must be either fixed or documented as an accepted
  limitation with its exposure stated -- it may not simply stay open. **That disclosure is no longer
  gated on this item**: see SH-15.8, which must land before 0.1.0 publishes regardless of what is
  decided here.
  **One comparison to make explicitly rather than leave implied, now that SH-15.9 adds a third
  answer to SH-14.1.** On the evidence so far the permit claim dominates the wide claim on every
  axis except maturity: it fixes the hazard on *all* targets where the wide claim covers only 64-bit
  ones, it is 2.7x faster at 16-32 producers where the wide claim keeps the retry loop that costs,
  it needs no dependency, and **it now reaches the same 2^62 ceiling on every target**: its ticket
  was widened to `u64` (as `slotwise_mpsc`'s was in SH-14.2), which was the wide claim's last
  remaining unique advantage.
  What the wide claim has instead is **risk**: it is a width change to a shape that has been through
  nine review rounds and whose behaviour is unchanged, where the permit claim is a new protocol with
  23 tests, no trait impls, no verification, and an open question about whether it reports
  backpressure more eagerly (SH-15.5.1). Those are genuinely different products -- conservative fix
  versus better fix -- which is the argument for shipping both rather than the argument for choosing.
  Do not let this comparison be settled by whichever is finished first.
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
  right shape. Without this, the prototype above is argued rather than demonstrated, and the fix that
  is adopted has no regression test that would go red if it were reverted.
  **COMPLEMENTS SH-6.1, and neither substitutes for the other.** That item drives the queue across
  2^32 and so exercises the arithmetic; this one supplies the stall that turns a wrap into the actual
  defect. A reader who does only SH-6.1 will get a green run and conclude wrongly.
  **This is the one M15 item that is worth doing whatever SH-15.6 decides** -- a fix with no test
  that fails without it is a fix nobody can safely revisit.

- [x] **SH-15.8** -- **Disclose SH-14.1 publicly, and gate 0.1.0 on the disclosure rather than on the
  fix.** **RELEASE BLOCKER.** The crate is days from its first publish with a known path to *silent
  data loss* -- a producer overwriting a live, unconsumed item -- documented nowhere a caller would
  see. That is not acceptable to ship in silence, and it is separable from deciding the fix: the
  limitation exists now, whatever SH-15.6 later concludes.
  **Precedent, and the reason this is a legitimate outcome rather than a dodge.**
  [D-31](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31) already ships one known gap this way --
  "the disclosure, not the deferral, is the decision" -- with its own README section and crate-doc
  section stating plainly what is verified and what is not. This follows that shape exactly and sits
  beside it.
  **But the two are not equally forgiving, and the disclosure must say so.** An unverified memory
  ordering is a risk of a bug; this is a *known* bug with a computed exposure. Its failure mode is
  silent: no error, no panic, no counter -- an item is overwritten and the consumer receives the
  wrong one, so **a caller cannot detect it and therefore cannot mitigate it after the fact.** A
  disclosure that only a careful reader finds is not a disclosure for a fault of that shape.
  What it must state, in the crate docs, the README, and `reserving_mpsc`'s own module docs:
  1. **It is not a 32-bit-only concern.** `POSITION_BITS` is 32 by construction on every target, so
     this reaches x86-64 and ARM64 exactly as it reaches i686. The sibling spelling "32-bit
     position" invites precisely the misreading that SH-6.1 already had to be corrected for once, so
     the words "on every target" belong in the first sentence.
  2. **The exposure, quantified**: 2^32 pushes, which is 37 s to about 4 minutes of sustained pushing
     at this crate's own measured rates -- roughly two minutes at two producers, the smallest count
     that can trigger it. Sustained, not cumulative-over-uptime.
  3. **What is required to trigger it**: the wrap *plus* a producer stalled between its room check
     and its claim, a window a few instructions wide. Rare, not unreachable, and a preemption is
     enough.
  4. **The alternatives a caller has**, which is what makes this a decision they can actually take:
     `slotwise_mpsc` does not have this hazard (SH-14.2 widened its positions to 64 bits on every
     target); `spsc` never had it; and the queue is safe at any push volume below the wrap.
  Sweep for consistency when writing it, per the contract-integrity rule: this fact will end up
  stated in at least four places and they must not drift.

- [ ] **SH-15.9** -- **Ship the claim word in two widths: the narrow one everywhere, the wide one where
  the hardware allows.** The engineer's decision, and it follows
  [D-29](crates/windows-waitable-queues/DESIGN-NOTES.md#d-29): publish what we measured and let the
  caller choose, rather than picking one tradeoff for everyone.
  - **`reserving_mpsc` is unchanged and always ships**, on every target, with SH-15.8's warnings. It
    is never silently swapped for the wide one on targets that could support it: a shape whose
    contract changes with the target is exactly what
    [PLATFORM INTEGRITY](../.github/copilot-instructions.md) rule 2 forbids, and a caller reading
    "2^32" in the docs must get 2^32.
  - **`reserving_mpsc_wide` is new**: the same claim protocol with a `u128` word split 64/64. The
    position then needs 2^64 pushes to recur -- about 16,000 years at this crate's measured rates --
    so SH-14.1 is unreachable rather than merely unlikely. The capacity ceiling rises from 2^31 to
    2^62, which was [D-18](crates/windows-waitable-queues/DESIGN-NOTES.md#d-18)'s original point.

  **The gate is one line of `Cargo.toml`, and this was measured rather than designed.** An earlier
  version of this item specified `#[cfg(target_has_atomic = "128")]` plus a `const` assertion on
  `is_always_lock_free()`. Both are unnecessary. Probing `portable-atomic` 1.15 on the pinned
  toolchain established:
  - With **`default-features = false`**, `portable_atomic::AtomicU128` **does not exist** on
    `i686-pc-windows-msvc` (`error[E0432]: unresolved import ... no AtomicU128 in the root`), nor on
    `x86_64` built with `-C target-feature=-cmpxchg16b`. It exists exactly where the target has a
    compile-time-guaranteed native lock-free 128-bit exchange. **The `use` statement is the gate**,
    and it fails loudly, naming the missing type.
  - With **default features**, the `fallback` feature compiles on i686 and silently substitutes a
    **global lock**. That -- not anything intrinsic to a 128-bit exchange -- is the whole source of
    the silent-degradation hazard, and it is opted out of rather than guarded against.
  - `#[cfg(target_has_atomic = "128")]` is the **wrong** gate regardless: `rustc 1.98.0 --print cfg`
    still emits it under `-C target-feature=-cmpxchg16b`, because it tracks the target's maximum
    atomic width and not instruction availability.
  - `is_always_lock_free()` **is** const-evaluable (confirmed: `const X: bool =
    AtomicU128::is_always_lock_free();` compiles, yielding `true` on x86_64). A const assertion on it
    is nonetheless **worse than useless** here -- redundant where the type exists, and unreachable
    where it does not, because there is nothing to compile.
  So: depend on `portable-atomic` with `default-features = false`, and add no cfg and no assertion.
  Note the consequence plainly in the shape's docs: **`reserving_mpsc_wide` cannot be built for
  i686 at all**, so a caller targeting 32-bit uses `reserving_mpsc` (with SH-15.8's warnings) or the
  permit claim. A portability cliff was chosen over a performance cliff because a compile error
  naming `AtomicU128` is more informative than a queue that silently stops being lock-free.

  **[D-7](crates/windows-waitable-queues/DESIGN-NOTES.md#d-7) puts the burden of proof on adding a
  feature, and it is met here rather than waived.** D-7 rejected feature-gating shapes because the
  only benefit was compile time, which dead-code elimination already provides. That reasoning does
  not reach this case: the wide shape's cost is a **new third-party dependency** on a crate whose
  only current one is `windows-sys`, and dead-code elimination removes nothing from `Cargo.lock`,
  from a downstream auditor's review, or from a `cargo vet` run. A caller who does not want the
  dependency must be able to not have it. Record the discharge as a decision rather than leaving it
  to look like D-7 was ignored.

- [ ] **SH-15.10** -- **Measure the wide claim beside the narrow one, and publish the difference.**
  Same harness, same host, same run as SH-15.5. The question is narrow and worth an answer either
  way: does a 128-bit exchange cost anything measurable against a 64-bit one on this hardware? If it
  does not, the wide shape is strictly better wherever it builds, and the guidance should say so. If
  it does, that number is what a caller needs to choose between reach and speed.
  Note the expected shape of the result, so a surprise is recognisable: the wide claim keeps the
  compare-exchange **retry loop**, which [D-35](crates/windows-waitable-queues/DESIGN-NOTES.md#d-35)
  identified as what actually costs at high producer counts -- so it should track `reserving_mpsc`
  closely and should **not** approach the permit claim's numbers. A wide claim that measured as fast
  as the permit claim would mean D-35's explanation is wrong.

## M16: PR #56 tenth review round -- the SH-3.1.1 diff review

> **Six of these items are superseded.** SH-16.5, SH-16.8, SH-16.9, SH-16.11, SH-16.12 and SH-16.13
> are all the same piece of work seen from different angles -- reshaping the machine memory topology
> -- and they now live in
> [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) as a plan of
> their own, numbered `MMT-*`. They are left here, unchecked and marked, rather than deleted: each
> records how the defect was *found*, which the new plan does not repeat.
>
> **Read the new plan for what to do; read these for why.** The six that remain live here are the
> review round's own findings, already fixed.

**This round is the one [SH-3.1.1](#m3-land-the-branch) asked for**, and it is the first that read the
branch as a *diff* rather than reacting to a reviewer's comment. Five reviewers took non-overlapping
crate scopes across all 200 changed files; seven findings came back, listed here worst-first rather
than by crate.

**Two of them are the reason the round was worth running.** SH-16.1 is a regression this branch
introduced *two commits ago* -- it would have failed the next `windows-ioring-sys` publish, twenty
minutes in, with an error naming the wrong cause. SH-16.2 is a soundness hole in the crate that is
about to freeze its API, in a shape whose selling point is that its ordering arguments are written
down and checked.

**Neither was reachable from a memory of having written the code**, which is exactly what SH-3.1.1
predicted about a 222-commit branch.

- [x] **SH-16.1** -- **`publish-crate.yml`'s sibling-dependency wait cannot handle a `*` requirement,
  so `windows-ioring-sys` can no longer publish.** The wait step derives a concrete version with
  `sed -E 's/^\^//'`, which handles only a caret. Commit `f1fc4eb` on this branch made ioring's
  topology dev-dependency path-only, so `cargo metadata` now reports `req=*` -- **verified, not
  assumed**. `windows-topology-sys` is in `workspace_crates`, so the loop is entered, `dep_version`
  becomes the literal `*`, and `select(.vers == "*")` can never match: 60 attempts x 20 s, then an
  error telling the operator to re-run once the dependency is available, which will never help.
  A versionless path dev-dependency is **stripped from the published manifest entirely**, so there is
  nothing to wait for and the right answer is to skip it.
  Note that `tools/check-publishable.ps1`, added in this same branch to catch "release-managed but
  unpublishable", does **not** catch this -- ioring passes all three of its checks.
  **Done:** `*` is skipped with the reason stated, and any requirement that does not reduce to a
  comparable version (`~1.2`, `>=1, <2`, `=1.2.3`) now fails **immediately** naming the requirement,
  rather than reaching the same twenty-minute timeout by a different route. Verified by extracting the
  `run:` block and exercising it under `bash` against real `cargo metadata` output: ioring's seven
  dependencies now resolve to two waits, four skips and one versionless skip, and the step exits 0.
  A side benefit worth recording, found by getting the harness wrong first: the new check also
  catches a returning CR corruption -- the failure the `tr -d '\r'` above was added for -- because
  `0.1.3\r` is no longer a comparable version. That failure used to be a silent timeout too.

- [x] **SH-16.2** -- **`reserving_mpsc::Reservation::send` wrote a slot with no happens-before edge to
  the consumer's read of the previous occupant.** A slot is freed only by `Consumer::pop`'s
  `head.store(Release)`, and the matching acquire lives in `has_room_beyond_reservations`.
  `Producer::push` gets its edge from that room check; `send` deliberately has none -- the code says
  so -- and its claim CAS is `Relaxed` on every path, so there was no release sequence to inherit
  either. The claim proves the slot is *logically* free, which is not the same as a synchronization
  edge, and the `SAFETY` comment cited "the room check that permitted the claim" on the one path
  where no room check exists.
  **The default configuration was the unsound one**: `Options::tracking_high_water()` accidentally
  repaired it, because the metric's `head.load(Acquire)` sat just before the write. Fixed by making
  that load unconditional, which is where it belonged.

- [x] **SH-16.3** -- **`CancelIo` does not wait, so a test frees an `OVERLAPPED` and an I/O buffer the
  kernel may still write to.** In
  [reopen_by_id_cannot_be_watched.rs](crates/windows-file-watcher/tests/reopen_by_id_cannot_be_watched.rs),
  an overlapped `ReadDirectoryChangesW` is issued into a **stack-local** `OVERLAPPED` and a heap
  buffer, then `CancelIo` is called and both are dropped immediately. `CancelIo` only *requests*
  cancellation; the IRP still completes asynchronously and writes `Internal`/`InternalHigh` into a
  frame that has been reclaimed. Two safety comments assert the opposite of what the code guarantees.
  Aggravated by the helper being called twice back-to-back, so the second call's `overlapped` likely
  lands on the same stack address the first IRP will write into.
  **This crate has already been bitten by this exact class of corruption** -- the
  `STATUS_STACK_BUFFER_OVERRUN` history recorded on the now-removed `reopen_via_existing_handle`.
  Fix by calling `GetOverlappedResult(..., bWait = TRUE)` and accepting `ERROR_OPERATION_ABORTED`
  before either buffer leaves scope.
  **Done, and the wait is measured to be load-bearing rather than assumed.** A probe on the control
  path returned `completed=0, err=995` -- `ERROR_OPERATION_ABORTED` -- proving an IRP really was
  outstanding at the moment `CancelIo` returned and completed only during the wait. Without it, that
  completion landed on a reclaimed frame. The same wait is what makes `Owned`'s later `CloseHandle`
  safe, since closing a handle with I/O outstanding is another cancellation request and not a wait.

- [x] **SH-16.4** -- **`cache_partitions_at_level` counted a domain covering no processors as a
  partition.** An empty `ProcessorSet` is not *equal* to any non-empty one, so deduplication kept it,
  and `is_disjoint` is vacuously true on it, so the pairwise check passed it. A level with one real
  cache plus one empty domain therefore reported two partitions and was treated as dividing a machine
  it does not divide. `Domain` is publicly constructible and `ProcessorSet` has `empty()`, so this is
  reachable by hand and by deserialization -- precisely the input the method promises not to trust.
  Fixed by dropping empty domains, with the contrast against `memory_domains` (which deliberately
  keeps a processor-less domain, D-5) recorded at the filter.

- [ ] **SH-16.5** -- **SUPERSEDED by [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) (MMT-*); kept for how it was found.** **`windows-placement-probe` refuses a partially-covering cache level that
  `windows-topology-sys` deliberately hands back.** `outermost_partitioning_cache` documents that
  "full coverage of the online processors is deliberately *not* required"; `places_from_topology`
  treats any online processor the chosen level does not name as `MissingPlacement::CacheDomain` and
  fails the **entire run** with `InvalidData`. Two crates state opposite rules about the same return
  value -- a [CONTRACT INTEGRITY](.github/copilot-instructions.md) defect, not merely a bug.
  Decide the rule **once**, in the crate that owns the topology, and have the consumer ask rather than
  restate. Note the asymmetry that makes the NUMA arm different and correct: for NUMA, `None` has no
  honest value, whereas `cache_domain` is already `Option<u32>`.
  **BLOCKED on
  [DESIGN-SESSION-2026-09-02-cache-locality-model.md](design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md)
  -- and *not* for want of a consumer.** The fix was implemented; implementing it surfaced a design
  question the fix would have silently answered. The primitive it adds is a single "which cache domain
  is this processor in", which **is** the single-boundary collapse that session is about, so landing it
  would prejudge the outcome. The prototype compiled, and its topology-side tests passed and were
  sabotage-verified; it was reverted deliberately and preserved outside the repository as
  `sh-16.5-prototype.patch`.
  **Unblocked 2026-09-03, and superseded rather than resumed.** The session's questions were answered
  as `D-13` through `D-21`, and the answer is *not* the primitive this item prototyped: under
  [D-19](crates/windows-topology-sys/DESIGN-NOTES.md#d-19) the unified relation set with its
  inclusion order replaces a single per-processor cache-domain lookup, so the prototype would have
  landed the collapse the session existed to remove. The contradiction is fixed by `MMT` **M2+.5** and
  **M5+.4** instead. The patch is kept as the record of what was tried and why it was not taken.

- [ ] **SH-16.8** -- **SUPERSEDED by [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) (MMT-*); kept for how it was found.** **The locality model collapses a seven-kind, any-depth topology onto one cache
  boundary, and nothing records that as a choice.** Raised by the engineer during the SH-16.5 fix, and
  confirmed: `windows-topology-sys` hardcodes no level count (`level` is a `u8`, and a regression test
  already guards against a consumer sweeping `1..=4`) and models `Group`, `Package`, `Die`, `Module`,
  `Core`, `Cache` and `Memory` -- but `outermost_partitioning_cache` selects one level and discards the
  rest, `ProcessorPlace::cache_domain` is one scalar, and `Placement` carries three tiers.
  Three consequences, all verified: "same cache" denotes **a different boundary on different machines**,
  so a label is not portable across records; `CrossCache` conflates "different L2, same L3" with
  "different L3" on any machine with two live boundaries; and it has already cost a row in this
  project's own matrix -- the x64 host's "cannot express `same cache, same class`" note in
  [DESIGN-NOTES.md](crates/windows-waitable-queues/DESIGN-NOTES.md) is attributed to hardware, but
  those sixteen processors do share one L3, so a per-level model would express it.
  Gated on the session above, which carries the design space and the open questions.
  **Scope addition from [D-13](crates/windows-topology-sys/DESIGN-NOTES.md):** the audit that decision
  performed over every `Option` in the crate found exactly one site that documentation cannot fix.
  `DomainKind::Memory::memory_bytes` is unambiguous from `discover`, which always sets `None`, but a
  **description's** `None` conflates "the description omitted the field" with "this node's capacity is
  genuinely unknown" -- the two are the same value today. Whatever representation this item lands must
  cover it, since absence becoming first-class is precisely the fix.
  **Direction now settled** by the engineer: presence and observation must be modeled, not
  collapsed into an `Option`. "Win32 did not report it" and "it was found not to be present" are
  different facts, and the representation must be built for **observed connectivity** rather than
  for a ladder of levels with optional rungs. That rules out the SH-16.5 prototype's `Unknown` arm,
  which merges both. Shape still open.

- [ ] **SH-16.9** -- **SUPERSEDED by [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) (MMT-*); kept for how it was found.** **The "outermost partitioning cache" rule is stated three times, and two of the
  three disagree.** `MachineMemoryTopology::outermost_partitioning_cache` requires more than one partition **and**
  pairwise disjointness. `Observation::outermost_partitioning_cache` in `windows-platform-probes` is
  `caches.iter().filter(|c| c.domains > 1).max_by_key(|c| c.level)` -- **no disjointness check** --
  computed over a `CacheLevel` summary that crate builds itself, even though it already depends on
  `windows-topology-sys`. On a hand-built or deserialized topology with overlapping domains the two
  crates give different answers to the same question. `windows-placement-probe` restates it a third
  time by rebuilding the map from the partition list, which is SH-16.5.
  A [CONTRACT INTEGRITY](.github/copilot-instructions.md) defect of the exact shape the rules name:
  a rule re-encoded by a consumer rather than derived from the owner. Note the ordering -- fixing
  this by pointing both consumers at today's method would have to be redone once SH-16.8 lands, so
  either fix it now and accept the rework, or sequence it after the design session.

- [x] **SH-16.10** -- **`GetSystemCpuSetInformation` is not consumed anywhere, so a whole Win32
  topology model is unexposed.** The crate consumes all seven `GetLogicalProcessorInformationEx`
  relations, but `SYSTEM_CPU_SET_INFORMATION` is a *second, parallel* model carrying at least
  `LastLevelCacheIndex` -- Windows's own LLC grouping, which is a **different answer** from
  "outermost partitioning cache" and would be directly comparable against it -- plus
  `SchedulingClass`, `AllocationTag`, `EfficiencyClass`, and per-processor `Parked` / `Allocated` /
  `RealTime` state.
  Raised by the engineer's question of whether we expose everything a real system would reveal
  through the Win32 API set. Today the answer is **no**.
  Note `Parked` and `Allocated` bear directly on **thread counts and assignments**, one of the three
  decisions the model exists to serve, so this is a gap already costing a named use rather than
  speculative completeness.
  **Ungated, and split, because the gating premise was wrong.** This said "gated on SH-16.8, since
  what shape it lands in depends on the model". That conflated two things: *acquiring* the data and
  *reconciling* it with what `GetLogicalProcessorInformationEx` already reports. Acquisition does
  not depend on the model at all -- CPU Sets is a **cheap OS read**, in the same class as the walk
  this crate already does, and nothing about reading it presumes a granularity representation. Only
  reconciliation depends on the model, and that is now SH-16.13.
  Field list **verified against `windows-sys 0.61.2`** rather than recalled: `Id`, `Group`,
  `LogicalProcessorIndex`, `CoreIndex`, `LastLevelCacheIndex`, `NumaNodeIndex`, `EfficiencyClass`,
  a union carrying `AllFlags` (`Parked` / `Allocated` / `AllocatedToTargetProcess` / `RealTime`), a
  union carrying `SchedulingClass`, and `AllocationTag`. All five APIs are present
  (`GetSystemCpuSetInformation`, `GetThreadSelectedCpuSets`, `SetThreadSelectedCpuSets`,
  `SetThreadSelectedCpuSetMasks`, `SetProcessDefaultCpuSets`) and `Win32_System_SystemInformation`
  is already an enabled feature, so there is no manifest change and no blocker.
  **Done.** `src/cpu_set.rs` walks the records with the same buffer discipline the relationship walk
  uses -- size first, advance by each record's own `Size`, read every field unaligned -- and
  `MachineMemoryTopology::discover` now populates `MachineMemoryTopology::cpu_sets`. Carried as
  `Option<Vec<CpuSet>>` where `None` means **not observed**, which a hand-built or deserialized
  topology genuinely is; that is the honest use of `Option`, one absence rather than two collapsed
  together. `#[serde(default)]` so descriptions written before the field still load.
  **Nothing is reconciled**, per duplicate-then-decide. SH-16.13 owns that.
  **The live dump justified the caution.** On the x64 host, CPU Sets reports **one** distinct
  `LastLevelCacheIndex` across all sixteen processors, while `outermost_partitioning_cache` reports
  **eight** partitions at L2. Both are right -- Windows names the *last* level, the derivation names
  the outermost level that *divides* -- so a merge treating `LastLevelCacheIndex` as "the cache
  domain" would have collapsed eight shard groups into one on this machine. Kept as a test asserting
  the *relationship* (Windows's grouping is never finer) rather than the host's numbers.
  It also confirms the matrix-hole argument from
  [DESIGN-SESSION-2026-09-02-cache-locality-model.md](design-sessions/DESIGN-SESSION-2026-09-02-cache-locality-model.md):
  this is the host recorded as unable to express `same cache, same class`, and a second source now
  says all sixteen share an LLC, so that row is real rather than inferred.
  **One thing is verified only against the SDK's documented bitfield order, not against Windows:**
  the four flag bit positions. Every processor on this host reads `parked=false, allocated=false,
  allocated_to_target_process=false, real_time=false`, which is consistent with a process that has
  requested no CPU-set allocation but confirms no bit position. `each_flag_is_read_from_its_own_bit`
  checks the decode is self-consistent, not that it matches the OS. Confirm against a parked
  processor or an explicit `SetProcessDefaultCpuSets` before relying on the flags.

- [ ] **SH-16.13** -- **SUPERSEDED by [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) (MMT-*); kept for how it was found.** **Reconcile the CPU-set observation with the relationship walk.** `CoreIndex`,
  `NumaNodeIndex` and `EfficiencyClass` **duplicate** facts `GetLogicalProcessorInformationEx`
  already reports, from a different kernel path -- so this is not redundancy to remove, it is a
  **second independent observer of the same relations**, and the two can disagree under a hypervisor
  or where one path is stale.
  This is the concrete instance of the design session's "can one relation hold several
  observations?" question, which until now rested on the file-handle spike's agree/disagree
  reasoning about a different subject. It is no longer speculative: two Win32 sources describe the
  same processor's NUMA node and efficiency class today.
  Per [PLATFORM INTEGRITY](.github/copilot-instructions.md)'s duplicate-then-decide rule, SH-16.10
  lands the CPU-set data as its **own** observation alongside the existing domains, without merging.
  This item is the merge-or-delete decision, made when the model settles rather than pre-empted.
  Gated on SH-16.8.
  Note it also bears on SH-16.12: CPU Sets carries `EfficiencyClass` as a plain `u8` with **no
  sentinel**, so it is a cleaner source for the field whose `capacity` encoding collides with
  "unknown".

- [ ] **SH-16.11** -- **SUPERSEDED by [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) (MMT-*); kept for how it was found.**
  **And now ANSWERED, in the opposite direction to what this item proposed.**
  [D-20](crates/windows-topology-sys/DESIGN-NOTES.md#d-20) rules that the crate does not go below the
  Win32 topology APIs, so a fact Win32 does not report is not one the crate has: `distances` is
  **deleted, not filled**. The removal is `M5+.5` in
  [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md). Everything
  below is the reasoning that led there and is kept for that; it no longer describes work. **`MachineMemoryTopology::distances` is a field for a fact Win32 cannot supply, it is never
  populated, and the measurement that would fill it already exists elsewhere.** `discover()`
  hardcodes `distances: None`, every other construction sets `None`, and no consumer reads the
  field. Windows exposes no API for NUMA node distance -- ACPI carries SLIT, Win32 does not surface
  it -- so measurement is the only source. `windows-placement-probe` **already measures the
  equivalent** through `node_pairs_measured()`, producing per-node-pair handoff cost with ring
  placement, and renders it as a table that goes nowhere else.
  This is the canonical case for the whole model: under the bar that the model must be usable
  **without further measurement**, a consumer shaping memory allocation must today either run the
  probe at decision time -- forbidden -- or guess. Gated on SH-16.8, and on the open question of
  which component owns the measurement phase.
  **Corrected while stating [EP-D-3](crates/topology-planner/DESIGN-NOTES.md#ep-d-3): the
  wording above reads as an oversight, and it is not one.** The field is documented as being for a
  fed-in description, because Windows exposes no user-mode SLIT reader -- accurate, and deliberate.
  Two sharper problems replace the one this item claimed.
  **First, `distances` can never carry `Measured` provenance, by construction.** Its only inputs are
  hand construction (defaulting to `Synthetic`) and deserialization (capped at `Restored` by
  `downgraded_to`), and `discover()` hardcodes `None`. So populating it would not help: a planner on
  a real machine still could not obtain trustworthy distance *for that machine*.
  **Second, even populated it answers the wrong question.** The matrix is SLIT-shaped -- one
  symmetric, workload-independent scalar per pair -- while the residency decision is directional,
  since the producer writes and the consumer reads. `D-9` in
  [crates/windows-topology-sys/DESIGN-NOTES.md](crates/windows-topology-sys/DESIGN-NOTES.md) already
  anticipated exactly this and deferred it, naming an attributed edge list that "would absorb HMAT,
  **asymmetry**, and multi-hop CXL fabrics", with the trigger being that "scalar distance
  demonstrably mismodels a machine somebody is tuning for". `D-8` keeps the JSON schema outside
  semver specifically to make that revision cheap.
  **The trigger is approached but not met, and the difference is a measurement nobody here can
  take.** The probe treats direction as real -- four numbers per undirected edge, and its code says
  "a hop is not symmetric even though the link is" -- but no run has *shown* those numbers differ,
  because both development hosts report a single NUMA node and every such run prints "VACUOUS ON
  THIS MACHINE". Take that measurement on multi-node hardware before reopening D-9 on asymmetry
  grounds, not after.

- [ ] **SH-16.12** -- **SUPERSEDED by [crates/windows-topology-sys/CHECKLIST.md](crates/windows-topology-sys/CHECKLIST.md) (MMT-*); kept for how it was found.** **`Processor::capacity` uses `0` as both a legitimate efficiency class and a
  sentinel for "not known", and the two collide on the common case.** It is computed
  `online.then(|| find the owning Core domain).flatten().unwrap_or(0)`, so `0` means the processor is
  offline, *or* is online but named by no `Core` domain, *or* genuinely has efficiency class zero.
  The third is **every processor on every non-hybrid machine**, so the sentinel is not a rare
  collision -- it is the usual value.
  Found by [crates/topology-planner](crates/topology-planner/DESIGN-NOTES.md#ep-d-1)
  EP-1.1 while checking what a shard planner can rely on, and it is worse for that consumer than for
  most: Windows orders efficiency class with `0` as **least** performant, so on a hybrid part an
  unknown processor is indistinguishable from an efficiency core. A policy excluding efficiency cores
  would silently drop a processor that may be a performance core; a policy tiering them would put it
  in the wrong tier. Neither shows up in a functional test.
  **A third instance of the pattern SH-16.8 exists to fix**, and the one not previously swept -- the
  others being `ProcessorPlace::cache_domain`'s `Option<u32>` (SH-16.5) and
  `MachineDescription::cpu_model`, where the same conflation was noticed and solved with a side
  boolean. Note this one is *worse* than an `Option`: a sentinel that collides with a valid value
  cannot be distinguished even by a careful caller. Gated on SH-16.8, since the fix is the same
  question -- how absence is represented -- and doing it twice would be doing it twice.
  Note `DomainKind::Core { efficiency_class }` already carries the value without a sentinel, so the
  interim guidance is to read that instead; the defect is that `capacity` exists and looks usable.

- [x] **SH-16.6** -- **The thread-stack NUMA spike's `deep_probe` measures the shallow end of its own
  filler, so the discrimination it exists to make is inert.** The stack grows down, so `filler[0]` is
  the deepest address and `filler[last]` sits immediately below the caller's frame -- but the probe
  takes `&raw const filler[last]`, landing very likely on the same page as the shallow probe rather
  than 64 KiB away. The spike would then report "not first touch" on a machine where placement *is*
  by first touch: a confident wrong answer, in a file whose whole point is avoiding those. Both ends
  are already touched, so probing `filler[0]` is a one-token change.
  **Done, and the defect was worse than reported.** Printing the three addresses on all three spike
  threads showed the old probe was not merely *likely* on the shallow probe's page -- it was on the
  **same page every time**, 209 bytes away, where the review had estimated "at worst adjacent". So
  `shallow.node != deep.node` compared one page against itself and could not fire even in principle.
  After the change the two probes are 16 pages apart on every thread. Measured on all three threads
  (`0x...dff7c0` vs `0x...dff6ef` -> same page; vs `0x...def6f0` -> 16 pages), then the instrumentation
  was removed.

- [x] **SH-16.7** -- **A `windows-thread-ambient-sys` test claims a restore-failure it never
  injects.** `release_reports_a_genuine_restore_failure_and_restores_on_drop_even_without_it` asserts
  the *opposite*: it `expect`s the release to succeed and both closing assertions check that restore
  worked. Its siblings in `declared/tests.rs` and `error_mode/tests.rs` do force genuine failures; this
  one inherited the name without the failure-injection half.
  **Done, by renaming -- and the review's stated hazard did not hold.** It reported that
  `TransactionGuard::release`'s error path "reads as covered when it is not". Checked rather than
  taken: the path **is** covered, by `explicit_release_reports_an_injected_restore_failure` in the
  same file, via a `FaultPoint::TransactionSet` injection built for it. Verified by running both.
  So the defect was only ever the name. Renamed to
  `release_and_drop_each_restore_a_real_entry_transaction`, and the comment now records *why* the
  sibling naming does not apply -- a transaction restore either sets a real handle or clears to
  "none", and both succeed, so unlike a null WOW64 cookie or `SEM_NOALIGNMENTFAULTEXCEPT` there is no
  naturally-rejecting value to provoke. Written down so the missing half is not re-attempted.

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
