# The blast radius is one pool

2026-09-30. What a single occurrence of this fault actually takes down. Assembled
from measurements already taken; no new run.

**One worker factory and its completion port -- everything on that pool, and
nothing off it.** For the pool that was observed to be affected, "everything on
that pool" means most of the process.

## Everything on the pool stops

Not the wait object, not waits as a kind. Every dispatch mechanism the pool
offers, measured in [which-poke-releases-the-stall](../2026-09-27-which-poke-releases-the-stall/README.md)
with a two-second observation window each:

| mechanism, exercised during the stall | dispatches? |
|---|---|
| the waits that were already armed | no |
| a **fresh** wait, armed and signalled | no |
| a **fresh** timer, due in 1ms | no |
| a **real overlapped read** that completed | no |
| a work item submitted | yes -- and it recovers the pool |

The poked objects' *own* callbacks do not run either, so this is not a property
of the ring, or of the particular waits, or of waits at all. The pool is simply
not dispatching.

## Nothing off the pool stops

Every capture of this process holds **two** worker factories, and only one is
affected. While the default pool sits at zero workers with work queued, the other
keeps three workers parked and healthy for the entire five-second stall, and its
own port reads depth 0 throughout.

That second factory is **not** created by this workspace. The reproducer has no
`ThreadpoolPool`, no `CreateThreadpool`, and no thread-count call anywhere; the
factory is present from 0.002s in healthy runs of the same binary, and carries a
maximum of 3 threads with a 30s idle timeout against the default pool's 768 and
67s. What creates it is not established here. That it survives untouched is.

So the fault does not escape the pool it happens on. It is scoped to one
completion port and the factory bound to it.

## Which makes the affected pool the whole story

The pool observed to stall is the **default process pool** -- the one every
thread-pool API uses when no callback environment names another. So although the
blast radius is "one pool", in practice that is every component in the process
that did not explicitly opt out: unrelated libraries, the runtime, anything using
`TrySubmitThreadpoolCallback` or a bare `CreateThreadpoolWait`.

A library that tears a wait down this way takes out dispatch for code it has
never heard of, in a process it does not own.

## A private pool was not observed to stall

[private-pool-does-not-stall](../2026-09-27-private-pool-does-not-stall/README.md)
moved all three of the reproducer's delivery objects -- including the trigger --
onto one shared private pool, and saw **0 stalls in 12000 runs** across three
arms, against a default-pool control that stalls.

**Read that as what it is.** It says this fault was not reproduced on a private
pool in that configuration. It does not establish that a private pool cannot be
damaged the same way, and no mechanism is offered here for why the default pool
would be special. Treating "use a private pool" as a mitigation would be binding
to an unexplained negative.

## What this does not say

Nothing here bears on how long the damage lasts. Recovery and duration are in
[what-a-process-does-after-the-stall](../2026-09-30-what-a-process-does-after-the-stall/README.md),
and the question of whether the pool breaks again once its workers retire is open.
