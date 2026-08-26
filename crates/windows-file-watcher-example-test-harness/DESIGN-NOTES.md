# Design notes: windows-file-watcher-example-test-harness

This crate is a **published, deliberately-legible EXAMPLE**. It shows one coherent way to build a
unit-test harness for file-change-notification handlers on top of
[windows-file-watcher](../windows-file-watcher/DESIGN-NOTES.md)'s `test-util` seam. Its purpose is
**exposition of technique**, not to be a supported framework. Third parties are expected to read it,
cut-and-paste from it, and adapt it to their own test platform.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **This is an example, not a framework, and the crate name says so.** A framework that must *compose* with an arbitrary third party's other test infrastructure is an unsolvable problem (see the file-watcher testability discussion). So we do not ship one; we ship a legible exemplar that composes with nothing by design and is meant to be adapted. `windows-file-watcher-example-test-harness` is wordy on purpose -- the name is the disclaimer, and most consumers will cut-and-paste rather than depend. Legibility beats completeness everywhere the two conflict. |
| <a id="d-2"></a>D-2 | **Built only on file-watcher's *public* `test-util` surface -- never any `pub(crate)` item.** This is a forcing function: if the exemplar cannot be built from the public seam, that is a seam gap to fix in `windows-file-watcher`, not to paper over here. So the crate doubles as proof that the M13 seam is sufficient for a real harness. |
| <a id="d-3"></a>D-3 | **The handler is a trait (the one plug point); capture and replay are handler-linked.** Both capture (find a schedule that breaks the handler) and replay (reproduce it) must run the consumer's handler *in-process*, and Rust cannot load an unknown third-party handler into a prebuilt binary. So this crate's `capture`/`replay` bins run against a **built-in example handler**, and are themselves worked examples of how a third party writes their own bins against their own handler using the library. |
| <a id="d-4"></a>D-4 | **The schedule wire format (`NotificationSpec` / its JSON) is harness-owned and explicitly NOT semver-covered.** `windows-file-watcher` does not serialize `Notification`, so the harness defines its own serde-able description of a notification and converts it to a real `Notification` (via the `test-util` builders) at drive time. That JSON is a tool I/O format -- a captured/replayed schedule -- not a data contract; its shape may change in any release. Precedent: file-watcher D-71 and topology D-8. |
| <a id="d-5"></a>D-5 | **The generator emits only contract-legal schedules.** This is the D-83 fidelity principle lifted from values to *schedules*: perturbations (ordering, timing, loss) stay inside what file-watcher's documented contract permits (D-12 in-stream ordering, D-29 loss/backpressure via `Desync`), so a pathology the harness finds is one a real substrate could actually produce -- not a phantom manufactured by an impossible schedule. |
| <a id="d-6"></a>D-6 | **Publication is gated on a published file-watcher that includes `test-util`.** The crate builds in-workspace today via the path dependency, but it cannot be published to crates.io until `windows-file-watcher` is published with the `test-util` feature available. This is a release-ordering constraint, recorded so it is not discovered at publish time. |
| <a id="d-7"></a>D-7 | **The wire format is deliberately unvalidated; legality is a documented policy, not a type invariant.** The format can express schedules file-watcher would never produce (a `Resumed` with no prior `Suspended`, two concurrent questions for one watch, a `Batch` after a `Cancelled`). This is intentional on two counts: the same format must faithfully carry a *recorded* schedule (whatever actually happened, so it cannot be pre-constrained to only-legal), and the legality rules are stateful *sequencing* constraints a per-value type cannot express anyway. Staying inside file-watcher's contract -- the data and control-flow dependencies documented on the `schedule` module -- is therefore the generator's (D-5) and the hand-author's responsibility. It is mechanism-not-policy applied to the format: the format supplies the vocabulary, the caller supplies legal sentences. |

## What it demonstrates

Three integration modes, so a reader can pick whichever fits their platform:

1. **In-process unit test** -- script or generate a schedule, drive your handler, assert. The simplest mode.
2. **Capture** (`capture` bin) -- run the seeded generator under chaos against a handler, detect a pathology
   (the handler panicked, stopped consuming, or failed its own invariant), and dump the offending schedule
   to JSON.
3. **Replay** (`replay` bin) -- load a captured JSON schedule and re-drive it against the handler to
   reproduce the pathology deterministically, as a regression test.

The fidelity limit is the same one the seam carries (file-watcher D-83): the harness tests the *handler's
reactions*; it does not prove the crate would ever emit a given sequence, and a bug that depends on the
handler's own internal nondeterminism (below the delivery seam) replays as a lead, not a guaranteed repro.
