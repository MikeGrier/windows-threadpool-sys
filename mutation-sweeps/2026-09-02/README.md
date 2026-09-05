# Mutation sweep -- 2026-09-02

A cargo-mutants run over every workspace member, kept so its findings can be
worked through later without paying for the sweep again. It took roughly
fourteen hours.

## What was run

Each package in turn, through the workspace wrapper, which is where all the
settings that matter live (WER dialog suppression, `-j 2`, features on the
correct side of `--`, a per-run output directory):

```powershell
.\tools\run-mutants.ps1 -Package <name> -OutputDirectory <dir>
```

Only the `missed.txt` and `timeout.txt` lists are kept here. The full run
produced 219 MB across 10,624 files, almost all of it per-mutant build logs
that say nothing once the outcome is known.

## Totals

| Package | Caught | Survived | Timeout |
|---|---|---|---|
| wtf-string | 97 | 0 | 0 |
| windows-thread-ambient-sys | 142 | 0 | 0 |
| windows-impersonation-token-sys | 33 | 1 | 0 |
| windows-ioring-sys | 229 | 2 | 6 |
| windows-waitable-queues | 400 | 4 | 120 |
| windows-topology-sys | 127 | 5 | 0 |
| windows-guard-alloc | 88 | 22 | 0 |
| windows-overlapped-io-sys | 135 | 43 | 23 |
| windows-threadpool-sys | 102 | 44 | 18 |
| windows-namespace-request-sys | 120 | 49 | 1 |
| windows-file-enumeration-sys | 410 | 50 | 8 |
| windows-file-watcher-example-test-harness | 77 | 69 | 3 |
| windows-file-watcher | 415 | 113 | 10 |
| windows-placement-probe | 315 | 199 | 4 |
| windows-platform-probes | 102 | 511 | 5 |
| **total** | **2792** | **1112** | **198** |

## Three ways this table misleads, and what to do instead

**A timeout is usually a detection that lost its name, not a gap.** `cargo test`
runs tests as threads in one process, so a single test parked on a queue that
will never fill stops the whole harness reporting -- and the run is recorded as
a timeout even when other tests have already failed. This is why
`windows-waitable-queues` appears to sit at 76% while actually having four
survivors in 524.

Measured rather than assumed: re-injecting `validate_capacity -> Ok(())`, one of
that crate's 120 timeouts, fails four tests in **0.00 seconds**. It was caught
instantly and then buried.

So do not read a timeout as a gap. To find out what one really is, re-inject
that single mutant and run the one test that should catch it, or use
`cargo_test`'s `bisect` to name the thread that parked.

**A low score on an executable probe is measuring the wrong thing.**
`windows-platform-probes` is fourteen binaries you *run* to answer a question
about Windows, not a library with a test surface; it is `publish = false` at
version `0.0.0`. Its 511 survivors are mostly `main`-adjacent code that no test
was ever going to reach. The same caveat applies, less strongly, to
`windows-file-watcher-example-test-harness`, which is published to be read and
copied rather than depended on.

Excluding those two, the substantive backlog is roughly 530 survivors.

**Not every survivor is a missing test.** Three other kinds turn up often enough
to check for before writing anything:

- *Equivalent mutants*, which change no observable behaviour. Argue the
  equivalence, then measure it: declare the sabotage `survives` in a manifest
  for `tools/run-sabotage.ps1` and also inject the non-equivalent direction, so
  the claim is a property of the code rather than of weak tests. Worked examples
  already in the tree: `poison::mul_inverse`'s idempotent extra Newton round,
  `from_filetime`'s disjoint word halves, `ProcessorSet::empty`, whose mutant
  replaces the body with the code already there.
- *Unreachable code*, where no test could reach the line. Check that a test
  *could* before writing one. `RegisteredBuffers::is_empty` cannot return true
  because the kernel refuses an empty registration; what was worth pinning there
  was the platform behaviour, not the accessor.
- *Constants*, where a `const` assertion beats a test -- it fails the build
  rather than a run somebody chose to make. Verify it in both directions: the
  mutation must fail to compile with the assertion and compile cleanly without.

## Reading a per-package file

One file per package, listing survivors grouped by source file, with the
timeouts kept separate. Eight of them note commits on
`mikegrier/deferred-namespace-ops` that already closed part of the list; those
files are **not** pruned, because pruning them by hand would be a second source
of truth. Re-run the affected package before treating any single line as
outstanding.

The most useful way to read one is by *shape* rather than line by line. A large
block of survivors usually names one absent kind of test, and one new test can
close all of it. The two that dominated this sweep:

- **Accessors that are never read back.** Every test builds a value with `with_*`
  and then performs it, and the perform path reads the fields directly -- so a
  constant accessor is indistinguishable from a truthful one. Fixed by
  configuring non-zero, pairwise-distinct values and reading each back, with the
  distinctness itself asserted so a later edit cannot silently weaken it.
- **Boundaries tested with comfortably-wrong values.** A 400-character path
  proves a check exists but not that it sits at the right unit, so moving the
  limit by one survives. Fixed by asserting the exact unit at which the answer
  changes.
