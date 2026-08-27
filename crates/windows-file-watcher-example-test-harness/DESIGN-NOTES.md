# Design notes: windows-file-watcher-example-test-harness (Tier 1)

This crate is a **published, deliberately-legible EXAMPLE**. It shows one coherent way to build a
unit-test harness for file-change-notification handlers on top of
[windows-file-watcher](../windows-file-watcher/DESIGN-NOTES.md)'s `test-util` seam. Its purpose is
**exposition of technique**, not to be a supported framework. Third parties are expected to read it,
cut-and-paste from it, and adapt it to their own test platform.

Current, canonical decisions for the crate. This is the authoritative record; the "why" and the
alternatives considered live in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) (Tier 2). On any conflict,
this file wins.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **This is an example, not a framework, and the crate name says so.** Composing with an arbitrary third party's other test infrastructure is an unsolvable problem, so we ship a legible exemplar meant to be cut-and-paste-adapted, not depended on. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-1). |
| <a id="d-2"></a>D-2 | **Built only on file-watcher's *public* `test-util` surface -- never any `pub(crate)` item.** A forcing function: if the exemplar cannot be built from the public seam, that is a seam gap in `windows-file-watcher`, not something to paper over here. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-2). |
| <a id="d-3"></a>D-3 | **The handler is a trait (the one plug point); capture and replay are handler-linked.** Both run the consumer's handler *in-process* against a **built-in example handler**, which doubles as a worked example of using the library. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-3). |
| <a id="d-4"></a>D-4 | **The schedule wire format (`NotificationSpec` / its JSON) is harness-owned and explicitly NOT semver-covered.** A tool I/O format -- a captured/replayed schedule -- not a data contract; its shape may change in any release. Precedent: file-watcher D-71 and topology D-8. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-4). |
| <a id="d-5"></a>D-5 | **The generator emits only contract-legal schedules.** The D-83 fidelity principle lifted from values to *schedules*. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-5). |
| <a id="d-6"></a>D-6 | **Publication is gated on a published file-watcher that includes `test-util`.** A release-ordering constraint, recorded so it is not discovered at publish time. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-6). |
| <a id="d-7"></a>D-7 | **The wire format is deliberately unvalidated; legality is a documented policy, not a type invariant.** Staying inside file-watcher's contract is the generator's (D-5) and the hand-author's responsibility. See [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#d-7). |

## What it demonstrates

Three integration modes, so a reader can pick whichever fits their platform:

1. **In-process unit test** -- script or generate a schedule, drive your handler, assert. The simplest mode.
2. **Capture** (`capture` bin) -- run the seeded generator under chaos against a handler, detect a pathology
   (the handler panicked or failed its own invariant), and dump the offending schedule to JSON. `capture`
   itself runs the plain oracle (`run`), which cannot detect a handler that stops consuming without
   panicking; a wedge is only caught by the deadline oracle (`run_with_deadline`, demonstrated separately,
   [tests/oracles_catch_pathologies.rs](tests/oracles_catch_pathologies.rs)), not by this bin.
3. **Replay** (`replay` bin) -- load a captured JSON schedule and re-drive it against the handler to
   reproduce the pathology deterministically, as a regression test.

The fidelity limit is the same one the seam carries (file-watcher D-83): the harness tests the *handler's
reactions*; it does not prove the crate would ever emit a given sequence, and a bug that depends on the
handler's own internal nondeterminism (below the delivery seam) replays as a lead, not a guaranteed repro.
