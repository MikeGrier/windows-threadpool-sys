---
applyTo: "**/*.rs"
---

# Rust instructions

Binding for every `.rs` file in this workspace, and for any reviewer -- human or
automated -- forming a judgement about whether Rust in this repository compiles.

## Language baseline

**The values below are restatements. The authoritative sources are
[rust-toolchain.toml](../../rust-toolchain.toml) and the `[workspace.package]`
table of the root [Cargo.toml](../../Cargo.toml); if either disagrees with this
section, it wins and this section is the bug.**

| Property | Value | Declared in |
|---|---|---|
| Edition | **2024** | root `Cargo.toml`, `[workspace.package] edition` |
| MSRV | **1.98** | root `Cargo.toml`, `[workspace.package] rust-version` |
| Pinned toolchain | **1.98.0** | `rust-toolchain.toml`, `[toolchain] channel` |
| Target | Windows only; every item is behind `cfg(windows)` | -- |

Every crate inherits the first two with `edition.workspace = true` and
`rust-version.workspace = true`, so a crate's own `Cargo.toml` shows a pointer
rather than a value. **Reading one crate manifest is not enough to learn the
baseline** -- it has to be resolved against the root table above.

CI builds, tests, and lints on the floating `stable` toolchain (each job sets
`RUSTUP_TOOLCHAIN: stable` to override the directory-scoped pin), with one
dedicated `MSRV check 1.98` job that pins 1.98.0. So code must compile on 1.98.0
and must not depend on anything newer.

### What 1.98 permits that older Rust does not

The MSRV is far ahead of most Rust in circulation, and the gap is the source of a
recurring false "this will not compile" finding. Anything stabilised at or below
1.98 is fair game and needs no import, no feature gate, and no apology.

The specific case that has already cost review rounds:

- **`size_of`, `size_of_val`, `align_of`, and `align_of_val` are in the prelude**
  as of **Rust 1.80** (released 2024-07-25). They are written **bare**:

  ```rust
  let n = size_of::<TOKEN_STATISTICS>();
  ```

  `use std::mem::size_of;` is **not** required and is not the house style. A
  file that calls `size_of::<T>()` with no such import is correct, not broken.
  `std::mem` still owns `ManuallyDrop`, `MaybeUninit`, `swap`, `replace`,
  `take`, and `transmute`, which are **not** in the prelude and are still
  imported normally -- so the presence of a `use std::mem::...` line next to a
  bare `size_of` call is expected, not a contradiction.

More generally, treat everything in the **1.80 -> 1.98** window as available.
That window is under-represented in most training data, so an automated reviewer
is systematically likely to flag it; before reporting that any such construct
fails to compile, build it.

### Verify a compile claim before reporting it

**Do not report "this will not compile" on the basis of reading a file.** The
claim is cheap to check and expensive to get wrong:

```
cargo check --all-targets
```

The root `Cargo.toml` declares no `default-members`, so this covers every crate
and every test target in the workspace. Run it via the `cargo_check` MCP tool
(see the Cargo section of [copilot-instructions.md](../copilot-instructions.md)),
never a terminal.

## Pre-commit gate

If any staged file has a `.rs` extension, all three steps below **must** pass
before `git commit`. This is the full form of the gate summarised in
[copilot-instructions.md](../copilot-instructions.md).

1. **Format.** Run `cargo fmt` (the `cargo_fmt` MCP tool). Commit *every* file it
   reformats, including files outside the current task's scope -- a fmt run
   rewrites everything in the formatted scope, and leaving that diff unstaged
   leaves the tree dirty and defers the cleanup onto the next contributor.
2. **Lint.** Run `cargo clippy --all-targets` (the `cargo_clippy` MCP tool) and
   resolve every diagnostic. Do not commit while any remains.
3. **No inline test modules.** Unit tests live in a sibling `tests.rs` reached by
   `#[cfg(test)] mod tests;`, never in an inline `#[cfg(test)] mod tests { ... }`
   block. Verify the staged diff adds none:

   ```powershell
   git --no-pager diff --cached -U0 -- "*.rs" | Select-String '^\+\s*mod tests\s*\{'
   ```

   Any hit is a blocking violation; move the tests into a sibling `tests.rs`
   first.

Tests must pass before committing. A pre-existing failure unrelated to the
current change does not block the commit but must first be recorded in the
nearest `UNRESOLVED-TEST-FAILURES.md` -- a per-component file, created next to
the component's `CHECKLIST.md` if it does not yet exist. The only one that
currently exists is
[crates/windows-file-watcher/UNRESOLVED-TEST-FAILURES.md](../../crates/windows-file-watcher/UNRESOLVED-TEST-FAILURES.md).

## Testing

Prefer `cargo_nextest_run` for unit and integration tests where cargo-nextest is
available, falling back to `cargo_test`. Either way nextest does **not** run
doctests, so run those separately with `cargo_test` and `doc: true`. A milestone
is not complete while any doc test fails or is unrun.

This repository targets a single operating system, so depending on Windows does
not by itself make a test an integration test. Placement is decided by duration
and by whether the test must cross a real process, filesystem, network, device,
or OS boundary.
