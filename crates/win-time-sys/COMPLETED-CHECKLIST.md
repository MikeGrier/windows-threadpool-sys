# Completed checklist: win-time-sys

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-07 23:11:28 -04:00 -- WT-1.1: the crate

### <a id="wt-11"></a>WT-1.1 -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. *(completed 2026-10-07 23:11:28 -04:00)*

Registered as `win-sync-sys` was: a workspace member, in release-please's configuration and manifest
(at `0.0.0`, so the first release is `0.1.0`), and in the publish workflow's tags, manual-dispatch
choices and workspace-sibling list. The README states the two layers
([WT-D-2](DESIGN-NOTES.md#wt-d-2)) and what is deliberately absent. It has no dependencies yet: the
Win32 bindings arrive with the clocks that call them, so no feature list is guessed in advance.
"Each `unsafe` with its safety argument" is enforced rather than asked for:
`clippy::undocumented_unsafe_blocks` is denied, the first crate in the workspace to do so. Committed
as `chore`, because a crate with no API has nothing to release; `WT-1.2` is the first item that does.

The item as it stood at completion:

- [x] **WT-1.1** -- **Scaffold the crate.** `crates/win-time-sys`, a workspace member registered for
  release and publication, Windows-only, with a README saying what it is. Its `unsafe` is confined to
  the Win32 calls, each with its safety argument; everything it exports is safe.
