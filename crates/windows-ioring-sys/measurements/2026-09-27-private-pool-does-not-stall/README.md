# A private pool does not stall, and the thread minimum is not why -- 2026-09-27

Four arms, four thousand runs each, answering `M26.13`'s experiment 2. It is a
**null result on the hypothesis the experiment was written to test**, and the
arm that makes it a null result is the one the original wording did not call
for.

## The question, and why it was split into four arms

The experiment as queued read: "if a private pool with `SetThreadpoolThreadMinimum`
does not stall, the supply reading is supported". That conflates two variables
-- being off the default process pool, and having a non-zero thread minimum --
so a clean private-pool-with-minimum arm would have been read as confirming
supply when it might only have shown that any private pool is clean.

Splitting them costs one extra arm and is the whole value of the result:

| Arm | Pool | Minimum |
|---|---|---|
| `default` | the default process pool, as shipped | n/a (control) |
| `private-min0` | one shared private pool | none set |
| `private-min1` | the same | `SetThreadpoolThreadMinimum(1)` |
| `private-min4` | the same | `SetThreadpoolThreadMinimum(4)` |

**All three `EventDelivery` objects in the reproducer share one pool in every
arm**, including the one built by the co-running create-and-drop that `M26.9`
identified as the trigger. So the variable is *which* pool, never
shared-against-isolated: the private arms are exactly as shared as the default
arm is.

## What was observed

| Arm | Failures | Runs |
|---|---|---|
| `default` | 13 | 4000 |
| `private-min0` | 0 | 4000 |
| `private-min1` | 0 | 4000 |
| `private-min4` | 0 | 4000 |

Every arm is the same size, which is what makes zero mean something: at the
control arm's own rate each private arm would have been expected to produce
about as many failures as the control did, and across the three of them about
three times that. None occurred. Three captures from the control arm are kept
in [default/](default); the private arms produced nothing to capture.

Two readings:

- **The stall requires the default process pool.** Any private pool eliminates
  it.
- **The thread minimum is not the variable.** `private-min0` sets no minimum at
  all and is already clean, so the arm the experiment was written around --
  a minimum of one or four -- adds nothing that `private-min0` had not already
  shown. The supply reading is **not** supported by this experiment. Had the
  experiment been run as originally worded, with only a private pool and a
  minimum, its clean result would have been read as confirming supply.

## The positive control, and why it was needed

A private arm whose callback environment silently named no pool would run on
the default pool and report "does not stall" for the wrong reason -- the one
way this experiment could produce a confident false negative. So every
`EventDelivery` construction asserted that its environment named a non-null
pool and recorded the pointer, and the assertion was checked by sabotage:
removing the `set_pool` call makes the private arm fail with
`the private-pool arm built an environment naming no pool`, as it must.

## What these runs do not say

They do not say **why**. "A private pool" is still a compound change: a
different pool object, its own thread set, and no sharing with whatever else in
the process uses the default one. Nothing here distinguishes those, and nothing
here observes the pool's internal state, so this is not evidence that the
default pool is defective -- only that the failure has never been seen off it.

They also do not make a remedy. `EventDelivery::new` already takes an
environment, so putting deliveries on a private pool is reachable today, but
who owns that pool, how many there are, and whether a workaround should be
adopted ahead of a diagnosis are decisions for the engineer, recorded in
[CHECKLIST.md](../../CHECKLIST.md) -> `M26.13` rather than taken here.