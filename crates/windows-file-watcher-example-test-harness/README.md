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
  constructible boundary types.
- `drive(&Schedule, &mut impl Handler)` -- feed a real `Receiver`, dispatch each notification to your
  handler.
- A contract-legal seeded generator (chaos that stays inside file-watcher's documented contract).
- JSON record/replay, and `capture` / `replay` bin tools, for turning a found pathology into a
  deterministic regression.

## Fidelity limit

It tests your handler's *reactions*, not whether file-watcher would ever emit a given sequence. The
generator stays inside the legal envelope, so a pathology it finds is real; a bug that depends on your
handler's own internal nondeterminism replays as a lead, not a guaranteed repro.

## Windows only

Like `windows-file-watcher`, this is Windows-only; it resolves to an empty crate elsewhere.

## License

MIT. Copyright (c) Mike Grier.
