# Mutation survivors from the 2026-09-02 sweep

Work queued from the workspace-wide cargo-mutants run recorded in
[mutation-sweeps/2026-09-02/README.md](mutation-sweeps/2026-09-02/README.md).
That README carries the command, the totals, and -- more importantly -- the
three ways its numbers mislead. **Read it before starting any item here.**

The sweep cost roughly fourteen hours, so its findings are kept rather than
re-derived. Each item below points at the per-package file listing that
package's survivors grouped by source file.

**Read a package file by shape, not line by line.** A large block of survivors
usually names one absent *kind* of test, and one new test closes all of it. The
two shapes that dominated this sweep were accessors nothing ever reads back, and
boundaries tested with comfortably-wrong values. Both are described in the
README with the fix that worked.

**Three kinds of survivor are not missing tests**, and checking costs less than
writing a test that asserts a property the code does not have: equivalent
mutants, unreachable code, and constants that want a `const` assertion rather
than a test. The README describes how to tell each apart, and how to *measure*
an equivalence claim instead of merely arguing it.

## M1: the shipping crates

- [ ] **MS-1.1** -- **`windows-overlapped-io-sys`: 43 survivors, 23 timeouts.**
  See [windows-overlapped-io-sys.md](mutation-sweeps/2026-09-02/windows-overlapped-io-sys.md).
  Concentrated in `fs.rs` (10), `config.rs` (6), and `device.rs` (4). The 23
  timeouts are the second-largest block in the workspace after the queues, and
  this crate has the same blocking-API shape -- so expect most of them to be
  detections that lost their name rather than gaps, and confirm before writing
  anything.

- [ ] **MS-1.2** -- **`windows-threadpool-sys`: 44 survivors, 18 timeouts.**
  See [windows-threadpool-sys.md](mutation-sweeps/2026-09-02/windows-threadpool-sys.md).
  The workspace's namesake crate and the lowest-level one that ships.

- [ ] **MS-1.3** -- **`windows-file-watcher`: 113 survivors.**
  See [windows-file-watcher.md](mutation-sweeps/2026-09-02/windows-file-watcher.md).
  The largest block in a shipping crate, but **71 of the 113 are in
  `scenario.rs`**, which is the crate's own `scenario-tool` test tooling rather
  than the watcher. Decide whether that tooling is in scope before starting:
  the remaining 42 are spread across `watcher.rs` (21), `directory.rs` (8), and
  the rest.

- [ ] **MS-1.4** -- **Finish `windows-file-enumeration-sys`: about 20 of 50 remain.**
  See [windows-file-enumeration-sys.md](mutation-sweeps/2026-09-02/windows-file-enumeration-sys.md).
  The `error.rs` and `path.rs` blocks are closed, along with the completion
  ring's reservation accounting. What remains is
  spread thinly: `native.rs` (5), `session.rs` (4), `submission_ring.rs` (3),
  `pattern.rs`, `admission.rs`, `engine.rs`, `registry.rs`.

- [ ] **MS-1.5** -- **Finish `windows-namespace-request-sys`: about 21 of 49 remain.**
  See [windows-namespace-request-sys.md](mutation-sweeps/2026-09-02/windows-namespace-request-sys.md).
  The accessor and error-surface blocks are closed. What
  remains is mostly `final_path.rs` (8) and `full_path.rs` (2), plus
  `handle.rs`'s four `delete -` mutants and `buffer.rs`/`watch.rs` drop impls.

- [ ] **MS-1.6** -- **Finish `windows-guard-alloc`: about 16 of 22 remain.**
  See [windows-guard-alloc.md](mutation-sweeps/2026-09-02/windows-guard-alloc.md).
  Seed parsing and `poison::identify`'s bound are closed, and two loop bounds
  are recorded as equivalent. What remains is in `lib.rs`
  (`seed`, `announce_seed`, `poison_check`, `data_offset`) and `witness.rs`.

- [ ] **MS-1.7** -- **Finish `windows-placement-probe`: about 190 of 199 remain.**
  See [windows-placement-probe.md](mutation-sweeps/2026-09-02/windows-placement-probe.md).
  The plan's arithmetic and the median's rates are closed. The bulk
  is still `peer_index_cache.rs` (70) and `core_affinity.rs` (54), both of which
  are pure selection and measurement logic that the module documentation already
  says is testable offline -- so most of it should be reachable without hardware.

## M2: the crates that are not libraries

Both of these score badly for reasons that are not defects, so **decide whether
they are in scope at all before spending time on them.** That decision belongs
to the engineer, not to whoever picks up this checklist.

- [ ] **MS-2.1** -- **`windows-platform-probes`: 511 survivors, 17% caught.**
  See [windows-platform-probes.md](mutation-sweeps/2026-09-02/windows-platform-probes.md).
  Executable probes you *run* to answer a question about Windows, not a library
  with a test surface; `publish = false` at version `0.0.0`. Most of the
  survivors are `main`-adjacent code no test was ever going to reach. Judging
  this crate by mutation score is measuring the wrong thing.
  **The sweep measured a fourteen-probe tree, of which eight are in this
  repository** -- `core_affinity`, `peer_index_cache`, `doorbell_cost`,
  `queue_contention`, `request_cost` and `topology` were only ever on the
  originating branch, so six of the fourteen sections in the sweep report have no
  counterpart here yet. Read the totals with that in mind before scoping any of
  it.
  A related, separately-tracked item, **partly discharged**: at sweep time twelve
  of those fourteen probes printed directly rather than through a `Report` sink,
  tracked as `SH-13.4` on the branch that ships the topology and queue crates.
  The sink has since landed here
  ([crates/windows-platform-probes/src/report.rs](crates/windows-platform-probes/src/report.rs))
  and every probe in this repository routes through it -- stated without a count,
  because the count moves as stages land -- so what remains of
  `SH-13.4` is the six branch-only probes, and it stays named rather than linked
  because that branch checklist is still not in this repository. The crate's own
  durable work now has a home to link:
  [crates/windows-platform-probes/CHECKLIST.md](crates/windows-platform-probes/CHECKLIST.md).
  For the eight that landed, the "would make some of this reachable" prerequisite
  is met -- each `main` is one line and each report is a `String` a test can read.

- [ ] **MS-2.2** -- **`windows-file-watcher-example-test-harness`: 69 survivors.**
  See [windows-file-watcher-example-test-harness.md](mutation-sweeps/2026-09-02/windows-file-watcher-example-test-harness.md).
  A deliberately legible *example* harness, published to be read and copied
  rather than depended on. Its value is in being clear, and tests written purely
  to raise its score would work against that.

## M3: keeping the record honest

- [ ] **MS-3.1** -- **Re-run the affected packages and prune what is closed.**
  Eight package files note commits that already closed part of their list, and
  those files are deliberately **not** pruned by hand -- editing them to match
  would create a second source of truth that drifts from the tool's own output.
  A re-run is the only honest way to shrink them. Do this once the M1 items are
  substantially done, and replace this directory with a dated sibling rather
  than editing it in place, so the two runs can be compared.
