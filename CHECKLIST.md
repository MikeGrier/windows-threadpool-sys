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

- [x] **M1.1** -- Record the taxonomy in the workspace [DESIGN-NOTES.md](DESIGN-NOTES.md) as a shared
  invariant: the ten gap categories, each with the PR #42 instance and commit that evidences it, plus the
  rule that a contract claiming a consumer can rely on it must state every category explicitly rather than
  by omission. This is the canonical home; the per-crate notes reference it rather than restating it.

- [x] **M1.2** -- `windows-file-watcher`: record the retrospective as a decision (D-84) -- the contract was
  underspecified, the second implementation is what exposed it, and `has_room` is the evidence that the cost
  is real rather than cosmetic. Cross-reference the taxonomy and the specific decisions each finding
  amended (D-9, D-12, D-17, D-27, D-28, D-30, D-50, D-78, D-83). Queued the audit this does *not* claim to
  have done as that crate's M14.

- [x] **M1.3** -- `windows-overlapped-io-sys`: apply the taxonomy to its own published contract. Found two
  categories it had already paid for (`Issued`'s state-dependent legality, which hung rundown until M10.5;
  `post`/`post_raw`'s arbitrary completion key), two it got right (`OperationId` generations, removing
  `from_parts`), and one consequential omission -- **completion observation order was never stated**, which
  matters because `windows-file-watcher` builds on this crate and *does* promise ordering to its own clients.
  Remaining categories queued as that crate's M14.

- [x] **M1.4** -- `windows-ioring-sys`: apply the taxonomy. Cited D-17 (`RingId`) and
  `Completion::synthetic`'s test-only gate as the pattern done right, recorded D-14 as an honestly-flagged
  cross-message continuity assumption, and stated the previously-missing completion-ordering rule -- a gap
  this crate is *more* exposed to than its siblings, since "ring" invites the ordered-queue assumption.
  Remaining categories queued as that crate's M10.

**The audits are deliberately partial, and say so.** Between them they reach five of ten categories in
overlapped-io and four of ten in ioring; the rest are recorded as "not examined" rather than "does not
apply", because that distinction is the entire point of having the taxonomy. Completing them is each crate's
own milestone, not this one's.
