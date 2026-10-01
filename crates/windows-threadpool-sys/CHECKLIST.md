# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M-T6 -- Cancellation self-heals

Implements [Cancellation repairs the pool it may have wedged](../../DESIGN-NOTES.md#cancellation-self-heals),
decided 2026-10-01. Consumer-facing account in
[README-FEATURE-self-heal.md](README-FEATURE-self-heal.md); how the design was
reached, including the branches abandoned, in
[DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md#how-cancellation-self-heal-was-reached).

Supersedes **M-T5.8**, which asked whether `cancel_pending` should be removed.
The answer is no: it is renamed, made best-effort in name as it always was in
behaviour, and backed by a repair.

- [x] **M-T6.1** -- Add the `self-heal` feature, default on, and the pool registry. ->
  [completed 2026-10-01](COMPLETED-CHECKLIST.md#m-t61)

- [x] **M-T6.2** -- Stamp the last dispatch in every trampoline. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t62)

- [x] **M-T6.3** -- Rename to `try_cancel_pending`, and add the ungated `unsafe` sibling. ->
  [completed 2026-10-01](COMPLETED-CHECKLIST.md#m-t63)

- [x] **M-T6.9** -- Repair the one sabotage case still declared wrong. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t69)

- [x] **M-T6.10** -- The two wait-drain sabotages stopped detecting; cause found and both guards
  restored. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m-t610)

- [x] **M-T6.4** -- The self-heal timer. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t64)

- [x] **M-T6.5** -- Guard it, with the sabotage that matters. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t65)

- [x] **M-T6.6** -- Verify the `self-heal`-off build. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t66)

- [x] **M-T6.7** -- Decided: `stop_and_drain` is the name, added to the four types that lacked it,
  with `run_down` and `close_members` deliberately left alone. -> [completed
  2026-10-01](COMPLETED-CHECKLIST.md#m-t67)

- [ ] **M-T6.8** -- **DECISION TO RAISE, reserved by the engineer 2026-09-28 as CRATE-WIDE: a
  fail-fast that forces the caller to have closed, making these types linear rather than affine.**

  > **Gated on `M-T6.7`**, because there is no uniform method to be linear *about* until the
  > synchronous close is uniform. Raised in `M-T4` as `M-T4.4` and moved here 2026-10-01 with the
  > item it depends on.

  > **`M-T5.8` set a relevant precedent 2026-10-01**, without settling this. Faced with an
  > operation that could not be made safe, the crate did not reach for linearity -- it paired a
  > safe form with an `unsafe` sibling carrying a statable obligation, and let a feature decide
  > which exists. That is a different answer to "how do we make the caller take responsibility"
  > than a linear type is, and it is now shipping. Weigh it as an alternative here rather than
  > treating linearity as the only way to bind a caller to a protocol.

  **Not to be made piecemeal, and if made, made uniformly.** That is a constraint on the work, not
  a note about it: implementing it for `ThreadpoolWait` alone -- the type the M26.13 measurement
  happens to implicate -- would leave the crate with one linear type and four affine ones, which
  is a worse surface than either choice made consistently.

  **It is not greenfield.** `ThreadpoolIo` already ships a soft version: its `Drop` reports a
  skipped rundown and then continues. A hard fail-fast changes that type's existing behaviour too,
  so the decision is "does the crate become linear", not "do we add something new".

  `M-T4.3` has since given every type a flag recording whether a drain is owed, deliberately not
  behind the `trace` feature, so a fail-fast has the fact it needs without new bookkeeping.

  Still open within it: whether the fail-fast is off by default (assumed), how it is selected --
  environment variable, constructor option, process-wide setter -- and what it does when the
  object is dropped on an already-unwinding path, where a panic aborts.

  **One bound is already fixed and constrains every answer: forward progress is not the
  alternative.** A teardown that cannot drain may abort, or fail fast by some other route, but it
  may not return to its caller having abandoned the callback. Bounding the wait is a question
  about which failure to take, never about whether to continue.
