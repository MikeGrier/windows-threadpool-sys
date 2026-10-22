# Checklist: wtf-string

`OsString`-shaped strings with native `u16` (WTF-16) storage. Design and decisions
(D-1...D-18) are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1),
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2), and
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md)
(Tier 3).

Work items are dependency-ordered. Each milestone ends with tests. The implicit
end-of-milestone gate (default build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). Nothing is pending; the horizon item is in
[PARKED.md](PARKED.md).
