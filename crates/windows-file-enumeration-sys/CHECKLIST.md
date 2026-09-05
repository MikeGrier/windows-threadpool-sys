# Checklist: windows-file-enumeration-sys

The crate's implementation milestones are M5 through M7 in the workspace
[CHECKLIST.md](../../CHECKLIST.md). This file holds work owned by the crate
itself. Status is tracked in [PLANS.md](PLANS.md).

## M-inf -- Horizon (ungated)

- [ ] **REVIEW-1** -- Review the request path contract against a traversal layer, before one is built.
  **This is a review item: it schedules no change.** Its output is an answer -- possibly "no change
  needed" -- and any work it turns out to imply becomes its own item afterwards. Raised from the
  `windows-file-watcher` side while recording
  [D-85](../windows-file-watcher/DESIGN-NOTES.md#d-85) and the shared principle in
  [the workspace design notes](../../DESIGN-NOTES.md#path-contracts-follow-path-construction);
  flagged here because that principle's *building* half lives in this crate and nothing else would
  bring it up.
  **Context, not conclusions.** [lib.rs](src/lib.rs) states that recursive traversal belongs in a
  separate layer composing this one, so that layer does not exist yet and the contract has never been
  exercised by the consumer it was designed for. Facts noted while reading, each of which the review
  should confirm rather than take on trust:
    - [D-7](DESIGN-NOTES.md#d-7) accepts an *ordinary* path only if both the input and the
      `GetFullPathNameW`-resolved form fit `MAX_PATH`, deliberately, so acceptance does not depend on
      the host executable's `longPathAware` manifest.
    - Win32 has no relative open, so descending means appending to an absolute path -- from a path
      already at or near that cap.
    - The contract's stated remedy for an over-limit path is "supply a fully qualified `\\?\` path",
      which for a traversal layer means converting partway down rather than at the caller's boundary.
    - `EnumerationRequest::path()` is `pub` and returns the stored resolved form, so a traversal layer
      has access to a Win32-normalised base rather than only the caller's original string.
  **Questions the review should answer.** Does D-7's contract still hold once descent is a real
  consumer, or does the `MAX_PATH` cap on ordinary paths become reachable in normal use? If a
  traversal layer must move into `\\?\` form mid-descent, is that conversion specified anywhere, and
  is it the same conversion for every namespace the crate accepts -- a UNC path needs `\\?\UNC\...`
  rather than a bare prefix, and a `\\.\` device form does not take one at all? Does
  manifest-independence still mean what it should for requests a traversal layer *derives* rather than
  receives? And is `path()`'s resolved form the intended base for that, stated as such, rather than
  something a traversal layer is left to infer?
  **The trap worth naming.** Converting *before* Win32 has normalised under the caller's own parsing
  mode silently reinterprets the path; converting *after* preserves it. Both crates reached that rule
  independently, which is why it is recorded once at the workspace level -- but reaching for the
  prefix at the wrong moment is exactly what a traversal layer under length pressure would do.