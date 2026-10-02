# The drain still holds with the self-heal and the explicit teardown in place -- 2026-10-01

Closes **M26.15**. Figures in [arms.csv](arms.csv).

## What this answers, and what it does not add

[2026-09-29-the-fix-on-the-real-path](../2026-09-29-the-fix-on-the-real-path/README.md)
already measured the committed drain on the real `EventDelivery` path: 33
failures in 10000 reverted runs against 0 in 70000 drained. **That is not
re-established here, and this measurement is not evidence the fix works** --
that question was answered, and M26.15's own framing that "nothing has re-run
this crate's stall against the shipped drain" was wrong when it was written.

What this run answers is narrower. That measurement was taken against the
`M-T4` build. Three things have changed the path since, all of them on the
teardown this stall is about:

- **`M-T6` added the self-heal**, default on, so every process now runs a
  repair timer that submits a work item to a pool owing a repair. `M26.13.3`
  established that a work submit is the one thing that ends the stall, so a
  mechanism that submits work on a timer could in principle *mask* the failure
  rather than leave it absent.
- **`M-T6.3` renamed the cancelling form** and backed it with that repair.
- **`EventDelivery` gained its own `Drop`** on 2026-10-01, draining the wait
  explicitly before any field is dropped, rather than reaching
  `ThreadpoolWait::drop` through field order.

So the composition under test is new even though each part is argued to be
safe.

## Arms

The same reproducer as every earlier run: the three-test subset of
[event_delivery.rs](../../../windows-ioring-sys/tests/event_delivery.rs) -- two victims plus the
`dropping_with_nothing_outstanding_does_not_hang` trigger -- one fresh process
per run, scored by exit code, serial, with `WINDOWS_THREADPOOL_TRACE='*'` so
the rates compare with the earlier figures.

- **control** -- `M-T4.2` undone (`ThreadpoolWait::drop` cancels) *and*
  `EventDelivery`'s explicit drain removed, so teardown takes the pre-2026-09-28
  path.
- **current** -- the committed tree, default features.

## Result

The control reproduces at **12 in 4000**, which sits inside the range every
earlier capture of this arm has reported (8, 13, 15, 16, 18). The current build
reports **0 in 12000**.

Under the null hypothesis that the rate is unchanged from the control's, 12000
runs would be expected to produce about 36 failures, and the probability of
observing none is about 2e-16.

**The control is the load-bearing half of this.** A zero on its own cannot
distinguish a fix from a reproducer that has stopped reproducing -- the machine,
the toolchain and the surrounding tests have all moved since September. The
control was run first, from the same session and the same source tree, for that
reason.

## What it does not say

It does not distinguish "eliminated" from "reduced below what 12000 runs can
see"; it excludes an unchanged rate.

It does not say the self-heal is uninvolved. The repair timer only submits to a
pool that owes a repair, and with the drain in place a teardown records none --
but this run did not instrument whether any repair fired, so "the self-heal did
not mask anything" is an argument from the mechanism, not a measurement. An arm
built with `--no-default-features` would settle it.

It is not a root cause. Why closing a wait handle behind its own disarm stalls
the pool is still open; see [STALL-TIMELINE.md](../../STALL-TIMELINE.md).

## Method note

The control's patches were made with a tool that fails on a missing anchor, and
both were read back out of the source before building -- the procedure
[2026-09-29-the-fix-on-the-real-path](../2026-09-29-the-fix-on-the-real-path/README.md)
adopted after a PowerShell `.Replace()` silently matched nothing and voided a
60000-run measurement. The patched build also emitted a `field is never read`
warning for `EventDelivery::wait`, which is a second, independent signal that
the explicit drain really had been removed.
