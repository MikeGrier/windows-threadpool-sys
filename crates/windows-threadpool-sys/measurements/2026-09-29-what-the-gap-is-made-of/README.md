# What the gap is made of

2026-09-29. Six runs of the hand-rolled trigger, about 250000 processes in all,
asking what it is about a gap between the disarm and the close that prevents the
stall.

**It raises a mechanism, kills it, and ends somewhere else.** The middle sections
record a hypothesis that fit every cell of a factorial, was directly observed
rather than inferred, predicted the magnitude of the known fix -- and is false.
Read to the end before quoting any of it.

## The question

[closing-too-soon-after-the-disarm](../2026-09-28-closing-too-soon-after-the-disarm/README.md)
established that a gap of about 1.4ms between `SetThreadpoolWait(NULL)` and
`CloseThreadpoolWait` prevents the stall, and that the same delay spent after the
close does not. It did not say *why* a gap helps.

The engineer asked for three arms to separate the candidates, on the observation
that a 1ms sleep is really a timer wait rounded up to the system tick: spend the
gap in the yield syscall instead, spend it in the pause instruction, and spend it
sleeping with the timer raised to its finest resolution.

## Run 1 -- how the gap is spent makes no difference

[how-the-gap-is-spent.csv](how-the-gap-is-spent.csv). Six arms interleaved
round-robin across six workers, 10002 runs each.

All four ways of spending the gap reach zero. The two controls -- no gap, and the
same delay after the close -- fail at the same rate as each other. The timer is
not implicated and the scheduler is not implicated.

**But the run cannot support that reading**, and the flaw is in its construction
rather than its execution. The control spends its gap inside
`WaitForThreadpoolWaitCallbacks(wait, TRUE)`; the four gap arms do not call that
function at all, so they differ from the control in two ways at once. Since
M-T4.2 showed that flipping that call's flag from TRUE to FALSE fixes the fault
*with no timing change whatsoever*, "the cancel call is the mechanism" fits this
table exactly as well as "elapsed time is".

## Run 2 -- the 2x2 that separates them

[cancel-and-gap.csv](cancel-and-gap.csv). Cancel call {made, not made} x gap
{none, 1ms}, 10002 runs per cell.

Neither factor moves the rate alone. Only the cell with both reaches zero.

## Run 3 -- a mechanism that fits every cell

`SetThreadpoolWait(NULL)` is measured not to cancel an already-queued callback,
so after the disarm a callback may still be queued. That suggests a rule covering
all four cells: the pool is poisoned when a wait is closed without its queued
callback ever having run. The cancel discards it; a gap without the cancel lets
it run; a drain waits for it; closing at once gives it neither.

That is observable rather than inferential, so it was observed. The callback was
changed to stamp `QueryPerformanceCounter` when it ran, and the trigger to report
whether that had happened by the time the close was issued:
[did-the-callback-run.csv](did-the-callback-run.csv). The indicator separates the
four cells perfectly, and
[callback-latency-vs-gap.csv](callback-latency-vs-gap.csv) finds the transition
where the measured latency distribution says it should be.

## Run 4 -- and the prediction it makes is false

The rule is quantitative, so it was made to predict rather than merely fit: if
closing without the callback having run is the poison, the stall rate must track
the *fraction* of runs in which that happens -- a gap that lets the callback run
only a third of the time should stall about two-thirds as often, reduced rather
than eliminated.

[gap-swept-against-callback-ran.csv](gap-swept-against-callback-ran.csv) measures
both quantities in the same processes under the same load, four gap lengths and a
live control, 10002 runs each. (Pairing matters: callback latency stretches under
parallel load, so a fraction imported from the quieter serial sweep would not
describe these runs.)

Every gap arm reaches zero, including ones whose own paired indicator says the
great majority of their runs closed the wait with the callback still pending --
the condition the rule names as the poison, occurring thousands of times, with no
stalls.

**The rule is false.** Whether the callback had run was a correlate of elapsed
time in the 2x2, not the cause, and a factorial with one cell per corner cannot
distinguish those.

## Runs 5 and 6 -- what the gap actually has to be

Two questions were left. How short a gap still works, and whether the cancel path
is genuinely immune to time or merely needed longer than 1ms.

[how-short-a-gap-works.csv](how-short-a-gap-works.csv) sweeps the gap down to
tens of microseconds and holds a 10ms gap with the cancel call still made.
[cancel-and-gap-are-both-required.csv](cancel-and-gap-are-both-required.csv) then
settles it at 20004 runs per arm, with a no-cancel-no-gap baseline in the same
run -- which run 5 lacked, and which is the arm that decides whether the gap
matters at all.

Both agree, and together they give the rule:

**The teardown needs both that it make no cancel call and that the close be well
separated from the disarm. Either condition alone fails.**

> **Refined 2026-09-30.** This section's "at least a few tens of microseconds"
> reading implied a settling time, and a finer sweep shows that is wrong: a *short*
> gap is worse than no gap, peaking about five times higher at 3us before decaying.
> See [a-short-gap-is-worse-than-none](../2026-09-30-a-short-gap-is-worse-than-none/README.md),
> which also corrects the 30us figure below from 0 to 1 failure once pooled over
> 40008 runs. The ranking is unchanged; the mechanism implied by it is not.

- Dropping the cancel call alone does nothing: with no gap it fails at the
  control's rate.
- A gap alone does nothing: with the cancel call still made, 1ms does not help
  and neither does 10ms. That path is immune to time over four orders of
  magnitude.
- Without the cancel call, a gap of 30us already gives a large reduction (see the
  refinement above: 1 failure in 40008 once pooled, not the 0 in 20004 this run
  alone showed).
- The drain that M-T4.2 ships is the same cell reached honestly: no cancel, and
  blocking until the callback has run.

The callback-ran indicator is what makes the last point worth stating precisely.
In the 30us arm it stands at roughly one run in a thousand -- the callback
essentially never runs -- and the arm still reaches zero. Whatever the gap buys,
it is not the callback's dispatch.

## What survives

- How the gap is spent -- timer, yield syscall, pause instruction -- makes no
  difference.
- The cancel call and the gap are separate effects, both necessary.
- The cancel call poisons in a way that elapsed time does not heal.
- Closing within a microsecond or two of the disarm poisons even with no cancel
  call, and tens of microseconds heal that.
- "Closed without its queued callback having run" does not describe the fault.

## What is still open

Why either effect exists. A close issued a microsecond after a disarm is
poisonous and one issued thirty microseconds after is not, which has the shape of
something in the pool or the kernel needing to settle; but nothing here reaches
inside to say what. And the cancel call's immunity to time is a different shape
again -- not a race that waiting fixes, but a state the call leaves behind.

## Method

Every run is a fresh process running the same three-test subset, scored by exit
code. Arms are interleaved round-robin across six parallel workers so machine-load
drift is shared rather than assigned to whichever arm ran during it; per-arm cost
is reported in each CSV as the confound check. Each worker gets its own temp
directory and purges it as it goes, because the reproducer leaks two files per run
(M26.14).

**Per-run cost varies by an order of magnitude across these runs** -- the machine
was materially busier during the earlier ones. The live control is what makes them
comparable anyway: it is present in every run and its rate stays put, so the
timing spread is a fact about the host rather than about the arms. No claim here
rests on comparing an arm in one run against an arm in another without that
control between them.

Two guards on the instrument itself, both earned. The trigger prints its variant,
the cancel flag it resolved, the measured gap, and whether the callback had run,
on every run -- so an arm that silently fell through to the default is visible in
the output rather than inferred from a plausible-looking rate. A previous
experiment was voided entirely by a PowerShell `.Replace()` that failed to match
its anchor and left every arm running the same code. And the paired harness was
smoke-tested before use, which is how it was found that `--nocapture` plus
`$ErrorActionPreference = 'Stop'` kills a worker before it writes its results.
