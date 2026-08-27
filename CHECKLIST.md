# Checklist: workspace

Workspace-level and cross-cutting work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M1 -- Amplify PR #42's contract-specification findings across the delivery-contract crates

PR #42 ("Testability: consumer test surface for windows-file-watcher + example test harness crate") took
**19 automated review rounds**, and the review-response phase (39 commits) added more code than the original
implementation (16 commits) did. The dominant failure was not implementation error: it was that
`windows-file-watcher`'s delivery contract, written as prose, was **true but incomplete** in categorizable
ways -- and nobody could see the gaps until a second implementation (the harness's contract-legal generator,
D-5) had to obey the contract mechanically. Eight separate rounds fixed generated sequences the watcher could
never emit; five more corrected the contract prose itself; and one found a real shipped reliability defect in
`windows-file-watcher` (`has_room`, 700e0eb) on D-29's backpressure path.

The transferable asset is the **taxonomy of gap categories**, not the individual fixes. The three crates in
this repo that publish a delivery/completion contract -- `windows-file-watcher`, `windows-overlapped-io-sys`,
and `windows-ioring-sys` -- are all exposed to the same categories, and the latter two have already paid for
instances of them (overlapped-io's M10.5 rundown wedge; ioring's D-17 cross-ring identity).

- [ ] **M1.1** -- Record the taxonomy in the workspace [DESIGN-NOTES.md](DESIGN-NOTES.md) as a shared
  invariant: the ten gap categories, each with the PR #42 instance and commit that evidences it, plus the
  rule that a contract claiming a consumer can rely on it must state every category explicitly rather than
  by omission. This is the canonical home; the per-crate notes reference it rather than restating it.

- [ ] **M1.2** -- `windows-file-watcher`: record the retrospective as a decision (D-84) -- the contract was
  underspecified, the second implementation is what exposed it, and `has_room` is the evidence that the cost
  is real rather than cosmetic. Cross-reference the taxonomy and the specific decisions each finding
  amended (D-9, D-12, D-17, D-27, D-28, D-30, D-50, D-78, D-83).

- [ ] **M1.3** -- `windows-overlapped-io-sys`: apply the taxonomy to its own published contract. It has
  already paid for two categories (the `Issued` "will a packet arrive" conflation, which its own notes call
  "the single most misread part of the seam"; and `post`/`post_raw`'s arbitrary completion key, which its
  notes already flag as corrupting per-endpoint counters). State them as instances of a named category
  rather than as isolated war stories, and enumerate the categories its contract has *not* yet stated.

- [ ] **M1.4** -- `windows-ioring-sys`: apply the taxonomy. Its D-14 (registration bookkeeping advances at
  queue time) is an explicitly unverified cross-message continuity assumption; its D-17 (`RingId`) is a
  cross-object identity rule it already got right and should be cited as the positive example; and
  `Completion::synthetic`'s `#[cfg(test)]` gate is the production-domain-fidelity rule it already got right.
  Record what a consumer may and may not infer about completion ordering, which the notes do not yet state.
