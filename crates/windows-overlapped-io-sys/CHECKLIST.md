# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M1 -- `AssociatedEndpoint` drop-while-outstanding

Found while designing `windows-ioring-sys`'s M8 (a PR #20 review response): `AssociatedEndpoint` owns its
`handle: OwnedHandle` directly and has no `Drop` impl, so a caller can drop one (closing its handle via
`OwnedHandle`'s own drop) while an operation is still outstanding against it specifically.
`CompletionPort::run_down` is the wrong scope to catch this -- it blocks on the *port's* whole outstanding
count, not any one endpoint's.

`AssociatedEndpoint` has exactly one owner (`!Clone`), so the fix is the same shape
`CompletionPort` already applies to itself, not `windows-ioring-sys`'s `Arc`-based `SharedFile` (that
mechanism is for genuine multi-owner sharing, which this endpoint does not have and should not pay for).

- [ ] **M1.1** -- Track each `AssociatedEndpoint`'s own outstanding count (or reuse the port's per-endpoint
  accounting if it already exists) and give `AssociatedEndpoint` a `Drop` that blocks until it reaches
  zero, mirroring `CompletionPort::run_down`'s own drop behavior.

- [ ] **M1.2** -- Integration test: drop an `AssociatedEndpoint` with a real operation still outstanding
  against it and assert the drop blocks until that operation's completion is observed, rather than closing
  the handle out from under a live kernel operation.
