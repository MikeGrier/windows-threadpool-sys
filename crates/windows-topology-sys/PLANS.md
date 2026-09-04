# Plans: windows-topology-sys

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | **M6: the record walks' bounds discipline.** Opened by the PR #56 diff review, which found an out-of-bounds read in `cpu_set.rs` -- the `Type` field is at offset 4, but the loop guard proved only four bytes, so a trailing record declaring a `Size` of 1..=7 put the read past an exactly-sized allocation. That half is fixed and shipped in PR #56. `walk::decode` is the sibling it exposed: its guard is `while offset < length` (one byte), it never checks `offset + size <= length` at all, and `decode_body` trusts each relationship's trailing-array counts unbounded. Left out of PR #56 because `walk.rs` was unchanged there. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |

Completed plans are in [COMPLETED-PLANS.md](COMPLETED-PLANS.md).
