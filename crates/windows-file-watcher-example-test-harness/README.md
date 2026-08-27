# windows-file-watcher-example-test-harness

A **published example** -- read it, cut-and-paste from it, adapt it. It shows one coherent way to
unit-test file-change-notification handlers built on
[`windows-file-watcher`](https://crates.io/crates/windows-file-watcher)'s `test-util` seam, with no
filesystem and no thread pool.

It is **not** a supported framework. A test framework that must compose with your other test
infrastructure is an unsolvable problem, so we don't ship one; we ship a legible exemplar you own the
moment you copy it. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for the reasoning.

## What it shows

- A `Handler` trait -- the one thing you plug your own handler into.
- A harness-owned, serde schedule format (`NotificationSpec` / `Schedule`) built from file-watcher's
  constructible boundary types, with the data and control-flow dependencies a *legal* schedule must
  respect documented on the `schedule` module.
- `drive(&Schedule, &mut impl Handler)` -- feed a real `Receiver`, dispatch each notification to your
  handler.
- A contract-legal seeded generator (`Generator`) -- chaos that stays inside file-watcher's documented
  contract, reproducible by seed.
- Oracles (`run`, `run_with_deadline`) that catch a handler panic, a failed invariant, or a wedge.
- JSON record/replay (`Recording`), and `capture` / `replay` bin tools, for turning a found pathology
  into a deterministic regression.

## Three ways to use it

Each is a runnable example (`cargo run --example <name>`) and a way you might integrate the technique:

| Example | Integration mode | What it shows |
|---|---|---|
| [`examples/in_process_test.rs`](examples/in_process_test.rs) | In-process unit test | Script a `Schedule` by hand, drive your handler, assert. The simplest mode. |
| [`examples/capture_demo.rs`](examples/capture_demo.rs) | Capture | Run the generator across seeds, keep whatever trips your handler's oracle. |
| [`examples/replay_demo.rs`](examples/replay_demo.rs) | Replay | Reproduce a captured pathology deterministically from its JSON. |

The `capture` and `replay` binaries (`cargo run --bin capture` / `--bin replay`) are the on-disk,
CLI-argument versions of the same two ideas -- `capture` writes `Recording`s as JSON files, `replay`
loads one back. Both are **handler-linked**: they drive the crate's own intentionally-buggy
[`example_handler::BuggyHandler`], and are meant to be rewritten against your own handler, not depended
on directly.

## Wiring your own handler

1. Implement [`Handler`](src/handler.rs) for your type: `fn on(&mut self, notification: &Notification)`.
   Add `fn check(&self) -> Result<(), String>` if your handler has a cross-notification invariant worth
   asserting (e.g. "every name I've seen added is eventually removed").
2. Start with [`examples/in_process_test.rs`](examples/in_process_test.rs)'s shape: script a handful of
   `NotificationSpec`s that model the traffic you care about, `drive` your handler, assert.
3. Once that works, let `Generator` find cases you didn't think to script: `run(&generator.generate(seed),
   &mut YourHandler::new())` in a loop over seeds, checking `outcome.pathology()`.
4. When a seed trips something, `Recording::new(seed, schedule, outcome).save(path)` preserves it. Load
   it back with `Recording::load(path)` and re-`run` it any time to confirm the fix (or, before the fix,
   to hand a teammate an exact reproduction).

Copy [`src/bin/capture.rs`](src/bin/capture.rs) / [`src/bin/replay.rs`](src/bin/replay.rs) as your
starting point for a CLI version of the same loop against your own handler.

## Fidelity limit

It tests your handler's *reactions*, not whether file-watcher would ever emit a given sequence. The
generator stays inside the legal envelope, so a pathology it finds is real; a bug that depends on your
handler's own internal nondeterminism replays as a lead, not a guaranteed repro.

## Windows only

Like `windows-file-watcher`, this is Windows-only; it resolves to an empty crate elsewhere.

## License

MIT. Copyright (c) Mike Grier.
