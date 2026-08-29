# Checklist: automated-reviewer language baseline

Close the gap that let an automated PR review on
[#46](https://github.com/MikeGrier/windows-threadpool-sys/pull/46) raise seven
false "`size_of` is not in scope, this will not compile" findings against code
that builds clean on this workspace's pinned toolchain.

The finding was not careless. Three independent things had to go wrong together,
and two of them are ours:

1. **The baseline was invisible in the diff.** [rust-toolchain.toml](rust-toolchain.toml)
   (`channel = "1.98.0"`) is unchanged by a normal PR, so it never appears. Root
   [Cargo.toml](Cargo.toml) did appear, but its only hunk was the `members` list
   ending at line 16, while `edition = "2024"` and `rust-version = "1.98"` sit at
   lines 22-23 -- six lines past the end of the hunk, outside the three lines of
   context. The three new crate manifests appeared too, but they say
   `edition.workspace = true` / `rust-version.workspace = true`, which is a
   pointer, not a value, and the table it points at was not in any hunk.
2. **Our own code supplied confirming evidence for the wrong answer.** Several
   pre-existing sites import or qualify `size_of` in the pre-1.80 style, so a
   reviewer pattern-matching locally sees a real in-repo precedent that the new
   bare-prelude form is a missing import. The clearest pair is the same struct,
   field, and conversion written both ways.
3. **The base rate is against the modern form.** `size_of` / `size_of_val` /
   `align_of` / `align_of_val` entered the prelude in Rust 1.80, released 2024-07-25,
   so nearly all existing Rust writes the import.

Nothing here can fix (3), but (1) and (2) are ours to remove, and the blind spot is
not specific to `size_of`: it covers every language and library change in the
1.80 -> 1.98 window.

Completed groups are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).
This file is feature-scoped and is deleted when every item is done.

## M1 -- State the baseline where an automated reviewer will read it

- [ ] **RB-1** -- Create [.github/instructions/global.rust.instructions.md](.github/instructions/global.rust.instructions.md).
  [.github/copilot-instructions.md](.github/copilot-instructions.md) already cites this
  path twice (at the "Rust pre-commit gate" bullet and at the milestone-boundary build
  step) as the home of "the full gate", but the file does not exist -- so the one
  natural home for a Rust language baseline is a dangling reference. Create it as the
  authoritative Rust document: the language baseline (edition, MSRV, pinned toolchain,
  and the consequence that 1.80+ prelude items are used unqualified), then the full
  pre-commit gate the root file summarises. Convert both existing references into
  clickable relative links per the repository's cross-reference rule.

- [ ] **RB-2** -- Add a short Rust language baseline section to
  [.github/copilot-instructions.md](.github/copilot-instructions.md). That file is the
  one an automated PR reviewer is known to read, and it currently contains zero
  occurrences of `edition`, `MSRV`, `1.98`, `rust-version`, or `prelude` across its
  whole length. The section states the edition and MSRV outright -- a reviewer cannot
  follow a link out of a diff -- names the prelude items this unlocks, and points at
  RB-1's file for the rest. Depends on RB-1.

- [ ] **RB-3** -- Normalise the pre-1.80 `size_of` call sites so the workspace stops
  contradicting itself. Every site that imports `std::mem::size_of` or writes
  `std::mem::size_of` / `mem::size_of` / `std::mem::size_of_val` becomes the bare
  prelude form, and any import left unused by the change is dropped. This is the
  confirming evidence in (2) above: while it stands, a reviewer that pattern-matches
  against repository precedent will keep reaching the same wrong conclusion, whatever
  the instruction files say. `ManuallyDrop` and `MaybeUninit` imports are untouched --
  they are not in the prelude.

- [ ] **RB-4** -- Guard the restated MSRV against drift in CI. RB-1 and RB-2 both write
  the edition and MSRV into prose, which the repository's own "prefer a derived fact to
  a restated one" rule identifies as the setup for restatement drift: the authoritative
  values live in [Cargo.toml](Cargo.toml) and [rust-toolchain.toml](rust-toolchain.toml),
  and nothing currently detects the day they disagree with the documents. Add a check to
  [.github/workflows/ci.yml](.github/workflows/ci.yml) that reads both manifests and
  fails if either instruction file states a different edition, MSRV, or pinned channel.
  Depends on RB-1 and RB-2, since it asserts against what they write.
