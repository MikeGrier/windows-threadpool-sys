# The 5000ms submit timeout is not what makes the stall five seconds -- 2026-09-27

Raised in review, and a fair thing to be suspicious of: the reproducer contains
a literal `5_000`, the stall lasts five seconds, and coincidences of that shape
are usually not coincidences.

It is one here. Changing the constant does not move the stall.

## The suspicion

`completions_queued_before_handover_are_still_delivered` waits for its reads on
the submitting thread before handing the ring over:

```rust
batch.submit_and_wait(CHUNKS as u32, 5_000)
```

The second argument is `timeout_ms`, so that really is five seconds, and it
really is the same number as the observed stall.

## The experiment

Change it to **4000** and re-run. The design is what makes this decisive: if
that constant governed the stall, dispatch would resume at about four seconds
-- which is *before* the test's own `DELIVERY_BOUND` of five -- so the delivery
would arrive in time and the tests would simply stop failing.

## What was observed

4000 runs with the timeout at 4000ms: **14 failures**, which is squarely in the
range this configuration has produced all day (13, 21, 14, 18 and 24 in 4000).

| Where dispatch resumed | Captures |
|---|---|
| about 4 seconds | **0** of 14 |
| about 5 seconds | 14 of 14 |

In every one, dispatch resumes within milliseconds of
`postmortem delivery-wait-expired` -- the test's `DELIVERY_BOUND` firing and the
probe running -- exactly as before. Three captures are kept here.

## Why it was never going to be this, and why it was still worth running

Three things already pointed the other way, but each is an inference and the
experiment is direct:

- **The call returns in about two milliseconds.** The ring that hands over with
  eight completions queued does so at 2.18ms, 2.54ms and 2.71ms in the three
  captures of
  [2026-09-27-no-win32-call-blocks](../2026-09-27-no-win32-call-blocks/README.md).
  The timeout is never consumed.
- **The other victim has no such constant.** The two delivery tests fail
  together, never singly, and
  `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting`
  submits with `submit_and_wait(0, 0)` -- no wait operations and no timeout at
  all.
- **The stall already moved once.** Delaying the post-mortem probe by two
  seconds moved the end of the stall to seven, and by three to eight
  ([2026-09-26-what-releases-the-stall](../2026-09-26-what-releases-the-stall/README.md)).
  A timer armed near t=0 cannot fire at 8.01s.

## What the five seconds actually is

There are two five-second constants in the reproducer and only one of them
matters. `DELIVERY_BOUND` is the test's own `recv_timeout`, and it decides when
the test gives up and runs the pool-liveness probe -- and the probe's work
submit is what ends the stall. So the stall lasts five seconds *because* that is
when the probe runs, not because anything waits five seconds.

**No five-second constant exists in either crate's library code.** Every one is
in a test.