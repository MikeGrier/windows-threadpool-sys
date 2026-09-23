# Plans: win-numa-sys

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M1 finishes what creating the crate started: `windows-placement-probe` still has its own `VirtualAllocExNuma`, so the duplication is currently **relocated rather than removed**, and `N-1.1` is where that is closed. `N-1.2` decides whether `QueryWorkingSetEx` observation moves here with it -- the capability that would let a caller ask whether a placement request was honoured, rather than being told by the documentation that success proves nothing. `M2+.1` is publishing, which is not optional indefinitely: the crate is a path dependency of the published `windows-ioring-sys`. | N/A -- decisions so far are the repository's, at [DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix) |
