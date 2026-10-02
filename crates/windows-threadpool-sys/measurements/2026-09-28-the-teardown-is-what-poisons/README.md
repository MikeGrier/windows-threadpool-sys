# The teardown is what poisons, not the setup -- 2026-09-28

Testing the reading that `dropping_with_nothing_outstanding_does_not_hang`
poisons the process, rather than competing with the victims for anything.

The trigger was made selectable so one build served five arms, 4000 runs each.
Figures in [arms.csv](arms.csv).

| arm | what the trigger does | failures in 4000 |
|---|---|---|
| `default` | ring + `EventDelivery`, **dropped** | **11** |
| `none` | nothing | 0 |
| `ring-only` | ring created and dropped, **no delivery** | 0 |
| `leak` | ring + `EventDelivery`, **never dropped** | **0** |
| `x4` | four create-and-drop cycles | **26** |

`default` is the positive control and was re-run in the same session rather
than compared against an earlier figure, so the three zeroes are measured
against a live reproducer.

## It is the teardown

`leak` is the arm that matters. The trigger builds exactly what it always
builds -- a ring, an event attached to it, a `ThreadpoolWait` armed on that
event -- and then does not drop it. **Zero failures in 4000.**

So the setup is not what poisons. Creating a delivery and arming its wait
leaves the process healthy; it is taking it back down again that does not.

`ring-only` closes the other half: a ring created and dropped, with no
`EventDelivery` over it, is also **zero in 4000**. The ring alone is not it
either. It takes the delivery *and* its teardown.

## What that teardown is

The whole of it, from a committed capture, on the trigger's own thread and
spanning 54 microseconds:

```text
  0.000000  delivery   event-attached
  0.000020  wait       created
  0.000022  wait       armed
  0.000023  delivery   armed
  0.000041  delivery   setup-signalled
  0.000042  wait       drop-begin
  0.000043  wait       suppress-and-disarm     <- SetThreadpoolWait(wait, NULL, NULL)
  0.000045  wait       disarmed
  0.000046  wait       drop-drained            <- WaitForThreadpoolWaitCallbacks
  0.000054  wait       drop-closed             <- CloseThreadpoolWait
```

A `TP_WAIT` is created, armed on an event the kernel's `IoRing` holds, and
then disarmed and closed 32 microseconds later -- before the pool has ever
dispatched anything, and about 2.4 ms before either victim arms its own
delivery.

## More cycles, more failures, but not proportionally

`x4` does the same create-and-drop four times: 26 failures against `default`'s
11. Four independent chances would give about 44, which the observation is 3.5
Poisson standard deviations below; no increase at all would give 11, which it
is 2.9 above. So repeating it raises the rate and does not quadruple it.

**That comparison is weaker than it looks and is not offered as dose-response.**
Four cycles take longer than one, which moves when the trigger finishes
relative to the victims arming -- and that interval is the window the race
lives in. The arm changes two things at once. What it does establish is the
direction: the poisoning is not a once-per-process event that the first cycle
exhausts.

## What this does not say

It does not say which *part* of the teardown does it. Disarm, drain and close
happen within 12 microseconds of each other here, and this measurement cannot
separate them. Nor does it say why a ring is needed at all when the wait is on
an ordinary event -- an earlier arm found six thousand ring-free trials
produced nothing, and `ring-only` now shows the ring alone is equally
innocent, so the two are needed together and the reason is not established.

The natural next decomposition is inside the teardown: arm and close without
disarming, arm and disarm without closing, create and close without arming.
That is a `windows-threadpool-sys` experiment rather than a ring one, and it is
queued rather than taken here.
