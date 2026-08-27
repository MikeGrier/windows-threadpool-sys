# Design rationale: windows-file-watcher-example-test-harness (Tier 2)

*Why* the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were reached -- the alternatives weighed
and the reasoning. Keyed by decision ID. This file is consulted for "why" questions; it is not
authoritative for current decisions (Tier 1 is).

## D-1: an example, not a framework

A framework that must *compose* with an arbitrary third party's other test infrastructure is an
unsolvable problem (see the file-watcher testability discussion). So we do not ship one; we ship a
legible exemplar that composes with nothing by design and is meant to be adapted.
`windows-file-watcher-example-test-harness` is wordy on purpose -- the name is the disclaimer, and
most consumers will cut-and-paste rather than depend. Legibility beats completeness everywhere the
two conflict.

## D-2: public `test-util` surface only

This is a forcing function: if the exemplar cannot be built from the public seam, that is a seam
gap to fix in `windows-file-watcher`, not to paper over here. So the crate doubles as proof that
the M13 seam is sufficient for a real harness.

## D-3: the handler is a trait; capture/replay are handler-linked

Both capture (find a schedule that breaks the handler) and replay (reproduce it) must run the
consumer's handler *in-process*, and Rust cannot load an unknown third-party handler into a
prebuilt binary. So this crate's `capture`/`replay` bins run against a **built-in example
handler**, and are themselves worked examples of how a third party writes their own bins against
their own handler using the library.

## D-4: the wire format is harness-owned, not semver-covered

`windows-file-watcher` does not serialize `Notification`, so the harness defines its own
serde-able description of a notification and converts it to a real `Notification` (via the
`test-util` builders) at drive time. That JSON is a tool I/O format -- a captured/replayed
schedule -- not a data contract; its shape may change in any release. Precedent: file-watcher D-71
and topology D-8.

## D-5: the generator emits only contract-legal schedules

This is the D-83 fidelity principle lifted from values to *schedules*: perturbations (ordering,
timing, loss) stay inside what file-watcher's documented contract permits (D-12 in-stream
ordering, D-29 loss/backpressure via `Desync`), so a pathology the harness finds is one a real
substrate could actually produce -- not a phantom manufactured by an impossible schedule.

## D-6: publication is gated on a published file-watcher with `test-util`

The crate builds in-workspace today via the path dependency, but it cannot be published to
crates.io until `windows-file-watcher` is published with the `test-util` feature available. This
is a release-ordering constraint, recorded so it is not discovered at publish time.

## D-7: the wire format is deliberately unvalidated

The format can express schedules file-watcher would never produce (a `Resumed` with no prior
`Suspended`, two concurrent questions for one watch, a `Batch` after a `Cancelled`). This is
intentional on two counts: the same format must faithfully carry a *recorded* schedule (whatever
actually happened, so it cannot be pre-constrained to only-legal), and the legality rules are
stateful *sequencing* constraints a per-value type cannot express anyway. Staying inside
file-watcher's contract -- the data and control-flow dependencies documented on the `schedule`
module -- is therefore the generator's (D-5) and the hand-author's responsibility. It is
mechanism-not-policy applied to the format: the format supplies the vocabulary, the caller
supplies legal sentences.
