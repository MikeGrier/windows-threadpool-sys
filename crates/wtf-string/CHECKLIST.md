# Checklist: wtf-string

`OsString`-shaped strings with native `u16` (WTF-16) storage. Design and decisions
(D-1...D-18) are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) (Tier 1),
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2), and
[design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md](design-sessions/DESIGN-SESSION-2026-08-19-wtf-string.md)
(Tier 3).

Work items are dependency-ordered. Each milestone ends with tests. The implicit
end-of-milestone gate (default build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M-inf -- Horizon (ungated, post-v1)

Parked, not pending. The remaining item is placed outside the v1 scope by an explicit, recorded design
decision (D-7 makes the C-string companion optional). That recorded decision -- not the absence of a current
consumer -- is why it is deferred, which is a legitimate deferral rationale (a resolved, recorded scope
decision), not a scope-boundary excuse. It graduates to a numbered milestone when a post-v1 line of work
takes up that decision. See [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **M-inf.1** -- A checked no-interior-NUL C-string companion type (an enforced-guarantee analog of the
  terminated pointer) (D-7).
