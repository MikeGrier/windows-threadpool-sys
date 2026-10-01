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

- [x] **M-T6.8** -- Decided: the fail-fast is a default-off `fail-fast` Cargo feature that arms it
  directly. -> [completed 2026-10-01](COMPLETED-CHECKLIST.md#m-t68)

- [ ] **M-T6.11** -- **Implement the teardown fail-fast.**

  The mechanism is settled by [The teardown fail-fast is a default-off Cargo feature that arms it
  directly](../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature): a `fail-fast` feature, off
  by default, which when enabled arms the fail-fast with no second runtime switch. The accepted
  cost -- feature unification means any crate in the graph enabling it changes teardown behaviour
  for every crate in the graph -- is recorded there, including why the inverse polarity was
  rejected outright and why gating availability instead was declined.

  **What to build.** At `Drop`, when the obligation flag says a drain is owed, fail fast instead of
  reporting. `M-T4.3` already put that flag on every type and deliberately kept it outside the
  `trace` feature, so the fact is there without new bookkeeping; what changes is what `Drop` does
  with it.

  **Not to be made piecemeal, and if made, made uniformly.** That is a constraint on the work, not
  a note about it: implementing it for `ThreadpoolWait` alone -- the type the M26.13 measurement
  happens to implicate -- would leave the crate with one linear type and the rest affine, which is
  a worse surface than either choice made consistently.

  **It is not greenfield.** `ThreadpoolIo` already ships a soft version: its `Drop` reports a
  skipped rundown and then continues. A hard fail-fast changes that type's existing behaviour too.

  **`M-T6.7` gave the crate a uniform `stop_and_drain`, but not across all six types.**
  `ThreadpoolIo::run_down` and `CleanupGroup::close_members` keep their own names for reasons
  recorded with that decision, so this item has to say what a fail-fast means for those two rather
  than assume the uniform method covers them.

  **STILL OPEN, to settle here or raise:** what it does when the object is dropped on an
  already-unwinding path, where a panic aborts. The mechanism decision deliberately did not answer
  it.

  **One bound is already fixed and constrains every answer: forward progress is not the
  alternative.** A teardown that cannot drain may abort, or fail fast by some other route, but it
  may not return to its caller having abandoned the callback. Bounding the wait is a question
  about which failure to take, never about whether to continue.
