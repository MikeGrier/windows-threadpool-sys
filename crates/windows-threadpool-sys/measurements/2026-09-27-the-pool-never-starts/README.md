# At 100ms the heartbeat becomes a clock, and says the pool never starts -- 2026-09-27

The same standing timer as
[2026-09-27-a-timer-armed-while-healthy-also-stops](../2026-09-27-a-timer-armed-while-healthy-also-stops/README.md),
with its period changed from four seconds to a hundred milliseconds. At four
seconds it could only say the timer was late. At a hundred milliseconds it is
fine-grained enough to say *when* dispatch stopped -- and the answer is that in
a stalled run it never starts.

## What was observed

18 failures in 4000 runs, squarely in the range this configuration has produced
all day (13, 21, 14, 18, 24, 14, 10). A constantly-expiring timer does not
prevent the fault.

The timer is created and armed in the first few microseconds:

```text
      0.000017s syscall-enter          CreateThreadpoolTimer
      0.000021s syscall-leave          CreateThreadpoolTimer
      0.000023s timer                  created
      0.000027s syscall-enter          SetThreadpoolTimer
      0.000030s syscall-leave          SetThreadpoolTimer
      0.000031s timer                  armed
```

So it is **due at 0.100031s**, and every 100 ms after that. In all 18 captures:

| | |
|---|---|
| firings before the release | **0** |
| firings in total | **1**, at the release |
| consecutive expiries missed | about 50 |

## The stronger statement this licenses

At four seconds, the finding was "a timer armed while healthy fires late". At a
hundred milliseconds the first expiry lands at 0.1s -- well inside the window --
and it is missed too. Pushing further: across all 18 captures, **not one pool
callback of any kind is dispatched before the release**. No wait, no timer, no
work, nothing.

So this is not a pool that runs for a while and then wedges. In a run that
stalls, the pool dispatches **nothing at all, from process start until a work
item is submitted**. The five seconds is not a period during which the pool
stopped working; it is a period during which it never started.

That is a simpler claim than "it wedges at some moment", and it is what the
clock was added to settle.

## It remains its own positive control

The timer fires exactly once, at the release, which proves the arming took
effect and the machinery works. A probe that never fired at all would have been
ambiguous between a wedged pool and a broken probe.

## What it does not say

It does not say why, and it does not distinguish "the pool would never have
dispatched in this process" from "the pool was put into this state during
setup, before anything needed dispatching". Nothing in these runs required a
callback before the deliveries were armed at about 2.5 ms, so there is no
earlier successful dispatch to compare against.

Bracketing that needs a shorter period still -- one whose first expiry lands
*before* the deliveries are set up -- so that a healthy process shows firings
and a stalled one shows where they stop. That is the obvious next step and is
queued rather than taken here.