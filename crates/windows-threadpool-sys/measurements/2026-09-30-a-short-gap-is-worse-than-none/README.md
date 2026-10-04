# A short gap is worse than no gap

2026-09-30. Two independent runs, 20004 processes per arm, asking where between
0us and 30us the gap between the disarm and the close starts to help.

**The answer is that it first makes things worse.** The rate peaks at about 3us,
roughly five times the no-gap rate, and decays from there. Counts for both runs
are in [arms.csv](arms.csv).

## Why the question was asked

[what-the-gap-is-made-of](../2026-09-29-what-the-gap-is-made-of/README.md)
bracketed the effect very loosely: 0us failed and 30us did not, with nothing
measured in between. That is consistent with a settling time -- wait long enough
and the hazard passes -- and it was read that way. The engineer asked whether the
only difference between the passing and failing arms really was the delay, which
is what exposed the gap in the bracket.

## The shape

Pooling the arms both runs share:

| gap | failures | runs | per 1000 |
|---|---|---|---|
| 0us | 35 | 40008 | 0.87 |
| 3us | 166 | 40008 | 4.15 |
| 5us | 47 | 20004 | 2.35 |
| 7us | 14 | 20004 | 0.70 |
| 10us | 19 | 40008 | 0.47 |
| 30us | 1 | 40008 | 0.025 |

The peak replicated across two runs measured hours apart under different machine
load (89 and 77 against no-gap counts of 15 and 20 in the same runs), so it is
not a cluster in one sample.

Three independent checks that the excess is real events rather than a counting
artifact. Both runs interleave their arms round-robin across the same six
workers, so load is shared rather than assigned to whichever arm ran during it.
Each arm's mean per-run cost tracks its own failure count -- the 3us arm is the
slowest in both runs, by about the time its extra stalls cost, and the 30us arm
is the fastest. And every run is a fresh process, so no run can influence the
next.

## What it rules out, and what it opens

A settling time cannot produce this. "The close is unsafe until the disarm has
finished landing" predicts that a longer gap is never worse than a shorter one,
and 3us is worse than 0us by a factor of about five.

What fits the shape is a **race against something the disarm starts**, which
takes a few microseconds to run. A close issued immediately arrives before that
work begins; one issued at 3us arrives while it is in progress; one issued at
30us arrives after it is done. Nothing here identifies what that work is -- that
is inside the pool or the kernel, where this workspace has no instrument -- but
its approximate duration is now measured rather than guessed.

## A correction owed on the earlier 30us figure

The previous artifact reported 0 failures in 20004 at 30us and the timeline row
read "30us is already enough". Pooled over 40008 runs the figure is 1, not 0.

The difference does not change any ranking -- 30us is still about 35 times better
than no gap and 166 times better than the peak -- but it changes the claim that
can be made from it. "Enough" overstates a single zero; what the two runs support
is a large reduction, with no evidence either way about whether the floor is zero.

**This does not describe the shipped fix, and the distinction matters.** M-T4.2
drains, which blocks until the queued callback has run -- measured at about 280us
here, an order of magnitude past the 30us arm, and it reaches 0 in 20004 in the
hand-rolled model and 0 in 70000 on the real path. A 30us gap is a partial
mitigation that happens to lie on the same axis; it is not what was shipped and
is not offered as an alternative to it.

## The two arms that differ only in timing

Recorded because it is the question that prompted the run, and because the
obvious candidate pair is the wrong one. `hand-control` spends about 1us inside
`WaitForThreadpoolWaitCallbacks(wait, TRUE)`, so it differs from a gap arm in two
ways -- the extra call and the elapsed time -- and cannot be read as "the same
thing, briefly delayed".

The pair that differs only in timing is `hand-nocancel` against a spin arm. Both
run exactly this sequence:

1. create an empty ring
2. create its completion event and register it with the ring
3. `CreateThreadpoolWait` on the default pool, with a callback that does nothing
4. `SetThreadpoolWait(wait, event, NULL)` -- arm
5. `SetEvent(event)` -- the wait is now satisfied, so a callback is owed
6. `SetThreadpoolWait(wait, NULL, NULL)` -- disarm
7. *(the gap, and nothing else, goes here)*
8. `CloseThreadpoolWait(wait)`
9. close the event, then the ring

The gap is a busy-wait on the pause instruction. It issues no system call, yields
nothing, and blocks nothing -- on a 16-core host the pool is free to proceed
throughout. In every arm the close at step 8 happens while the callback owed at
step 5 has still not run.
