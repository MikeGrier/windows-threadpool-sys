# It never self-releases: the stall is permanent, and the probe was repairing it -- 2026-09-27

Prompted by a question in review that this record could not answer: if the
post-mortem probe's work submit is what ends the stall, *why does anything wait
five seconds before submitting it?*

The answer is that nothing decides to. The probe is diagnostic code in the
test's failure path, and the five seconds is the test's own assertion deadline.
Asking the question exposed something this record had been getting wrong.

## What the sequence actually is

1. The test submits its reads and hands the ring to `EventDelivery`.
2. It waits on a channel: `rx.recv_timeout(DELIVERY_BOUND)`, five seconds.
3. The stall happens. Nothing dispatches.
4. At five seconds `recv_timeout` returns `Err`. **The test has now failed.**
5. *Only then* does the failure path run `pool_liveness()`, which creates a
   work item and submits it -- to ask "is this pool alive at all?".
6. That submit releases the pool, and the completions arrive.

So the probe is not part of delivery, and nothing waits five seconds to submit
work. The stall lasts five seconds because that is when the *test* gives up and
runs a diagnostic that happens to repair what it was measuring.

## What that implied, and had never been tested

If the probe is the only thing in the process that submits work after the stall
begins, then every observation of the delivery "arriving late" was taken
*after* the repair. So: remove the probe, extend the post-mortem to sixty
seconds, and see whether the delivery ever arrives on its own.

## What was observed

3 failures in 1896 runs. In **all three**:

| | |
|---|---|
| delivery callbacks run | **0** |
| wait trampolines entered after the stall began | **0** |
| completions delivered | 0 of 8 |
| last record in the capture | 65.02s |
| report line | `still nothing after a further 60s` |

The trace holds nothing at all between the setup at about 2ms and the
post-mortem's own records at 65 seconds.

## The correction this forces

This record previously said, in four places, that the failure was **"not a lost
wakeup"** but **"a delayed dispatch that eventually delivers everything"**, and
that `D-68` had converted a permanent loss into a merely late one.

That was wrong, and it was wrong in the most avoidable way: the instrument was
repairing the fault before the measurement was taken. `M26.9`'s original
signature -- a permanent lost wakeup, `callbacks run: 0`, nothing after a
further wait -- was right all along. What `D-68` changed, if anything, is not
established.

All four statements are corrected in the same change as this measurement:
[UNRESOLVED-TEST-FAILURES.md](../../../windows-ioring-sys/UNRESOLVED-TEST-FAILURES.md),
[RESOLVED-TEST-FAILURES.md](../../../windows-ioring-sys/RESOLVED-TEST-FAILURES.md),
[CHECKLIST.md](../../CHECKLIST.md), and this directory's siblings by reference.

## What it does not change

Everything measured about *what releases it* still stands, because each of
those experiments kept the probe and varied something else: the stall ends on a
work submit and at no other time, nothing else dispatches during it, no Win32
call blocks, no exception is raised, and it is never seen off the default pool.

What changes is the severity and the wording. This is a hang, not a delay.