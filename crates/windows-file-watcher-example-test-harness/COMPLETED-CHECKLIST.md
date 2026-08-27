# Completed checklist: windows-file-watcher-example-test-harness

Append-only archive of completed milestones, moved out of [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-26 -- M1..M6: the full crate (spine through examples and exposition)

A published EXAMPLE test harness for file-change-notification handlers, built on
[windows-file-watcher](../windows-file-watcher/README.md)'s `test-util` seam. Decisions D-1...D-7 are in
[DESIGN-NOTES.md](DESIGN-NOTES.md). Legibility beats completeness (D-1); the crate is built only on the
public `test-util` surface (D-2), which doubles as proof the M13 seam is sufficient for a real harness.

### M1 -- Spine: handler trait, schedule wire format, driver

- [x] **M1.1** -- Scaffold the crate: [Cargo.toml](Cargo.toml) (depends on `windows-file-watcher` with `test-util`,
  plus `serde`), add it to the workspace members, a README framing it as an example (D-1), and a
  [src/lib.rs](src/lib.rs) gating everything on `cfg(windows)`.

- [x] **M1.2** -- `Handler` trait: `on(&mut self, &Notification)` plus a default `check(&self) ->
  Result<(), String>` invariant hook. The one plug point (D-3).

- [x] **M1.3** -- The harness-owned schedule wire format (D-4): `NotificationSpec` (serde) mirroring every
  `Notification` variant and its boundary types, with `to_notification()` converting via the `test-util`
  builders; and `Schedule` (an ordered `Vec<NotificationSpec>`). Documented with the data and control-flow
  dependencies a legal schedule must respect (D-7: the format is deliberately unvalidated; legality is a
  documented policy, not a type invariant).

- [x] **M1.4** -- `drive(&Schedule, &mut impl Handler)`: feed a real `channel_with_bound` `Receiver` with
  the scheduled notifications and dispatch each drained one to the handler -- no filesystem, no thread pool.

- [x] **M1.5** -- An `example_handler::PresenceTracker` (a small realistic handler) and an integration test
  driving it through a scripted schedule, asserting its reactions.

### M2 -- Contract-legal seeded generator

- [x] **M2.1** -- A splitmix64 `Rng` and a `Generator` producing legal `Schedule`s (D-5): sequences within
  file-watcher's contract (D-12 ordering, D-29 loss as `Desync`), reproducible by seed.

- [x] **M2.2** -- Tunable shape (length, per-kind weights, watch count) with sane defaults; documented the
  legal-envelope constraints in prose so a reader can extend the generator safely.

- [x] **M2.3** -- Unit test: a fixed seed yields a byte-identical schedule run to run.

### M3 -- Oracles

- [x] **M3.1** -- An `Outcome` (Healthy | Pathology { kind, at_step, .. }) and `run(&Schedule, &mut impl
  Handler) -> Outcome` that catches a handler panic (`catch_unwind` around dispatch -- legitimate here, the
  handler is consumer code, not an FFI callback), a failed `Handler::check`, and (via `run_with_deadline`)
  a stalled/wedged handler.

- [x] **M3.2** -- Unit test: a deliberately-buggy example handler trips each oracle.

### M4 -- Record / replay (JSON)

- [x] **M4.1** -- Added `serde_json`; a `Recording { seed, schedule, outcome }` with save/load helpers. The
  JSON schema is explicitly not semver-covered (D-4). Replay always re-runs the captured schedule, never
  re-generates from the seed (the seed is kept only as provenance).

- [x] **M4.2** -- Unit test: generate -> find a pathology -> serialize -> deserialize -> replay -> the same
  pathology, deterministically.

### M5 -- capture / replay bins

- [x] **M5.1** -- [src/bin/capture.rs](src/bin/capture.rs): runs the generator under many seeds against the built-in example
  handler, preserving schedules that trip an oracle to JSON files. `[[bin]]` entry. Verified end to end: on
  the default generator config, every seed in `[0, 20)` tripped `BuggyHandler`'s oracle.

- [x] **M5.2** -- [src/bin/replay.rs](src/bin/replay.rs): loads a captured JSON schedule and replays it against the example
  handler, reporting reproduction. `[[bin]]` entry. Verified end to end: replaying `capture-0.json`
  reproduced the exact recorded outcome.

- [x] **M5.3** -- Documented that these bins are handler-linked exemplars (D-3): a third party writes their
  own against their own handler using the library. Added `example_handler::BuggyHandler`, an
  intentionally-buggy handler (panics on a second loss `Desync`) shipped so the bins have something real
  to find, with an explicit reviewer-facing warning (both a doc-comment `<div class="warning">` and an
  inline "INTENTIONAL BUG -- DO NOT FIX" comment) so human or automated code review does not propose
  "fixing" it.

### M6 -- Examples and exposition

- [x] **M6.1** -- [examples/*.rs](examples/) demonstrating the three integration modes (in-process unit test;
  capture; replay) against the example handler. All three run end to end.

- [x] **M6.2** -- Crate README + rustdoc teaching the technique, the fidelity limit (D-5), and the
  adapt-don't-depend framing (D-1); a "wiring your own handler" walkthrough.

- [x] **M6.3** -- Integration test tying the full arc together (generate -> run -> record -> replay),
  [tests/full_arc.rs](tests/full_arc.rs).
