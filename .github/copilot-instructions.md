# Copilot Instructions

Use LF line endings.

## PRIME DIRECTIVE — never defer work for a perceived lack of need

**Do not defer, drop, or narrow a feature merely because you cannot currently see a
consumer/client for it.** "Nothing calls this yet," "no client needs it now," "YAGNI,"
and "we'll add it when a caller appears" are **not** acceptable reasons to defer work
that the design or the task calls for. Absence of a visible client is not a blocker — it
is just absence of information, and it must never be used as a justification to shrink
scope.

The **only** legitimate reason to defer a piece of work is a genuine **blocking factor** —
a real dependency that does not yet exist, a hard technical impossibility, an unresolved
design decision, an external constraint, or a resource/tooling gap. When you hit a genuine
blocker:

1. **Name the blocker explicitly** — state precisely what is blocking the work and why.
2. **Raise it with the user** and discuss how to work *around* the block (build the missing
   dependency, sequence differently, find an alternative approach) rather than silently
   parking the feature.
3. **Only defer if the user agrees** the block is real and cannot be worked around now — and
   record the specific blocker (not "no client") as the deferral reason.

When in doubt, **implement it now** and surface the decision, rather than deferring and
moving on. If you find yourself writing a deferral note whose rationale is "no consumer
yet," "not needed for this witness," or any variant of "we don't need it," **stop** — that
is exactly the anti-pattern this directive forbids. Convert it into either (a) implementing
the work, or (b) an explicit blocker conversation with the user.

## PLATFORM INTEGRITY — layers and platforms are isolation boundaries, never collapsed for convenience

**This repository is built as layers and platforms on purpose. Do not omit, tune,
merge, delete, or "optimize" a feature in a way that subverts the intent of that
layering.** Treating a layer boundary as an inefficiency to be collapsed — folding two
paths into one, dropping a lower baseline to serve a higher one, skipping a platform's
feature because the currently-visible goal does not exercise it — is a **recurring,
costly anti-pattern** that undoes deliberate design and delays the project. When you
see what looks like a "layering inefficiency," **assume it is deliberate isolation and
ask about intent before proposing to consolidate it.** An assistant's efficiency
instinct is never a licence to redesign a plan the engineer built deliberately; help
coax the plan out and execute it, do not subvert it.

Three binding rules follow:

1. **Duplicate-then-decide is standard procedure for speculative work.** Building a new
   capability as a *separate, duplicated* path alongside the existing one — so the
   speculative work proceeds *without interfering with or disrupting* the working path —
   is correct and expected, **especially** for a feature whose dependency is not yet
   certain. The duplication is **not** debt to be minimized; it is the mechanism that
   keeps the working layer stable while the new one is proven. The **merge-or-delete**
   decision is made **when the new path is proven**, never pre-empted mid-development,
   and never traded away for a testing/efficiency shortcut that re-couples the paths.
   Track the *when-done* merge-or-delete decision so it is not forgotten — a duplicated
   path silently becoming permanent because nobody circled back is the real failure
   mode, not the duplication.

2. **Depend on specified primitives, never on incidental current behavior.** A consumer
   must bind to a layer's *specified* contract — its primitives, its documented
   semantics — never to how the layer happens to behave right now. Leaning on an
   implementation's incidental behavior (e.g. relying on how a code generator currently
   emits code plus a specific CPU's memory-ordering side effects, instead of using the
   specified atomic / memory-ordering primitives) is a correctness trap that has already
   cost days of preventable waste. This is the consumer-side twin of "Design Autonomy —
   Behavior is owned, never inherited from dependencies" below: each layer owns its
   behavior, and each consumer binds only to that owned specification.

3. **Do not narrow the platform to serve the visible goal.** Every platform component
   must remain a *level* platform — its lower baselines and less-exercised features are
   first-class, not optional trimmings to cut because the current task does not need
   them. This is the PRIME DIRECTIVE above viewed from the platform side: absence of a
   visible consumer for a layer or feature is never a licence to omit, downgrade, or
   collapse it.

When a genuine constraint (a real cost, a real blocker) makes a layer duplication or a
baseline truly unaffordable, that is a **decision for the engineer driving the work**,
raised explicitly per the PRIME DIRECTIVE's blocker protocol — never a shortcut an
assistant takes unilaterally in the name of efficiency.

## Line endings in tool parameters

All text content passed to tpu tools (`content`, `replacement`, `data` in edit ops) is
automatically normalized to LF before processing. You do not need to worry about whether
the text you send uses LF or CRLF — tpu handles the conversion. The file's existing
line-ending convention (LF or CRLF) is preserved on disk automatically.

**Creating a new file: use `tpu_write_file`, never the built-in `create_file`.**
This repo is LF-only. The built-in `create_file` writes **CRLF** on Windows even when
the `content` you pass uses LF, and git then rejects the commit with
`fatal: CRLF would be replaced by LF`. `tpu_write_file` writes LF by default for a new
file (verified: identical content is 17 bytes via `tpu_write_file` vs 20 bytes via
`create_file`). The same "prefer `tpu_*` over the built-in file tools" rule applies to
edits (`tpu_replace_in_file` / `tpu_edit_file`), not only to PowerShell/shell.

## Terminal / Git rules — hang prevention

**These rules prevent terminal hangs that freeze the session.**

- Every `git` command that can produce paged output **must** be run with
  `git --no-pager <subcommand>`. The list below is illustrative, not exhaustive:
  `diff`, `show`, `log`, `blame`, `reflog`, `stash list`, `branch -v`,
  `shortlog`, `tag -n`, `whatchanged`, `grep`. **If unsure whether a `git`
  subcommand may page, use `--no-pager`.**
- Never run `git commit` without `-m "…"`. Commit messages must be a **single
  line** when supplied via `-m`. For longer messages write the message to a
  file under `.scratch/` and use `git commit -F .scratch/<file>`. Never use
  PowerShell here-strings (`@"…"@`) or embedded newlines inside `-m` —
  PowerShell will either hang waiting for terminator or pass `\n` literally.
- **Rust pre-commit gate:** If any staged file has a `.rs` extension, you
  **must** run `cargo fmt` then `cargo clippy --all-targets` (via the
  `mcp_cargo-mcp_cargo_fmt` and `mcp_cargo-mcp_cargo_clippy` tools) and fix
  all issues before running `git commit`. Any issue reported by either tool
  must be resolved before continuing — do not proceed to `git commit` while
  any formatting diff or Clippy diagnostic remains. You **must** also verify
  the staged diff adds no inline `#[cfg(test)] mod tests { ... }` block
  (`git --no-pager diff --cached -U0 -- "*.rs" | Select-String '^\+\s*mod tests\s*\{'`);
  any hit is a blocking violation — move those tests into a sibling `tests.rs`
  first. See the full gate in
  `.github/instructions/global.rust.instructions.md`.
- **Commit every file `cargo fmt` reformats, even outside your task's scope.**
  `cargo fmt` rewrites *all* files in the formatted scope, not just the ones you
  edited — so a run can clean up a pre-existing formatting drift in a file your
  current change never touched. When that happens you **must** stage and commit
  that reformatting too (in the same commit, or a separate `Format <file> (cargo
  fmt)` commit if you want to keep it isolated). **Never** leave a
  `cargo fmt`-produced diff unstaged on the grounds that the file is "out of
  scope": doing so leaves the working tree dirty, silently defers the cleanup,
  and lets the next contributor mistake it for their own change. If a fmt run
  touches files you did not expect, that is expected behavior — commit them, do
  not `git checkout` them away.
- Never run `git pull` or `git merge` without `--no-edit`. **`--no-edit` is a
  `merge`/`pull` flag only** — `git rebase` does **not** accept `--no-edit` (passing
  it makes `git rebase` fail and print its usage). A plain, non-interactive
  `git rebase <upstream>` opens no editor on its own, so it needs no such flag: run
  it bare (never add `--no-edit`, and never use `-i`).
- Never run interactive commands: `git rebase -i`, `git add -p`, etc.
- Do not use `less`, `more`, or any other interactive pager.
- Never use PowerShell multi-line string operators (`@"…"@`) in terminal commands.

## Cargo commands — use cargo-mcp tools, never the terminal

Always use the `cargo_*` MCP tools instead of running `cargo` commands in a terminal.
This applies even inside a larger workflow — do not switch to the terminal for cargo
just because a previous step used the terminal.

**Always set `working_dir` to the workspace root.** Every cargo-running tool accepts
a `working_dir` parameter. Omitting it defaults to the cargo-mcp server's own working
directory, which may select the wrong manifest or fail to find a toolchain. Pass the
absolute workspace-root path on every call; the server rejects a directory from which
no `Cargo.toml` can be found by walking upward.

**This is a Windows-only workspace, so there is no cross-platform exception.** The
cargo-mcp server drives the **host** (Windows) toolchain, which is the only toolchain
these crates target; every item is behind `cfg(windows)` and CI builds, tests, and
lints exclusively on Windows. Every cargo invocation therefore goes through the
`cargo_*` MCP tools with no WSL/Linux carve-out.

| MCP tool | Replaces |
|---|---|
| `cargo_metadata` | `cargo metadata` |
| `cargo_check` | `cargo check` |
| `cargo_build` | `cargo build` |
| `cargo_test` | `cargo test` |
| `cargo_clippy` | `cargo clippy` |
| `cargo_fmt_check` | `cargo fmt --check` |
| `cargo_fmt` | `cargo fmt` |
| `cargo_tree` | `cargo tree` |
| `cargo_doc` | `cargo doc` |
| `cargo_clean` | `cargo clean` |
| `cargo_update` | `cargo update` |
| `cargo_fix` | `cargo fix` |
| `cargo_add` | `cargo add` |
| `cargo_remove` | `cargo remove` |
| `cargo_publish` | `cargo publish` |
| `cargo_nextest_run` | `cargo nextest run` (requires cargo-nextest) |
| `cargo_nextest_list` | `cargo nextest list` (requires cargo-nextest) |
| `cargo_setup` | *(no terminal equivalent)* |
| `cargo_diagnostic` | *(no terminal equivalent)* |

### Boolean arguments

Boolean flags (`all_targets`, `release`, `workspace`, `lib`, `tests`,
`all_features`, `no_default_features`, `locked`, `offline`, …) expect a JSON
boolean (`true`/`false`). The server also accepts loose forms (`"true"`,
`"1"`, `"yes"`, `"on"` and their negatives, case-insensitive, plus the
integers `0`/`1`), but prefer the native boolean. The same coercion applies
to nested boolean fields such as `cargo_test`'s `test_filter.include_ignored`.

An unrecognised value (e.g. `"maybe"`, an object, an integer other than
`0`/`1`) is treated as `false` and the server emits a `warning` MCP
notification naming the field. **If a flag you expected (`--all-targets`,
`--release`, …) is missing from the echoed `x-cargo-mcp-invocation` argv,
look for that warning — you almost certainly sent the boolean in an
unrecognised shape.**

### Test selection — which knobs belong to which tool

`cargo_test` and `cargo_nextest_run` have separate, non-interchangeable selection
parameters. Unknown arguments are rejected with an actionable error instead of
silently running the entire suite:

- **`cargo_test` only:** `test_name` (substring, or `exact: true`), `test_filter`
  (`{ "pattern": "<regex>", "include_ignored": <bool> }`),
  `per_test_timeout_secs` (with `test_filter`), and `doc: true`.
- **`cargo_nextest_run` only:** `filter` (substring over test names) and
  `filter_expr` (the nextest `-E '<expr>'` filterset DSL).

Do not pass `test_filter`, `test_name`, `per_test_timeout_secs`, or `doc` to
`cargo_nextest_run`, and do not pass `filter` or `filter_expr` to `cargo_test`.
For a very large suite, narrow it with the appropriate filter or intentionally
disable the wall-clock cap with `timeout_secs: 0`.

### cargo_test — phases and timeouts

Every `cargo_test` and `cargo_nextest_run` call normally has independently timed
build (`cargo test --no-run`) and test-execution phases. `timeout_secs` applies to
each phase on its own clock, so build time is not charged against execution time;
timeout errors identify which phase fired. Build warnings are preserved in the
combined output.

Doctests are the exception: `doc: true` runs as a single `doc test` phase because
Cargo does not support `--doc --no-run`. Combining `doc: true` with `no_run: true`,
`test_filter`, or `bisect` is rejected before Cargo starts.

`cargo_test` has three timeout controls:

- **`timeout_secs`** bounds both build and execution phases independently.
  Unfiltered calls default to `cargo-mcp.test.timeoutSecs` (**30 s** in the VS
  Code extension); filter mode has no overall default. Pass `0` to disable it.
- **`test_timeout_secs`** overrides only the execution budget for unfiltered
  calls. When used alone, the build is unbounded. When combined with
  `timeout_secs`, it can only tighten that overall cap. It is rejected with
  `doc: true`, `test_filter`, or `bisect`; pass `0` to omit the override.
- **`per_test_timeout_secs`** applies only in `test_filter` mode. In batched mode
  it is an idle watchdog reset by each test-completion line; in per-test mode it
  is a wall-clock cap for each invocation. The default is the server setting,
  with a **30 s** fallback when that setting is absent or `0`. Pass `0` to fully
  disable it for the call.

Raise or disable limits for legitimately slow suites and lower them when checking
for hangs. Use `test_timeout_secs` when a cold build should remain unbounded but
test execution still needs a cap.

### cargo_test — per-test execution mode

When the `cargo-mcp.test.perTestExecution` VS Code setting is enabled, each
matched test in `test_filter` mode runs as its own `cargo test -- --exact <name>`
invocation (one cargo process per test). Benefits: the hung/timed-out test is
named unambiguously (in the invocation `argv`), `per_test_timeout_secs` becomes a
plain wall-clock cap rather than an idle watchdog, and tests run serially so there
is no ambiguity about which test was executing. Cost: one cargo launch per matched
test (~200–500 ms each) — fine for targeted re-runs, but use batched mode for
broad filter runs.

### Per-call environment variables (`env`)

Every `cargo_*` tool that spawns cargo accepts an optional `env` object that
sets or unsets environment variables on the cargo subprocess for that one
call. Keys are env var names; values are a string (set) or `null` (unset).
The map layers on top of cargo-mcp's defaults (`CARGO_TERM_COLOR`,
`NO_COLOR`, `RUSTC`), so a caller-supplied value wins.

Use this — never a terminal — to apply a one-shot debug knob such as
`RUSTFLAGS`, `RUST_LOG`, `RUST_BACKTRACE`, `RUSTC_BOOTSTRAP`, or a
compiler-internal dump like `FIREBIRD_DUMP_MIR`:

```json
{ "env": { "FIREBIRD_DUMP_MIR": "1", "RUST_BACKTRACE": "1" } }
```

Do **not** use `env` for permanent/project-wide config (put that in
`Cargo.toml`, `.cargo/config.toml`, or `rust-toolchain.toml`) or for
secrets (the block is visible via OS process inspection).

### Redirecting full output to a file (`output_path`)

`cargo_check`, `cargo_build`, `cargo_test`, `cargo_clippy`, and `cargo_doc`
accept an optional `output_path`: a relative path (under the working
directory; no `..`; parent must already exist) that receives the **complete**
NDJSON output. When set, the tool result is a compact SUMMARY (invocation
header, an `x-cargo-mcp-output-file` pointer, all `level: error` messages,
`build-finished`, stderr, status trailer, and — for `cargo_test` — libtest
summary/failure markers); warnings, passing-test lines, artifact records, and
captured `println!` replays are dropped from the summary but preserved in the
file.

Use `output_path` when the full transcript would bloat context (long
`cargo_test` runs, large workspaces) instead of piping to a temp file. Per
the scratch-directory rule below, target `.scratch/` (e.g.
`".scratch/test-run.ndjson"`). Read the summary first; if `exit_code` is
non-zero or failure markers appear, open the file for the full transcript.

### Hang / slow-test bisection (`bisect`)

`cargo_test` and `cargo_nextest_run` accept an optional `bisect` object that
switches the call into a bisection engine for **finding which test hangs or runs
long**. It builds once, enumerates every test, then runs groups of tests under a
short kill-deadline (always single-threaded). Any group that times out (hangs) or
exceeds the slow threshold is recursively subdivided until the culprit test(s)
are isolated. It works identically on both tools (it runs the compiled libtest
binaries directly).

Only `group_timeout_secs` is required:

```json
{ "bisect": { "group_timeout_secs": 10 } }
```

Knobs (all optional except `group_timeout_secs`):

- `group_timeout_secs` (REQUIRED) — per-group kill-deadline in seconds; a group
  that exceeds it is treated as hung.
- `slow_threshold_secs` — must be < `group_timeout_secs`; a group that finishes
  but takes longer than this is `slow` and gets subdivided. Omit to detect hangs
  only.
- `split_factor` (default 2) — sub-groups per subdivision (binary search).
- `split_percent` — alternative to `split_factor`: yields ceil(100/p) sub-groups
  (mutually exclusive with `split_factor`).
- `min_group_size` (default 1) — stop subdividing at this size and report the
  members as culprits.
- `initial_group_size` / `initial_groups` — shape the first-level groups
  (mutually exclusive); default is one group of all tests.
- `max_rounds` (default 32) — cap on subdivision depth.
- `pattern` — RE2 regex; only matching `module::path::test_name` tests
  participate. `include_ignored` — also bisect `#[ignore]` tests.

The result is an NDJSON stream of `x-cargo-mcp-bisect-config`,
`x-cargo-mcp-bisect-group`, `x-cargo-mcp-bisect-culprit`, and
`x-cargo-mcp-bisect-summary` records; `output_path` is honoured (full body to
file, compact summary inline). The call is an error when any culprit is found.

### Reading cargo_test output

`cargo_test` returns a strict NDJSON stream. Parse it line-by-line; every
non-blank line is a JSON object. The `reason` field identifies the record type:

| `reason` | Content | Key fields |
|---|---|---|
| `x-cargo-mcp-invocation` | Effective command and working dir (first line) | `argv`, `cwd` |
| `compiler-message` | Compilation error or warning | `message` (rustc diagnostic) |
| `build-finished` | Build phase outcome | `success` (bool) |
| `x-cargo-mcp-test-output` | One line of libtest harness output or captured `println!` | `text` |
| `x-cargo-mcp-stderr` | `eprintln!` and other test stderr (when non-empty) | `text` |
| *(last line)* | Exit status | `status` (`"success"` or `"error"`), `exit_code` (on error) |

`println!` inside tests is captured by libtest and replayed as
`x-cargo-mcp-test-output` lines only when the test fails (standard libtest
behaviour). `eprintln!` bypasses libtest capture and always appears in
`x-cargo-mcp-stderr`.

### Recommended: cargo-nextest

This workspace contains a `.config/nextest.toml`, so prefer `cargo_nextest_run`
over `cargo_test` for unit and integration tests. Use `cargo_test` only for
**doctests** (nextest does not run them). `cargo_nextest_list` enumerates tests as
structured JSON when you need discovery without execution.

## Scratch directory for temporary files

When you need to capture command output, test results, debug logs, build warnings, or any
other temporary/diagnostic data to a file, **always write it under the `.scratch/` directory**
at the repository root. This directory is git-ignored.

- Create `.scratch/` if it does not exist.
- Use descriptive filenames (e.g., `.scratch/test_parser_output.txt`, `.scratch/build_warnings.txt`).
- **Never** write scratch or debug files to the repository root or any source directory.

## General instructions for this repository
- All code is Copyright Mike Grier.
- All source code should include a copyright statement. The statement should be brief, a single line comment as the first line of the file which reads something like: Copyright (c) Mike Grier.
- If the source file is also part of an open source library, there may be additional lines giving the details, but in general, open source content should not be checked in to this source repository except as part of a patching process to provide a patch over defective open source dependencies which have to be addressed for security or business continuity reasons.

## Interaction Guidelines
- Prefer concise responses: minimize verbosity, reduce repetition, and avoid excessive formatting/emojis. Get straight to the point in all interactions.

## Checklist execution discipline

When executing checklist items (CHECKLIST.md files):

- **Decide which mode you are in *before* you commit (read first).** Every time you are about to commit checklist work, first determine which situation you are in:
  - **(a) Recording work that is already finished** in the working tree (the items were implemented before this rule was applied, the work arrived as one chunk, or a single coherent change happened to satisfy several items at once). **Action:** commit the completed items together in **one** commit that cites every item ID it satisfies, check them all off in the same commit, and move on. Do **not** invent extra work to retroactively tease the change apart into one-commit-per-item — that artificial "commit surgery" is exactly the end-of-stream bookkeeping this rule is meant to avoid.
  - **(b) Implementing items forward, one after another.** **Action:** follow the one-item-then-commit loop below.

  This rule exists to enforce *implementation sequencing*, **not** to dictate how finished history is sliced. Its single purpose is to stop work item N+1's concerns from leaking into work item N's implementation *while you are still writing item N* — so each item is implemented against a clean, already-landed predecessor. Separate commits are a *byproduct* of separate implementation episodes (you implemented item N, committed, then started item N+1); they are never a goal pursued on their own.
  - If finished work in the tree spans multiple items and those items were genuinely independent, that independence was already preserved by how the code was written — re-slicing the commit adds nothing, so don't spend effort classifying: just commit together (mode a). If the items were *not* independent (item N+1's concerns flowed into item N), that is a sequencing/planning error: merge the items if the coupling is minor, or re-plan if the sequencing was seriously wrong (see the re-plan bullet below). The response is to fix the plan, never cosmetic commit surgery.
- **One item at a time (mode b — implementing forward).** When you are implementing items one after another, implement exactly one checklist item, then **stop and commit**, then move on. "Stop" means: do not begin reading, planning, or editing for the next item until the current one's commit has succeeded. This is the mechanism that delivers the sequencing guarantee above; it does **not** apply retroactively to work that is already done (mode a).
- **A checklist item may legitimately be large.** "One item, one commit" is a sequencing rule, **not** a commit-size rule. There is no upper bound on the diff size, file count, or scope of a single item's commit. If an item's work is genuinely coupled — for example, an IR-schema change that requires updates across lowering, codegen, freezer, pretty-printer, and tests to compile at all — do the whole thing in one commit. Do **not** invent sub-items (`M1.1.1`, `M1.1.2`, …) to make the commit feel smaller; that is artificial work that violates the "one item, one commit" rule by turning one item into several.
- **If items are mis-structured, say so; do not paper over it.** If you discover during execution that two checklist items cannot be implemented independently (one cannot compile or pass tests without the other), that is a checklist-structuring defect. Be honest: name the defect, then either (a) commit both items together in one commit referencing both IDs (acknowledged defect), or (b) restructure the checklist first (in its own commit) so the items become independent. Do **not** silently split, tease apart, or interleave commits to disguise the coupling.
- **No batching for convenience (mode b — implementing forward).** While you are still implementing, do not *start* work on item N+1 before item N is committed *just because* the work is similar, the context is loaded, or it feels efficient to do both at once. Convenience, similarity, or shared context across adjacent items is **not** sufficient justification to pull future work forward into the current item. (This forbids *reaching ahead* during forward implementation; it does **not** require *re-slicing* work that is already finished — that is mode a above.)
- **Re-plan when execution reveals planning was wrong.** A checklist is a hopeful projection, not a contract. When execution surfaces information that invalidates the plan — items that turn out to be coupled, an item that decomposes into work the original plan didn't anticipate, an item that turns out to already be done, an item whose scope expands or contracts based on what you now know — **stop and update the checklist before continuing.** Restructuring a checklist mid-execution is normal and expected; pretending the original plan was correct and silently working around it is not. The restructure itself is a commit (with a message explaining what new information forced the change), and then the revised plan governs.
- **If items must be done together, say so and do it; don't tease apart.** Once you have decided (and recorded in the checklist if the structure is wrong) that two items must land together, commit them together in one commit citing both IDs. Do **not** try to "unthread" a coupled implementation into per-item commits after the fact — that is fiction, not history.
- **Commit immediately after each item.** In mode b (implementing forward), the commit must happen before moving to the next item. In mode a (recording already-finished work), a single commit citing all the item IDs satisfies this.
- **Commit message format: a Conventional Commits subject line, with the checklist trailer in the body.**
  `release-please` (see "Release process" in [DEVELOPMENT.md](DEVELOPMENT.md)) drives every crate's version
  bump and CHANGELOG **only** from Conventional Commits subject lines (`type(scope)!: summary`); a subject
  that doesn't match that grammar is invisible to it, no matter how much checklist work the commit records.
  The mandatory `Completed item:` provenance is therefore never the subject line — it moves to the body, and
  the subject line carries the Conventional Commits header instead. Every checklist commit has this shape:

  ```
  <type>(<scope>)[!]: <short summary of what changed>

  Completed item: <item-id>: <full item text>
  ```

  - **`<type>`** — pick the type that matches the change's nature, same as any Conventional Commit:
    `feat` (new capability), `fix` (bug fix), `docs`, `test`, `refactor`, `perf`, `chore`, `build`, `ci`.
    Checklist bookkeeping that is not itself a code change (re-planning, archiving a milestone, recording a
    design decision) is normally `docs:` or `chore:`, matching this repo's existing history (e.g.
    `docs: archive M8, the fifth review round`).
  - **`<scope>`** — the crate the commit's diff lives under, using this repo's established short names:
    `threadpool` (`windows-threadpool-sys`), `overlapped-io` (`windows-overlapped-io-sys`), `wtf-string`
    (`wtf-string`), `file-watcher` (`windows-file-watcher`), `ioring` (`windows-ioring-sys`), `topology`
    (`windows-topology-sys`). Omit the scope for a commit with no single
    crate home (root-level docs, workspace-wide chores). A commit that touches more than one crate's `src/`
    should be split so each Conventional Commit scope stays accurate — release-please attributes a bump by
    which package path changed, and a misleading scope only confuses human readers, but a commit spanning
    two crates' actual code still needs its own subject per crate if both are meant to receive a bump.
  - **`!` after `<type>`/`<scope>`** — required whenever the commit is a breaking change (removes or changes
    an existing guarantee, contract, or public API in a way existing callers must react to). This is what
    tells release-please to cut a **major** bump instead of minor/patch; do not rely on prose alone to
    signal a breaking change.
  - **The `Completed item:` body lines are unchanged in content and purpose** — they are what checklist
    hygiene and future archaeology depend on, and are never dropped just because the subject line now
    carries the Conventional Commits grammar. When one commit records several already-finished items
    together (mode a), use one `Completed item:` line per item ID, e.g.:
  ```
  feat(file-watcher): add FunctionCall support to the filter expression grammar

  Completed items: SF-1, SF-2, SF-3

  Completed item: SF-1: Add extensible FunctionCall variant to FilterExpr
  Completed item: SF-2: Wire FunctionCall through the evaluator
  Completed item: SF-3: Add FunctionCall parser support
  ```
- **Check the item off** in CHECKLIST.md (change `- [ ]` to `- [x]`) and include that change in the same commit.
- After the commit, pull / rebase from origin then push back to origin
- **Tests must pass** before committing. Run the appropriate test command (per the language-specific instructions) after each item and fix failures before committing. Pre-existing failures unrelated to the current item do not block the commit, but must be recorded in `UNRESOLVED-TEST-FAILURES.md` (see language-specific instructions for the convention) before committing. When such a failure is later **resolved**, do not delete its entry — move it out of `UNRESOLVED-TEST-FAILURES.md` into a sibling `RESOLVED-TEST-FAILURES.md` (append-only) under a `## Resolved <YYYY-MM-DD HH:MM:SS ±hh:mm> — <description>` heading recording the date and time the resolution was finalized, in the same commit that removes it from the unresolved file.
- **When the last item in a CHECKLIST file is completed**, update its PLANS.md entry to "completed" in the same commit.
- **Cross-component handoff callouts.** When the next required action in a checklist sequence shifts to a different source-component (see "Source-Components" above) — i.e. the next dependency-ordered item cannot be worked in the current component because it lives in another component — the item whose completion triggers the shift must end with an explicit handoff callout naming the destination component, milestone, and work item ID. Use the reciprocal form on the destination side: the destination's first dependent item must carry a `CROSS-COMPONENT PREREQUISITE` callout naming the source component / item that must land first, and (if control returns) a `CROSS-COMPONENT HANDOFF` callout at the end pointing back. Recommended format (markdown blockquote so it stands out when scanning):
  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `<component-path>` -> `<milestone-id>` -> `<work-item-id>` (`<short title>`). See [`<path-to-CHECKLIST.md>`](...).
  The goal is that a reader executing a checklist linearly never has to infer cross-component dependencies from surrounding prose — the boundary is always called out at the exact item where the handoff occurs.

## 7-bit ASCII only in Copilot-maintained planning and design docs

Every Copilot-maintained planning and design document **must contain only 7-bit
clean ASCII** (every byte in `0x00`-`0x7F`; no code point above `U+007F`). This
governs:

- checklist files: `CHECKLIST.md`, `CHECKLIST-<feature>.md`,
  `COMPLETED-CHECKLIST.md`;
- plan indexes: `PLANS.md`, `COMPLETED-PLANS.md`;
- design docs: `DESIGN-NOTES.md`, `DESIGN-NOTES-*.md`, `DESIGN-RATIONALE.md`,
  `DESIGN-INSTRUCTIONS.md`, any other `DESIGN-*.md`, and every file under a
  `design-sessions/` directory;
- the test-tracking siblings `UNRESOLVED-TEST-FAILURES.md` /
  `RESOLVED-TEST-FAILURES.md`.

Why: non-ASCII punctuation in these files has repeatedly been round-tripped
through the wrong code page by some tool and corrupted into multi-layer mojibake
that is expensive to detect and repair. 7-bit ASCII round-trips cleanly through
every editor, code page, and shell, so this class of damage cannot recur.

Write ASCII spellings instead of the non-ASCII characters that tend to appear in
prose (described by name + code point so this rule file stays ASCII too):

- em dash (U+2014) / en dash (U+2013)  ->  `--`, or a single `-`
- right arrow (U+2192)  ->  `->`  ;  left arrow (U+2190)  ->  `<-`
- set membership (U+2208)  ->  the word `in`
- multiplication sign (U+00D7)  ->  `x` or `*`
- curly quotes (U+201C/U+201D/U+2018/U+2019)  ->  straight `"` / `'`
- horizontal ellipsis (U+2026)  ->  `...`
- non-breaking space (U+00A0)  ->  a normal space (U+0020)
- any other code point above U+007F  ->  its ASCII equivalent or a short word.

Some mandatory format tokens are shown elsewhere in this file with non-ASCII glyphs in
their *description*; in the ASCII-bound files above, always write their ASCII spellings
instead. In particular: the horizon-milestone bucket (the letter `M` followed by the
infinity sign, U+221E) is written **`M-inf`**, and any right-arrow glyph (U+27A1 or
U+2192) that appears in a format token is written **`->`**.

Apply this to the content you author or edit in these files going forward.

## Markdown cross-references must be clickable links

Every time a project document (CHECKLIST.md, COMPLETED-CHECKLIST.md, PLANS.md,
COMPLETED-PLANS.md, DESIGN-NOTES.md, DESIGN-RATIONALE.md, COMPONENT.md, README.md,
design-session files, or any other repository markdown file) **mentions another
markdown document by name, that mention MUST be written as a clickable markdown
link with a relative path** — never as bare text and never as an inline-code
filename. This lets a reader Ctrl+Click (Cmd+Click) the reference in VS Code to
open the target document.

Rules:
- **Relative paths only**, resolved from the directory of the *referencing*
  file. A sibling document in the same directory is `[NAME.md](NAME.md)`; a
  document in another crate is `[NAME.md](../<other-crate>/NAME.md)`. Do **not**
  use absolute paths, `file://` URIs, or workspace-root-relative paths unless a
  relative path is impossible.
- **The link text is normally just the document's file name** (e.g.
  `[DESIGN-NOTES.md](../firebird/DESIGN-NOTES.md)`), so the reader still sees
  which file is meant. Any qualifier that disambiguates *which* copy (e.g.
  "firebird", "coot-pass2", "this crate's") stays as prose **outside** the link.
- **Verify the target exists** before writing the link, so links are never
  broken on creation.
- **When a reference points at a specific decision ID or heading**, keep the ID
  as inline code *after* the link (e.g.
  `[DESIGN-NOTES.md](../firebird/DESIGN-NOTES.md) -> `D-GRAFT-1``). Linking to the
  heading anchor (`...DESIGN-NOTES.md#d-graft-1`) is encouraged when the anchor
  slug is known to be correct, but linking to the file is always acceptable.
- **This applies to references to non-markdown repository files too** (source
  files, `Cargo.toml`, JSON grafts) whenever the intent is "go look at this
  file" — make them relative-path links as well. Inline code without a link is
  reserved for symbols, identifiers, and snippets, not for file references the
  reader is expected to open.
- When **editing or adding to an existing document**, convert any bare or
  inline-code markdown-document references you touch into links as part of the
  edit; do not leave new non-linked cross-references behind.

## Coding conventions

### Source module size — measured in bytes, split at fracture points

Line counts are a poor proxy for module complexity, and an arbitrary line cap is a poor
design criterion. Judge a source file instead by its **size in bytes on disk**, on a
power-of-two scale. The scale applies to a **single source file**, not to a directory or a
crate. Crossing a threshold does not mandate a split; it raises the *pressure* to find a
fracture point, and that pressure compounds at every subsequent threshold.

**The scale is a smoke alarm, not a blueprint.** It decides *whether to go looking*; it never
decides *where to cut* or *how many parts to make* — that comes entirely from the fracture
points the file already has. A file may sit two thresholds up and still be right to leave
whole, and a file below every threshold may still be worth splitting when it obviously wants
to be two things. Never reach for the byte count to justify a cut.

| Size on disk | Name | Default posture |
|---|---|---|
| < 32 KiB | normal | No action. |
| >= 32 KiB | **large** | Look for a fracture point the next time you materially edit the file. Split if a clean one exists; leaving it whole is fine if none does. |
| >= 64 KiB | **extra large** (XL) | The default answer flips: **split**, unless you can name what makes the module indivisible. |
| >= 128 KiB | **XXL** | Split, or record a decision in the nearest DESIGN-NOTES.md naming the specific property that makes this module irreducible. |
| >= 256 KiB | **XXXL** | As XXL, and the file is a standing defect: queue the split as a CHECKLIST.md item rather than deferring it again. |

The scale continues by powers of two — each doubling adds one "extra" (32 KiB * 2^n = n
"extra"s) — and each step raises the bar for the justification that lets the file stay whole.

Rules:

- **Measure, do not estimate.** Use `tpu_stat_file` (`size`) or `tpu_count_file`; never infer
  size from a line count or from how large the file "feels".
- **Check at the moment you add.** Whenever you materially add to a source file, check whether
  the addition pushes it across a threshold. That is the moment to decide — not a later audit
  that nobody is scheduled to perform.
- **Split at fracture points, never at byte budgets.** A fracture point is a subset of the
  module with its own coherent responsibility and a narrow interface to the rest. If there is
  no fracture point, do not cut. **Never** split a module by relocating its trailing N bytes
  into a new file: a mechanical cut yields two modules that are each individually
  incomprehensible, which is strictly worse than one large coherent module.
- **A split is a refactor and lands on its own commit.** The module's public surface does not
  change, behavior does not change, and no other work rides along. Every split must also carry
  the provenance trail described below.
- **A split must not subvert layering.** See PLATFORM INTEGRITY above: if the only available
  fracture point would collapse or re-route a layer boundary, raise it with the engineer
  instead of taking it.
- **Some modules are legitimately irreducible** — a single exhaustive dispatch, a data table,
  a state machine whose states only make sense together. Say exactly that in the DESIGN-NOTES
  decision when such a module crosses XXL. **Generated code is exempt at every threshold.**
- **Test modules split more readily, not less.** They sit on the same scale, and their fracture
  points (by feature area, by scenario) are usually obvious — a large `tests.rs` should almost
  always become a `tests/` directory of focused siblings. For an **integration** test there is
  a layout trap: a test crate root resolves `mod x;` against the directory that *contains* it
  (`tests/`), not against a directory named after itself, so `tests/<name>/<part>.rs` is
  unreachable from `tests/<name>.rs` and parts placed in `tests/` would each be compiled as
  their own test target. `git mv tests/<name>.rs tests/<name>/main.rs` first — cargo
  auto-discovers that layout and the target keeps its name — then the parts resolve normally.
- **Prefer facade-preserving splits.** Promote `src/foo.rs` into `src/foo.rs` plus
  `src/foo/<part>.rs` (Rust 2018 style — no `mod.rs`), keeping `foo` as the facade that
  re-exports the parts, so every existing `use` path keeps working. A glob re-export
  (`pub use self::<part>::*;`) preserves both the paths and the public surface without naming
  each item, and it also puts the parts' public types back in scope for the facade's own code.
- **Watch the facade: if it is still the largest part, you split the periphery, not the module.**
  Shared items accumulate in the parent, so a split can report success against the byte scale
  while leaving the real mass exactly where it was. When the facade remains the biggest file in
  its own family, say so plainly rather than counting the split as done: that residue *is* the
  module. Either find the fracture inside it, or record what makes it irreducible.

#### Before you cut: check the privacy graph

Coherent responsibility is what makes a fracture point *worth* taking; **reachability** is what
makes it *possible*. In Rust that is decided by item visibility, and the shape is asymmetric:

- a child module **can** see its ancestors' private items;
- a parent **cannot** see its children's private items;
- siblings **cannot** see each other's private items.

So before cutting, list every **private** item defined in the candidate subset, and every
private item the subset references, then check the direction of each reference:

- defined in the subset, referenced only inside it — moves cleanly;
- defined in the parent, referenced by the subset — fine, no action;
- defined in the subset, referenced by the parent or by another part — **the cut is not viable
  as drawn**: that item belongs in the parent.

The reference that is easiest to miss, and the most expensive, is a private *type* named in a
parent-owned struct's field, because it drags its whole cluster back with it. In `bungo`'s
store, `Store` holds a private `Genesis`, whose impl reads `GenesisConfig`'s private fields,
which read `ShireId`'s — so all three types had to stay with `Store` even though the `init` /
`open` paths moved out cleanly. The same applies to private *methods*: a `fn` on the shared
type that two parts both call must live in the parent, not in whichever part happened to
define it first.

When a private item turns out to be shared, **move it to the parent — never widen it to
`pub(super)` to make the cut work.** Widening is a visibility change, which a pure relocation
forbids (see below); and "this item moved to the parent" is the honest signal that it is
shared, where a widened item silently stays in a module that no longer owns it.

Running this check with grep before cutting costs a minute. Discovering it from `E0624` /
`E0425` afterwards costs a full reset-and-retry of the split, and it has already cost three of
them on one file.

### Splitting a module — the mandatory provenance trail

Git has **no model of file identity**. It stores whole-tree snapshots and *infers* renames
and copies at query time by content similarity; nothing about a split is recorded in the
commit. A split is a **copy**, not a rename (the source file survives), so `git log --follow`
and GitLens's file history silently dead-end at the split commit and every line in the
extracted file appears to have been authored by whoever performed the split. To stop that,
three layers of trail are **mandatory** on every split, and all three land in the split
commit itself:

1. **Pure relocation** — so git's copy-detection heuristics can recover the lines.
2. **Commit trailers** — the only durable, machine-readable, non-heuristic record.
3. **A provenance header** in each extracted file — the human-visible pointer.

**1. Pure relocation.** The split commit moves bytes and does nothing else: no reflow, no
rename of any item, no visibility change, no behavior change, no unrelated work. Preserve
each moved block's **indentation level** so `cargo fmt` is a no-op on it (if `fmt` reflows
moved code, the cut was not clean — reconsider the fracture point). Keep `use`/import fixups
to the literal minimum needed to compile: **every line you touch is a line that loses its
provenance**.

**2. Commit trailers.** The split commit's message ends with a trailer block: one
`Split-Source:` line naming the file the content came from, and one `Split-Into:` line per
extracted file, all paths repository-relative. The block must be the **final paragraph**,
preceded by a blank line, containing **only** `Key: value` lines — a stray prose line inside
it makes git refuse to parse any of it.

**3. Provenance header.** Each extracted file carries, immediately after the copyright line,
a single comment naming its immediate source and the **pre-split commit** (that is `HEAD`
before you commit, so it is knowable while you author):

```rust
// Copyright (c) Mike Grier.
// Split from store.rs at 9ab3f21.
```

The facade file needs no header — its `mod` declarations are the forward pointer. When a file
that was itself split is split again, the header names only its immediate source; the chain
is walked one hop at a time.

#### Procedure — command-line git (PowerShell)

1. Start from a clean tree; the split is its own commit.
2. Capture the pre-split commit for the header:
   `git --no-pager rev-parse --short HEAD`
3. **Move the bytes by copy-then-delete, never by retyping.** `tpu_copy_file` the whole source
   file onto each new part, then issue **one** `tpu_edit_file` per part that deletes everything
   that part does not keep. All positions in a `tpu_edit_file` call reference the *original*
   file and are applied without interfering, so a single call carries every `delete` range plus
   the `insert` of the part's header. Then do the same to the source file itself, deleting the
   complement of what the facade keeps. This is what makes byte-identical relocation practical:
   re-emitting the text through the model reflows it, and blame then attributes the whole file
   to the split.
   - Include one adjacent blank line in each deleted range, or `rustfmt` collapses the doubled
     blank afterwards and adds noise to the diff.
   - When methods move out of an `impl` block, give each part its own `impl Type {` / `}`
     wrapper — two new lines — so the moved methods keep their original indentation and
     `cargo fmt` stays a no-op on them.
   - Derive the ranges from doc-comment starts, not from the `fn` / `struct` line, so each
     item's doc block travels with it.
4. Add the provenance header to each extracted file; wire the facade's `mod` / re-exports.
5. Run the Rust pre-commit gate (`cargo_fmt`, then `cargo_clippy --all-targets`) and the
   in-scope tests. `cargo fmt` should produce no diff inside the moved blocks.
6. Write the message to a scratch file (multi-line messages must never go through `-m`):

   ```
   Split store.rs: extract recovery and scrub

   Pure relocation, no behavior change. store.rs had reached 71 KiB (XL);
   recovery and scrub each carry their own responsibility behind a narrow
   interface.

   Split-Source: crates/bungo/src/store.rs
   Split-Into: crates/bungo/src/store/recover.rs
   Split-Into: crates/bungo/src/store/scrub.rs
   ```

   then `git add -A` the source, the new files, and the facade, and
   `git commit -F .scratch/split-commit.txt`.
7. **Verify the trailers parsed** — this must echo every extracted path; empty output means
   the block is malformed:
   `git --no-pager log -1 --format="%(trailers:key=Split-Into,valueonly)"`
8. **Verify blame traces through the split** — count the extracted file's lines still
   attributed to the split commit; expect only the provenance header and whatever you had to
   edit to compile:

   ```powershell
   $split = git --no-pager rev-parse --short HEAD
   git --no-pager blame -w -C1 -C1 -- crates/bungo/src/store/recover.rs |
     Select-String "^$split" | Measure-Object | Select-Object -ExpandProperty Count
   ```

   **Both parts of `-C1 -C1` matter, and they mean different things.** *Repeating* `-C`
   widens the search: a single `-C` only inspects files modified in the same commit, while
   **twice** makes blame inspect the other files in *the commit that created this file*,
   which is exactly the split case. The *numeric* argument is the detection threshold --
   the minimum run of alphanumeric characters a moved block must contain before blame will
   attribute it -- and it defaults to **40**, which is far too coarse for real source: code
   is full of short lines (`}`, `);`, `.expect("...")`) and whole extracted modules silently
   fail to trace at the default. Measured on the 16-way split of `crates/bilbo/src/tests.rs`:
   at the default threshold only 6 of 16 files traced at all, while `-C1 -C1` recovered
   **5,267 of 5,669 moved lines (93%)**.

   Expect a small residue attributed to the split: the provenance header, anything you had
   to edit to compile, and some short boilerplate lines that fall below matching confidence.
   If instead *most* of the file is attributed to the split, the relocation was not pure --
   fix it and `git commit --amend -F .scratch/split-commit.txt` (the commit has not been
   pushed yet).

#### Procedure — VS Code with GitLens

Same three layers; only the mechanics differ.

- **Authoring the message with trailers:** in the Source Control input box, `Enter` inserts a
  newline and `Ctrl+Enter` commits — so type the subject, a blank line, the body, a blank
  line, then the `Split-Source:` / `Split-Into:` lines. (The scratch-file + `git commit -F`
  route above is equally acceptable and is required if you are driving git from the terminal.)
- **Confirming the trailers:** open the GitLens **Commit Graph** or **Commit Details** view
  and check the trailer lines appear verbatim at the end of the message body.
- **Blame that traces through the split:** GitLens does **not** pass `-C` by default. Set
  `gitlens.advanced.blame.customArguments` to `["-w", "-C1", "-C1"]` in your user-local
  `.vscode/settings.json` (that path is git-ignored, so this is a per-clone setup step rather
  than a tracked repo file); with that in place, *GitLens: Toggle File Blame* on an extracted
  file attributes the moved lines to their original commits and authors rather than to the
  split. If you see the split commit on every line, either the setting is not active or the
  relocation was not pure. (See step 8 above for why the threshold must be
  `-C1` and not a bare `-C`.)
- **File history:** GitLens's file history follows renames only, so it will still stop at the
  split — that is expected and is why the trailer exists. Read `Split-Source` off the split
  commit and open the history of that path to continue.
- Note that **GitHub's web blame does not apply `-C`** and will always show the split commit
  as the origin. Local blame and GitLens (configured as above) are the tools of record.

#### One-time repository setup

```powershell
git config --local diff.renames copies
git config --local alias.blame-split "blame -w -C1 -C1"
```

The alias gives `git blame-split -- <file>` as shorthand for the verification in step 8.

### No manifest numeric constants in source code

Never write bare integer or byte literals as discriminants or protocol tags inline in logic code.
Instead, use **either** a named `#[repr(u8)]` enum **or** a `mod` of typed `const` values, and use
those named identifiers everywhere — in match arms, `vec![]` pushes, assertions, and doc tables.
Both approaches are acceptable; consistency within a single file or module is what matters.

**Bad:**
```rust
v.push(4u8);   // what is 4?
vec![255u8]    // magic
assert_eq!(key, vec![0u8]);
```

**Good (enum approach):**
```rust
#[repr(u8)]
enum ValueKeyTag { DbNull = 0, Text = 4, Err = 255, ... }

v.push(ValueKeyTag::Text as u8);
vec![ValueKeyTag::Err as u8]
assert_eq!(key, vec![ValueKeyTag::DbNull as u8]);
```

**Good (const approach):**
```rust
mod tags {
    pub const DBNULL: u8 = 0;
    pub const TEXT:   u8 = 4;
    pub const ERR:    u8 = 255;
}

v.push(tags::TEXT);
vec![tags::ERR]
assert_eq!(key, vec![tags::DBNULL]);
```

This rule applies to all binary encoding schemes, wire protocols, file format tags, sort-key type
bytes, and any other place where a numeric value carries identity meaning. The enum or const module
is defined in the same file or module as the logic that uses it, and its doc comment must note that
changing any value is a breaking change.

## Design Autonomy — Behavior is owned, never inherited from dependencies

We **define** our behavior. We **choose** dependencies that can satisfy our definition.

It is never acceptable to describe our behavior as "whatever crate X does" or "we delegate to
library Y." That framing surrenders our autonomy to decide what is correct for our users and makes
it impossible to reason about correctness, versioning risk, or future migration.

The correct framing is always:
1. State **what our specified behavior is** (inputs we accept, outputs we produce, errors we raise).
2. Note **which dependency is used to achieve it** and that the dependency was chosen because its
   behavior matches our specification.
3. If a dependency's actual behavior diverges from our specification, the dependency is wrong,
   not our specification. We either constrain the dependency, wrap it, or replace it.

We may align our specification with a dependency's behavior when that behavior is sensible for our
users — but the specification must still be written down explicitly and owned by us. When a
dependency is upgraded or replaced, our specification does not change; only the implementation does.

This applies everywhere: file formats, parse rules, error messages, wire protocols, encoding choices.

## Mono-repo bug policy — fix the layer, don't work around it

All crates in this repository are under active development. When work in one component
reveals a bug or deficiency in an underlying layer (another crate in the repo), **fix it
at the source** rather than working around it in the consuming crate. The whole point of
the mono-repo is that we own every layer and can change them together.

If the fix demands significant refactoring that would derail the current task, raise the
issue back to the engineer driving forward progress so we can decide together whether to
fix it now or defer it. But the default is always: fix the bug where it lives.

## Source-Components

- Source-Components are directory hierarchies in the repository rooted at some directory.
- Source-Components are identified by the presence of either a Cargo.toml file or a COMPONENT.md file in the directory.
- The root of the repository contains a Cargo.toml file, so the entire repository is a source-component, but there are also smaller source-components within the repository which may have their own Cargo.toml or COMPONENT.md files.

Examples:
- `src/tools/csv/` (has COMPONENT.md)
- `src/tools/csv/csv/` (has Cargo.toml)

## Always plan
- Always form a plan in the form of a CHECKLIST.md, at the lowest common source-component for the change
- Keep the plan up to date as you execute on the plan
- Keep a file at the root of each component, called PLANS.md, which tracks all the CHECKLIST.md files in the repository and their status (not started, in progress, completed). If it does not exist, create it. If it does exist, update it with the new CHECKLIST.md file and its status.
- When a CHECKLIST.md file is completed, move it to a table in a different file called COMPLETED-PLANS.md in the same directory, with a brief summary of the work completed, and remove it from PLANS.md.



PLANS.md format (markdown table):
| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|

COMPLETED-PLANS.md format (markdown table):
| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|

Status values: "not started", "in progress", "completed"

Design Notes column: Path(s) to DESIGN-NOTES.md file(s) that document the work, or "N/A" if none exist

## Plan sizing

If a plan exceeds roughly 10 work items or 3 levels of grouping/nesting, checkpoint it
into a CHECKLIST.md file in the repository before continuing. The goal is that the plan
survives a lost session — if the plan only exists in the chat, it will be lost.

## Design notes are not a work queue

Design notes (DESIGN-NOTES.md, DESIGN-RATIONALE.md, and related files) record *decisions*
— what was chosen and why. They steer future work, but they do **not** schedule it. The
only mechanism that queues work on existing code is a CHECKLIST.md item. A decision that is
recorded only in a design note, with the work it implies never transcribed into a checklist
item, is effectively orphaned: nothing will ever cause that work to be picked up.

This matters because the repository is worked by multiple people and multiple automated
agent sessions, often in parallel and on different machines. None of them share local or
session-private memory. The only directive any contributor — human or agent — can rely on
seeing is what is committed to the repository. Therefore work must be queued in committed
CHECKLIST.md files, never parked in an agent's memory, a chat thread, or a design note that
no one is obligated to act on.

When recording a decision that implies a change to existing code:

- In the same change that records the decision, ensure the implied work exists as a
  CHECKLIST.md item (creating or updating the checklist as needed), and reference the
  decision from that item so the two can be traced to each other.
- If a decision deliberately schedules **no** work — a reservation, a deferral, or a
  "leave as-is for now" choice — state that explicitly in the decision so the absence of a
  checklist item is visibly intentional rather than an oversight.

A component may layer additional, stricter conventions on top of this rule (for example, a
required cross-reference syntax between decision IDs and checklist items). Follow the
nearest applicable component instructions in addition to this baseline.

## CHECKLIST file hygiene

CHECKLIST files are **action-only**: they contain pending, in-progress, and recently
completed (`[x]`) items awaiting migration to `COMPLETED-CHECKLIST.md`. Completed items
must be moved to `COMPLETED-CHECKLIST.md` when a group is fully done (see below), with one
exception: a **large** completed item is moved *immediately* and replaced in place by a
one-line **stub-with-link** (see "Completed-item stubs (move-with-link)" below). Apart from
those single-sentence stubs, never leave historical records, prose summaries, rationale, or
context in a CHECKLIST file.

Checklists for work more than 2-3 items long should be organized into milestones.
Milestones should generally be sized to about 5 work items (suggestion, not a rule) and
should end with integration tests when possible.

At the end of every milestone, the following steps are **implicit** and must NOT be written
as checklist items:

1. **Build the default workspace (no warnings), both debug and release.**
   Fix **all** warnings that appear, even those unrelated to the milestone's
   changes. This is an ordinary (incremental) build — do **not** discard build
   artifacts first; we no longer force a clean rebuild to re-emit warnings. The
   exact commands depend on the language toolchain — see the language-specific
   instructions for the mapping (for Rust this is `cargo check --all-targets` and
   `cargo check --all-targets --release`,
   per [instructions/global.rust.instructions.md](instructions/global.rust.instructions.md)).

   **Scope = the default workspace, NOT every member.** "The default workspace" is the
   set of crates the build tool selects when given no explicit package/scope flag (for
   Cargo: the `default-members` list). Some members are deliberately **excluded** from
   the default set because they are expensive to build (for example, a crate with a
   large LALRPOP/codegen build script) — those exclusions are intentional and must be
   respected.

   **Do NOT broaden the scope to all members.** For Cargo specifically: run plain
   `cargo check --all-targets` (which honors `default-members`). **Never add
   `--workspace`** (nor enumerate every package with repeated `-p`) for a
   milestone-boundary build — doing so overrides `default-members`, drags in the
   intentionally-excluded slow crates, and has previously caused builds to appear to
   hang. `--all-targets` (tests/examples/benches) is fine and expected; `--workspace`
   (all members) is not.
2. **Test only the in-scope crate / source-component**, not the whole default workspace.
   **Include documentation tests**, not just unit and integration tests. For Rust,
   cargo-nextest does **not** run doc tests, so run them separately with `cargo_test`
   (`doc: true`, i.e. `cargo test --doc`) for the in-scope crate in addition to the
   nextest run; a milestone is not complete while any doc test fails or is unrun.
3. **Sync with origin**: `git fetch`, then merge or rebase the current branch on top
   of the updated upstream tip (`--no-edit`), resolving any conflicts, then push.
   Pushing is permitted at milestone boundaries without further confirmation; outside
   milestone boundaries, follow the standard "ask before pushing" rule.

These are standard procedure, not work items. Checklists contain only substantive work.

Work items in a milestone must be self-contained and all work items must be in dependency order.

### Blank line between multi-line items

**Separate multi-line checklist items with a blank line** so the division between one item and the
next is visible at a glance. A *multi-line item* is any `- [ ]` / `- [x]` whose body wraps onto
continuation lines or carries a nested sub-list — the "big work items" (a milestone's real
deliverables, an item with a `**Gap:**` / `**Target:**`-style body, a done-note, a decision
write-up). Two such items run together with no separator make it hard to see where one ends and the
next begins.

- **Required between** adjacent multi-line items (and between a milestone's last item and the next
  `## ` milestone heading).
- **Not required between** consecutive **single-line** items — a compact run of one-liners may stay
  dense (a blank line there is optional, never wrong).
- **Never within** an item — its own continuation lines stay contiguous; the blank line goes only
  *between* items.

```
- [ ] **ITEM-A** — a short one-liner.
- [ ] **ITEM-B** — another one-liner.

- [ ] **ITEM-C** — a big item whose body wraps onto continuation lines
  with **Gap:** / **Target:** prose.

- [ ] **ITEM-D** — the next big item, clearly divided from ITEM-C by the blank line above.
```

Repository-wide standard for all `CHECKLIST` / `COMPLETED-CHECKLIST` files. Apply it to items you
create or edit from now on; there is **no** obligation to reflow existing checklists.

### Checked means done — the move-or-spawn rule for deferred work

A checklist item is checked `[x]` **only when its own action is finished**. "Decided to defer,"
"does not apply yet," "no action needed now," or "kept open by a reserved field" is **not**
completion, and must never be recorded by checking the box while leaving the item sitting in a
milestone it does not belong to. Conflating *deferred* with *done* — and leaving post-deliverable
work parked in an early milestone — is the recurring defect this rule exists to stop. Every time
you are about to check, skip, or defer an item, first decide which case you are in:

- **There is still an action to take in *this* milestone** → leave the item **unchecked**. It is
  pending work and stays open until that work is done.
- **There is no action to take in this milestone** → the item does **not** belong here. Do one of:
  1. **Move** the item to the milestone in which it will actually be acted upon (use the `M{n}+` /
     `M∞` notation below when that milestone is not yet numbered), **or**
  2. If a real action *was* completed here that merely **spawns** later follow-up — e.g. a design
     decision recorded in a design milestone that implies later implementation, or an ABI-freeze
     obligation met by reserving a field whose *behavioral* decision is deferred — check the item
     `[x]` for the part that is genuinely done **and create a new work item for the follow-up in
     the correct (later) milestone in the same commit.** Never let the follow-up survive only as
     prose in a design note or a "Tracking" paragraph; that violates *design notes are not a work
     queue*.

The post-condition is mechanical and must hold after you touch any item: **no checklist contains a
checked item whose remaining work is unscheduled, and no milestone contains a no-action item that
belongs to a different milestone.** A deferred item is therefore *always* either moved, or replaced
by a checked-plus-spawned pair — never silently checked in place.

### Milestone naming for future and horizon work

Work is often gated on a deliverable that is not yet a concrete numbered milestone. Do not park it
as an unchecked item in the current milestone (that falsely implies the current milestone is
incomplete), and do not invent a fake number. Use one of two explicit placeholders, both visually
distinct from real milestones (`M1`, `M2.3`, …):

- **`M{n}+`** — read "after milestone *n*" — for work gated on a *specific* deliverable. Example:
  `M1+` = "to be done in some milestone after M1." The `+` names the gating milestone, sorts
  immediately after `M{n}`, and is an explicit **placeholder that graduates** to a real number once
  that later milestone is authored.
- **`M∞`** — read "the horizon" — a single terminal bucket per checklist for genuinely **ungated**
  "someday/maybe" work with no identified predecessor deliverable. In the ASCII-bound checklist
  files this token is written **`M-inf`** (see the 7-bit ASCII rule above).

Items in `M{n}+` / `M∞` are **parked, not pending**: they are not open obligations of any current
milestone, and the milestone that unblocks them **pulls them in (graduates them to a numbered ID)**
when it is authored. Rejected alternatives, recorded so they are not re-proposed: `M.Next`
(relative and ambiguous — "next" relative to which milestone? — and two files' `M.Next` mean
different things); `M{n}.5` (implies a *defined* intermediate milestone and collides with the
`RC-1.1` decimal sub-step notation); a bare `Backlog` / `TODO` (loses the gating deliverable and
does not sort with the milestones). `M{n}+` keeps the gate visible and graduates cleanly; `M∞` is
the catch-all when there is no gate.

### Completed-item stubs (move-with-link)

To keep the active checklist navigable without losing the in-place spine of "what we recently
did," a **large** completed item is relocated to `COMPLETED-CHECKLIST.md` the moment it is done
(rather than waiting for its whole group to finish) and a one-line **stub** is left in its exact
place, in dependency order. "Large" means the item's full text is **longer than 100 characters**
*or* it has sub-items. Short, single-line items are **not** stubbed — they stay checked in place
until their group migrates wholesale, exactly as before.

When a large item completes:

1. **Move** the item into `COMPLETED-CHECKLIST.md`, under the current `## Moved YYYY-MM-DD — …`
   date group, as a `###` heading that **opens with an explicit HTML anchor keyed to the
   work-item ID**, then repeats the stub's one-sentence summary, then an inline completion stamp:

   `### <a id="fl-d96"></a>FL-D9.6 — <same one-sentence summary as the stub>. *(completed YYYY-MM-DD HH:MM:SS UTC±hh:mm)*`

   The **HTML anchor — not the heading's auto-generated slug — is the link target**, so the
   descriptive heading text can be arbitrarily long without changing or breaking the anchor. VS
   Code resolves `#fl-d96` to the `<a id>`; the markdown `{#id}` attribute syntax does **not**
   work in VS Code's default engine, so always use the HTML anchor. The anchor id is the
   lowercased work-item ID with periods removed (`FL-D9.6` → `fl-d96`, `FL-C6.2` → `fl-c62`); get
   the timestamp fresh from `Get-Date -Format "yyyy-MM-dd HH:mm:ss zzz"`. The verbatim item body
   **may** be reproduced under the heading when it carries reasoning recorded nowhere else, but is
   **not required** when the authoritative text already lives in a DESIGN-NOTES decision or a
   design-session — in that case add a one-line pointer to it instead.
2. **Replace** the item in place with a single-line stub of the form:

   `- [x] **<ID>** -- <one-sentence summary>. -> [completed YYYY-MM-DD](COMPLETED-CHECKLIST.md#<id-slug>)`

   where `<id-slug>` is the lowercased ID with periods removed (matching the heading's `<a id>`
   anchor). The stub's summary and the heading's summary are written to **match**. Preserve the
   stub's original indentation if the item was nested. The summary is written **once** at move
   time and the archived heading is **immutable thereafter** (the archive is append-only history),
   so neither the summary nor the link target can drift.

**Never** use a line-number fragment (`#L123`) as the link target — list items shift, headings
do not; anchor only to the heading's `<a id>`. When a milestone finally completes, its remaining
stubs migrate (are deleted) with the group as usual: a stub carries no information not already in
the archive, so nothing is lost.

### Sub-step notation

When a checklist step is broken into sub-steps, always use decimal notation: `RC-1.1`, `RC-1.2`,
`RC-1.3`, etc. (or whatever prefix is in use). Never use lettered sub-items (`RC-1a`, `RC-1b`) or
nested bullet lists to represent sub-steps. This applies both to CHECKLIST files and to any inline
step breakdowns described during planning.

When a group of related items is fully complete:
1. Move the completed group to `COMPLETED-CHECKLIST.md` in the same directory.
2. Prefix the moved block with a heading: `## Moved YYYY-MM-DD — <brief description of what was done>`.
3. `COMPLETED-CHECKLIST.md` is **append-only**; always add new groups at the bottom.
4. Leave only the remaining pending or in-progress items in the source `CHECKLIST.md`.

Named feature files (`CHECKLIST-<feature>.md`) should be **deleted entirely** once all items are
complete. Move their content to `COMPLETED-CHECKLIST.md` in the same directory before deleting.

## Design note files

Any directory in the repository may have a DESIGN-NOTES.md file.

The DESIGN-NOTES.md file should record design decisions about the code in that directory and its children.

If a decision should be recorded, it should be recorded in a DESIGN-NOTES.md file. The DESIGN-NOTES.md
file to use is either the DESIGN-NOTES.md file in the source-component directory which should be created
if it does not already exist, or if there is an already existing DESIGN-NOTES.md file in any ancestor
directory between the file being changed and the source-component root, use that one instead.

### What to include

The design note files should include anything that a future developer should or may want to know about the
code to help them "get up to speed" or diagnose interesting or bad behaviors.

### What not to include

Like with code comments, don't include super obvious things.

Example: A query processor design note must describe its intent and unique approach in a paragraph, not provide a comprehensive tutorial on the underlying technology or theory. It may include links to external resources for further reading, but should not attempt to teach the reader about query processing in general.

### Supersedence / deferral status must be adjacent to the decision title

When a design decision is **superseded**, **deferred**, **withdrawn**, **dissolved**, or
otherwise no longer the plain current answer, a **one-line status marker in bold MUST appear
immediately adjacent to that decision's title** — on the line directly under the `##`/`###`
heading (or `|`-row entry in a decision-index table), *before* any other prose. This holds
**even when** the fuller explanation of the supersedence/deferral already appears later in the
decision's body: the reader must learn the decision's status from the top, not by reading to
the end. Missing this marker is the recurring defect this rule exists to stop (a reader takes a
superseded decision as current because the "superseded by …" sentence was buried at the bottom).

Form (keep it to a single line; link the target so it is Ctrl+Click-navigable per the
clickable-cross-reference rule):

- **Superseded:** `**Superseded by [<nice text>](<relative-link>).**`
- **Deferred:** `**Deferred to [<nice text>](<relative-link>).**` (or `— deferred, gated on
  <blocker>` when there is no single target yet).
- **Withdrawn / dissolved:** `**Withdrawn — <one-line reason>.**` /
  `**Dissolved by [<nice text>](<relative-link>).**`

The marker is a *pointer*, not the whole story: the detailed reasoning may still live in a
paragraph lower in the body (or in Tier 2 / a design session). Keep the top-line marker and the
body consistent — if the target changes, update the adjacent marker in the same edit.

Apply this to decisions you author or touch from now on; there is no obligation to retrofit every
existing decision, but fix the marker on any superseded/deferred decision you edit.

### Three-tier design documentation

Source-components with substantial design history should separate current decisions from
historical rationale using three tiers:

- **Tier 1: `DESIGN-NOTES.md`** — Current canonical decisions. Contains decision indexes,
  compact detail sections stating what was decided and why. Every paragraph must answer
  "what is the decision?" or "what constraint forced this choice?" — not "what else did
  we consider?" Content that answers the latter belongs in Tier 2.

- **Tier 2: `DESIGN-RATIONALE.md`** — Historical record of how decisions were reached.
  Alternatives considered, prior art, design session summaries, evolutionary reasoning.
  Cross-referenced by decision ID from Tier 1. This file is consulted for "why" questions,
  not for forward implementation work.

- **Tier 3: `design-sessions/DESIGN-SESSION-<date>-<topic>.md`** — Raw design session
  transcripts, dated by session. Reference material, not routinely loaded. Stored in a
  `design-sessions/` subdirectory under the source-component root.

When recording a new decision, write to both Tier 1 and Tier 2 in the same commit.
**Never treat Tier 2 or Tier 3 as authoritative for current decisions.** If there is a
conflict, Tier 1 wins.

A source-component may have a `DESIGN-INSTRUCTIONS.md` file specifying additional design
rules — including how these tiers are used — for that component and everything below it.
When working in a directory, locate and follow the nearest `DESIGN-INSTRUCTIONS.md` in
that directory or any ancestor up to the source-component root. These directives are
binding for all work under that directory.

Not all source-components need all three tiers. Small components may have only DESIGN-NOTES.md.

### Design session files

When a design conversation produces extended discussion, exploration, or working-through of a
topic — beyond what fits in a Tier 2 rationale section — capture it as a design session file.

**When to create a session file:**
- The conversation explores a topic in depth over multiple exchanges
- The discussion covers alternatives, trade-offs, or implications that would be valuable
  context for a future reader trying to understand the design landscape
- The topic warrants a standalone record beyond the decision summary in DESIGN-RATIONALE.md

**Naming:** `DESIGN-SESSION-<YYYY-MM-DD>-<topic-slug>.md` (e.g.,
`DESIGN-SESSION-2026-04-06-task-floating.md`)

**Location:** `design-sessions/` subdirectory under the source-component root. Create the
directory if it does not exist. This prevents session files from accumulating in the
component's top-level directory.

**Content:** The session file should be a faithful record of the design discussion — the
reasoning, alternatives, and conclusions as they unfolded. It does not need to be polished
prose, but should be readable by a future developer. Include a brief summary at the top
noting which decisions (D-numbers) resulted from the session.

### Historical Record

As features age out of a source-component, at the very least, move notes which are no longer relevant to a
different file, DESIGN-NOTES-AGED-OUT.md.

When moving the section to DESIGN-NOTES-AGED-OUT.md, include the date of the move, in YYYY/MM/DD format.

## Standing orders on git merge / rebase — ON-GIT-MERGE-OR-REBASE.md

A directory may carry an `ON-GIT-MERGE-OR-REBASE.md` file of **standing orders**: actions
that must be carried out whenever other contributors' commits are integrated into your
working tree for the component it governs. It is located by the **same nearest-ancestor
rule as `DESIGN-NOTES.md`** — the copy in the source-component root, or the nearest one in
any ancestor directory between the file being changed and that root. The filename is
deliberately long and unabbreviated so that a contributor following *either* the git
discipline *or* the design-notes discipline will not miss it.

**When the trigger fires.** The trigger is the *arrival of incoming commits in your working
tree* — via `git merge`, `git rebase`, `git pull`, or a `git fetch` followed by either —
**including conflict-free integrations**. It is *not* the act of running a git command, and
*not* the presence of conflicts: a clean, no-conflict integration can still oblige
follow-up. On every such integration, **before you push or resume your task**, consult the
nearest `ON-GIT-MERGE-OR-REBASE.md` for each component the incoming commits touched
(including the one you are working in) and carry out its orders — **even when those incoming
changes lie entirely outside the scope of what your session set out to do**.

**Why this exists.** Integration pulls in work your task would never otherwise touch, yet
that work can obligate reconciliation of the component's *derived or generated* artifacts.
Example: an incoming commit reshapes test metadata and adds a new test corpus file — that
file's **golden must be regenerated** even though your session changed nothing about that
test. Standing orders record exactly these component-specific post-integration duties so
that every session, human or agent, honors them regardless of its own goal. When you
discover such a recurring duty for a component, add it to that component's
`ON-GIT-MERGE-OR-REBASE.md` (creating the file per the location rule if absent) in the same
change that performs it.

## Quality

When providing testing, always provide extensive testing to test at least 10 normal cases, as well as all identifiable edge cases, unless the
computation required to test the edge cases would be excessive on a modern system. The unit tests for a submodule should be able to complete
in under one second of elapsed time on an AMD Ryzen R7 processor running at 1.5ghz with 16gb of memory.

If there are tests which seem vital that would take longer, put an item in a CHECKLIST.md file with special importance for the user to
decide on whether to include them or not.

In any case, if the test is vital it must be authored and be run as part of the integration tests rather than the unit tests.

### Milestone vs sub-milestone checklist work

When working on checklist items organized into milestones, build and test only the
source-component in scope, not the entire repository.

To complete the milestone, perform the implicit end-of-milestone steps described under
"CHECKLIST file hygiene" above (repo-wide build with zero warnings, in-scope tests,
sync with origin and push).

### Unit tests

Unit tests should always be reproducible and not use random sampling techniques at runtime without the developer's explicit approval and then
it should be recorded in a design note.

### Integration tests

Integration tests should use larger scale data.

There is no required minimum, but in general should start with data volume in the hundreds or thousands.

The data does not have to be necessarily stable. A guideline might be that smaller data sets (<10kb) should be checked in whether in
a separate file or somehow encoded in source files. Larger data sets may be generated at run time, whether exhaustively or
via random techniques.

## Architectural pre-steps

**Never call `stdout`/`stderr`/`print`/`eprintln` (or the language equivalent) from
more than one site in a tool.** At the first occurrence, introduce an output
abstraction — a writer trait, a sink, or a formatter — and route every subsequent
output through it. The abstraction need not be elaborate (a single trait with one
`write_str` method, or a UTF-8 character stream, is enough); the requirement is that
the storage target (file, channel, stdout, stderr) and the formatting concern be
separable from the call sites that produce content.

This applies to any feature whose output may plausibly need to be retargeted later:
CLI output, log output, diagnostic output, generated artifacts.

<!-- tpu-mcp:setup:begin -->
## File I/O — use `tpu_*` MCP tools, never PowerShell or shell

This workspace runs the **tpu-mcp** MCP server which exposes encoding-aware
file primitives as first-class tools. Plain `Get-Content` / `Set-Content` /
`Out-File` / `>` / `cat` / `sed` round-trip files through the active code
page and silently corrupt UTF-8, UTF-16, smart quotes, em-dashes, and
box-drawing characters. Use the MCP tools instead — they detect, preserve,
and round-trip the file's native encoding and line endings safely.

**Rule:** when working in any project that has the tpu-mcp server registered,
ALWAYS prefer the `tpu_*` tools over PowerShell or shell file commands.

| MCP tool | Use it for |
|---|---|
| `tpu_read_file` | reading text files (UTF-8, UTF-16, Windows-1252, Shift-JIS, …) |
| `tpu_read_head` / `tpu_read_tail` | first/last N lines or bytes |
| `tpu_read_file_binary` | inspecting raw bytes of binary files |
| `tpu_read_file_escaped` | reading text as a single 7-bit-clean escaped line |
| `tpu_write_file` | replacing a text file's full contents |
| `tpu_append_file` | appending text to an existing file |
| `tpu_replace_in_file` | literal (default) or regex substitution — pass `regex: true` to opt into regex matching |
| `tpu_edit_file` | targeted insert/delete/splice at known line numbers |
| `tpu_validate_file` | pre-flight assertion that a file is in the expected state |
| `tpu_count_file` | line / word / char / byte / pattern counts |
| `tpu_find` | encoding-aware grep across files and globs (pass `glob` to filter a directory walk, e.g. `path: "DIR", glob: "**/*.ndjson"`) |
| `tpu_copy_file` | copy a file or recursively copy a tree (resilient: per-entry warnings, never aborts mid-walk by default) |
| `tpu_render_file` | populate a file from a `{{TOKEN}}` template |
| `tpu_stat_file` | verify a write actually persisted (mtime / size) |
| `tpu_doctor` | scan files/dirs/globs for mojibake or encoding damage; optionally repair with `fix: "peel"` |
| `tpu_setup` | (re)write this guidance block into the active `copilot-instructions.md` |

### When to use each

- **Reads** — always use `tpu_read_file`. Never use PowerShell `Get-Content`
  for code review or content inspection.
- **Edits** — prefer `tpu_replace_in_file` (literal matching by default,
  no escaping needed) over `tpu_edit_file` when the target text is unique,
  because line numbers can shift between reads. Use `tpu_edit_file` when
  you have just read the file and know exact line offsets.
- **Writes that should be guarded** — pass `validate: [{ "selector":
  "line-contains:N", "value": "..." }]` to refuse the write if the file is
  not in the expected state.
- **Globs / recursion** — `tpu_find` and `tpu_copy_file` accept glob
  patterns and tolerate inaccessible directories by emitting warning
  records (configurable via the `on_error` argument). To search a directory
  tree with `tpu_find`, pass the directory as `path` and the filename
  pattern as `glob` (e.g. `path: "q:/src/foo/.scratch", glob: "**/*.ndjson"`)
  — this is the `find DIR -name PAT` shape and is the only way to recurse
  into an absolute directory.
- **Dependency-free templating** — `tpu_render_file` substitutes
  `{{NAME}}`-style tokens. Use `\{{` to emit literal braces.

### Tool output format

Every tool response uses a **mixed format**: a JSON invocation header,
a content-type-dependent body, and (for most tools) a JSON status trailer.
Not every line is JSON — read tools and `find` return raw content
between the header and trailer.

- **Line 1** — invocation header:
  `{"reason":"x-tpu-mcp-invocation","tool":"tpu_NAME","args":{...}}`
  Large `content`/`replacement`/`template` fields appear as `"<N bytes>"` placeholders.
- **Mutating tools** (write, replace, edit, append) — normal write:
  `{"status":"success","file":"...","mtime_epoch_ms":N,"size":N}`
  Preview modes do not stamp the file and return a reduced trailer:
  `diff:true` adds unified diff lines before the status (full stamp still present for write/replace/edit).
  `dry_run:true` (replace only): optional diff lines, then `{"status":"success","changed":true|false}`.
  `count:true` (replace only): `{"status":"success","count":N}`.
  `append diff:true`: diff lines when changed, then `{"status":"success","file":"...","changed":true|false}`.
- **Structured tools** (count_file, stat_file, copy_file, render_file,
  setup+target, doctor) — result line
  `{"reason":"x-tpu-mcp-result",...}` followed by `{"status":"success"}`.
- **Read tools** (read_file, read_head, read_tail, read_file_escaped) — header then raw content; no JSON trailer on success.
  **Exception** — `tpu_read_file_binary` with a non-empty `hash` arg acts like a structured tool:
  `{"reason":"x-tpu-mcp-result","encoding":"bytes-base64","content":"<base64>","hashes":[...]}` followed by `{"status":"success"}`.
  Without `hash`, `tpu_read_file_binary` returns header + 7-bit-clean escaped bytes (no trailer).
- **Find tool** (find) — header, then matching lines as plain text, then `{"status":"success","warnings":[...]}` trailer.
- **Errors** — `{"status":"error","message":"..."}` as the final line;
  `isError: true` in the MCP wrapper.

### When a file looks corrupted (mojibake)

Symptoms: `Ã©` where `é` should be, `â€"` where `—` should be, `â"€` instead
of `─`, stray `Â ` before numbers, `ð\u009f...` blobs instead of emoji.
This is *mojibake* — text that was decoded in the wrong encoding and then
re-encoded as UTF-8. It is almost always caused by a non-tpu writer round-
tripping the file through the OS code page (PowerShell `Get-Content` /
`Set-Content` / `Out-File` / `>` / `Add-Content`, a misconfigured editor,
a generator that assumed ASCII).

Workflow:

1. **Diagnose**: call `tpu_doctor` with the suspect file (or the
   surrounding directory / glob). It returns a JSON report listing every
   flagged file, its detected encoding, per-pattern match counts, exact
   line/column locations, and whether a one-layer "peel" repair would
   strictly improve the file (`peel_suggested: true`).
2. **Identify the offender**: when a file is corrupted in a git repo, run
   `git log -p -- <file>` (or `git blame -- <file>`) to find the
   introducing commit. The commit reveals which tool wrote the damage so
   you can stop the leak at the source rather than only repairing
   downstream.
3. **Repair (conservative)**: call `tpu_doctor` again with
   `fix: "peel"`. Only files whose peel produces *strictly fewer* mojibake
   matches are rewritten; the prior content is preserved at `<file>.bak`.
   Re-run `tpu_doctor` after the repair to confirm the report is clean.
4. **Don't paper over it**: if a file legitimately contains mojibake
   digraphs (test fixtures, regex sources, documentation about mojibake),
   add the line `encoding-check: allow-mojibake` (typically inside a
   comment) — `tpu_doctor` and the write-time guard will treat it as
   clean.

The write-time guard in `tpu_write_file` / `tpu_append_file` /
`tpu_replace_in_file` / `tpu_edit_file` already refuses to *introduce* new
mojibake (pre-existing damage passes through). If you genuinely intend to
write curated mojibake fixtures, pass `allow_mojibake: true`.

### When line endings disagree with git (CRLF / LF)

A separate, git-aware condition: a file's on-disk line endings can differ
from what git would materialise in the working tree for that path (per
`.gitattributes` `text`/`eol` attributes and `core.autocrlf` / `core.eol`).
This is *not* mojibake — the bytes are valid — but it produces noisy diffs
and "whole file changed" churn.

Detection is **opt-in per call** via a `git_root` argument (an absolute path
to the repository root; there is no upward auto-discovery):

1. **Detect on read**: pass `git_root` to `tpu_read_file`, `tpu_read_head`,
   or `tpu_read_tail`. When the file's endings differ from git's expectation
   the response is prefixed with a single `note:` line and the unchanged
   content follows.
2. **Report / repair with doctor**: call `tpu_doctor` with `git_root` to
   list mismatched files (each flagged with an `eol_mismatch` object). Pass
   `fix: "eol"` to normalise line endings only, or `fix: "all"` to also peel
   mojibake. `eol`/`all` require `git_root`; the rewrite is atomic with a
   `<file>.bak` backup and UTF-16 files are skipped.
3. **Normalise on write (off by default)**: when the server is started with
   line-ending normalisation enabled (the `tpu-mcp.normalizeLineEndings` VS Code
   setting, the `--eol-normalize` flag, or the `TPU_EOL_NORMALIZE` env var),
   mutating tools given a `git_root` denormalise to git's expected
   convention unless an explicit `line_ending` is supplied. This is **off by
   default** so writes never silently rewrite endings without opt-in.

### File encoding

When you must fall back to PowerShell, never round-trip non-ASCII files
through `Get-Content` / `Set-Content` — read and write via
`[System.IO.File]::ReadAllBytes` / `WriteAllBytes` and validate with
`tools/check-encoding.ps1` afterwards.
<!-- tpu-mcp:setup:end -->
