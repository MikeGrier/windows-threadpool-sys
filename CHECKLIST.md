# Checklist: workspace

Workspace-level and cross-cutting work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

The most recent completed group is
[M1 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m1),
which recorded the delivery-contract specification taxonomy and applied it to the three crates that publish
such a contract. Its follow-on audits are tracked per crate, not here:
[windows-file-watcher M14](crates/windows-file-watcher/CHECKLIST.md),
[windows-overlapped-io-sys M14](crates/windows-overlapped-io-sys/CHECKLIST.md), and
[windows-ioring-sys M10](crates/windows-ioring-sys/CHECKLIST.md).

## M2 -- Stop contract corrections from failing to propagate

M1's taxonomy addresses **under-specification**: what a contract fails to say. PR #42's later review rounds
exposed a second, distinct failure mode it has no mechanism for -- **restatement drift**: one fact is stated
in many independent places, a correction reaches some of them, and the rest keep teaching the old answer.
Five of six findings across three consecutive rounds were of this kind, and every one was a correction of
mine that had not propagated. Measured on the two crates involved: `QueueFull` semantics are restated across
13 files, "`Stopped` is terminal" across 8.

The remedy is ordered by how little it depends on anyone remembering: first make the fact impossible to
restate, then make the restatements compile, and only then fall back on convention for what neither covers.

- [x] **M2.1** -- Make the prose compile. `windows-file-watcher`'s
  [TESTING.md](crates/windows-file-watcher/TESTING.md) carries four Rust blocks and
  [README.md](crates/windows-file-watcher/README.md) one, and **none is compiled today** -- there is no
  `include_str!` anywhere, so they can only rot, and the `Stopped` drift proved they do. Wire both in as
  doctests so a contract change that invalidates an example breaks the build instead of misleading a
  consumer. Expect to massage the blocks: they are written as `#[test] fn`, and a doctest wraps in `main`.
  Done: doctest count 2 -> 7, verified by reintroducing the `Stopped` drift and confirming the failure.
  CI's `cargo test --workspace --all-features` covers `test-util`, so the guard is live there.

- [x] **M2.2** -- `DesyncCause::is_terminal()`, and adopt it everywhere. This is the value-level fact that
  drifted across 8 files. Handlers branching on `is_terminal()` rather than matching `Stopped` by name also
  means a sixth cause added later cannot silently break every consumer. Adopt at all four example sites (the
  crate-level rustdoc example, TESTING.md, `examples/test_your_handler.rs`, and the harness's
  `PresenceTracker`), so the pattern being taught is the durable one. Done: all four adopted, and the
  predicate carries its own doctest enumerating all five causes.

- [ ] **M2.3** -- `DesyncCause::is_reachable_in(WatchMode)`, and make the generator bind to it. The
  Coarse/`QueueFull` fact had four independent encodings -- audit table, D-17 bullet, workspace taxonomy row,
  and the generator plus its test -- and drifted in both directions across two rounds (emitting a cause a
  coarse watch cannot, then excluding one it can). A generator re-deriving tier legality from its own reading
  of prose is the exact violation of PLATFORM INTEGRITY rule 2 (*depend on specified primitives, never on
  incidental behavior*); it must call the predicate instead, with its test asserting against the same one.

- [ ] **M2.4** -- Record the decision in the workspace [DESIGN-NOTES.md](DESIGN-NOTES.md): restatement drift
  as a failure mode distinct from the ten specification-gap categories, why the taxonomy cannot catch it
  (the taxonomy asks what the contract fails to *say*; this is about copies of what it does say), the
  measured evidence from PR #42, and the three-tier remedy. Cross-reference it from M1's taxonomy section so
  a reader arriving at the categories learns that stating a rule correctly is necessary and not sufficient.

- [ ] **M2.5** -- Turn the two conventions into binding rules in
  [.github/copilot-instructions.md](.github/copilot-instructions.md), which is the channel both humans and
  Copilot actually read: (a) an analysis document never restates normative content -- it edits the authority
  and cites it (the D-30 miss is the proof: the audit said "now stated" while the decision still said the
  opposite); and (b) a mandatory blast-radius sweep before committing any contract correction -- grep the
  distinctive term across `src/`, `tests/`, `examples/` and `*.md`, fix every hit or justify each, and record
  the sweep in the commit. The sweep is already proven: run once voluntarily, it immediately found a site no
  reviewer had reported.
