# Plans: windows-overlapped-io-sys

Completed checklists are recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the milestones they
contained are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M1: give `AssociatedEndpoint` a blocking `Drop` (mirroring `CompletionPort::run_down`'s own drop behavior) so a caller cannot close its handle while an operation is still outstanding against it -- found while designing `windows-ioring-sys`'s M8 (PR #20 review response). | N/A |
