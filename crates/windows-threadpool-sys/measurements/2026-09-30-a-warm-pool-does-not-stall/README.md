# A warm pool does not stall

2026-09-30. `M-T5.11`. Two runs, 12000 processes per arm each. Counts in
[cold-warm-delay.csv](cold-warm-delay.csv) and
[cold-warm-first-run.csv](cold-warm-first-run.csv).

**Making the pool create its first worker before the trigger runs prevents the
fault entirely.** On the evidence here this is a cold-start hazard, not a
steady-state one.

## The question

Every stall this investigation had ever captured was of a pool holding zero
threads that had never made one -- the reproducer's trigger runs at 0.00004s,
before the pool has dispatched anything. Whether that was incidental or a
precondition decides the shape of the risk: a narrow window near process start,
or continuous exposure.

## Result

| arm | pool state when the trigger runs | failures | per 1000 |
|---|---|---|---|
| `hand-spin-3us` | cold -- no worker ever made | **66** / 24000 | 2.75 |
| `hand-spin-3us-delay` | cold, but the trigger runs ~400us later | **31** / 12000 | 2.58 |
| `hand-spin-3us-warm` | **one worker already parked** | **0** / 24000 | **0** |

Pooled across both runs. Under the null that warming does not matter, the cold
rate predicts about 66 failures in the warm arm's runs; zero has probability of
order 1e-29.

Every warm run is individually confirmed: the harness counts the `warm=true`
marker each process prints, and the figure is **12000 of 12000** in both runs. A
warm arm that silently ran cold would have reported the cold rate under the warm
name, so the arm aborts if its warm-up fails rather than proceeding.

## The delay arm is why this is readable

Warming costs time -- measured at 223-304us -- so the warm arm's trigger runs
later than its cold twin's, and "later" is a second explanation for any
difference. The `-delay` arm pays **400us**, deliberately more than warming
costs, in a spin that does not touch the pool.

It fails at the cold rate. So elapsed time before the trigger does not account
for the result, and having a worker is what does.

(That the delay arm fails is also consistent with everything else here: the
hazard window is between the disarm and the close, a few microseconds wide, and
delaying the whole trigger does not move the two calls apart.)

## What this means

The fault needs a pool with **no threads**. Once the pool has made its first
worker, the same trigger at the same rate produces nothing.

The mechanism this fits -- and it is a fit, not a proof -- is that the severed
notification is the one asking the factory to **create** a worker. A pool with a
thread already parked for work does not need that question asked: the arriving
packet is handed to the waiting thread. Breaking the create-request path then
costs nothing until there is no thread to hand work to.

For a program, the exposure is therefore:

- **near process start**, before the pool's first dispatch; and
- **after each idle-timeout expiry**, when the last worker retires and the pool
  becomes cold again -- measured at 67s of idleness for the default pool.

A process that keeps its pool busy is not exposed between those points.

## What it does not establish

- **That a warm pool cannot be damaged by any trigger.** It says this trigger, at
  this rate, does not damage a warm pool in 24000 runs. A different timing, or a
  pool with a worker that is busy rather than parked, is untested.
- **That a cold pool is the only vulnerable state** in a stronger sense than the
  above. "Cold" here means "never made a worker"; a pool that has gone cold again
  after its workers retired is a distinct state, and is `M-T5.10`.
- **Anything about the fix.** Draining remains the correct teardown regardless;
  this bounds when a *non*-draining teardown is dangerous, which is a statement
  about exposure, not about what the code should do.

## Method

Arms interleaved round-robin across six parallel workers, so machine-load drift
is shared rather than assigned to whichever arm ran during it; per-arm cost is in
the CSVs as the confound check. Each worker has its own temp directory. Every run
is a fresh process scored by exit code.

The warm-up submits a work item and waits for its callback. **A callback having
run is the proof a worker exists**, because it ran on one, and the worker then
stays parked for the idle timeout -- far longer than the rest of the test. That is
why the confirmation is "the callback ran" rather than a counter read, which would
have been a second thing to get wrong.
