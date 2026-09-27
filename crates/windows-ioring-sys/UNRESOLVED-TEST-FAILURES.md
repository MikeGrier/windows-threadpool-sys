# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

## The M26.9 delivery stall still occurs; D-68 did not close it, and it is not this crate's fault

**Found 2026-09-26**, by `M26.13`, which re-opened the mechanism. This entry replaces the claim in
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) that the stall was fixed.

**The fault is not in this crate.** Established 2026-09-27 by `M26.13.4`: during the stall the
process thread pool dispatches **no callback of any kind**. A wait, a timer, and an I/O object each
created and armed *during* the stall, with no connection to any ring, all sit undispatched for a
full two seconds and then run only once a work item is submitted. Fifteen captures across five
configurations, with a control proving the stall was still live throughout:
[measurements/2026-09-27-which-poke-releases-the-stall/](measurements/2026-09-27-which-poke-releases-the-stall/README.md).
The ring is still needed to reach the state -- six thousand ring-free trials never produced it --
but nothing about the ring's own wait is what is stuck. The entry stays here because the failing
tests are here.

**It has only ever been seen on the default process pool.** `M26.13.5` put every `EventDelivery` in
the reproducer -- including the trigger's -- on one shared *private* pool: 0 failures in 4000 runs,
against 13 in 4000 on the default pool in the same build. A thread minimum makes no difference; the
arm with no minimum set was already clean, so the worker-supply reading the experiment was written
to test is **not** supported. Figures and the positive control in
[measurements/2026-09-27-private-pool-does-not-stall/](measurements/2026-09-27-private-pool-does-not-stall/README.md).

**Tests:** `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting` and
`completions_queued_before_handover_are_still_delivered` in
[event_delivery.rs](tests/event_delivery.rs). They fail together, never singly, and only in parallel
with a co-running test that creates an `EventDelivery` and drops it promptly.

**Rate**, using that record's own reproducer against the current build: 2 failures in 600 runs and 2
in 900. The prior entry recorded 0 in 3600 after D-68's fix; that no longer holds. Re-measured on
2026-09-26 with the fuller trace, in three configurations: 3 in 1200, 3 in 1312, and 3 in 1070; and
on 2026-09-27 across five more, from 3 in 525 to 3 in 2105.

**D-68's ordering is in effect and does not prevent it.** The trace of a captured failure shows
`wait armed` at 0.002286s and `setup-signalled` at 0.002288s -- arm first, then signal, exactly as
D-68 requires.

**It is not a lost wakeup. It is a delayed dispatch.** In the same capture the wait is armed and its
event signalled at 2.3ms, and `trampoline-entered` does not appear until **5.008724s**. The callback
then runs and every completion is delivered. The signature has therefore changed since M26.9, which
recorded `callbacks run: 0` and nothing after a further ten seconds; today the callbacks run and the
data arrives. D-68 appears to have converted a permanent loss into a delayed dispatch rather than
removing it.

**The delay ends when a work item is queued to the pool, and not before.** Corrected on
2026-09-26 by `M26.13.3`, which replaces this entry's earlier reading that it ends "when the test
gives up". It does not end on a timer, it does not end on its own, and the coincidence with the 5s
`DELIVERY_BOUND` is a coincidence of *when the pool-liveness probe runs*: delaying the probe by two
seconds delays the end of the stall by two seconds, and delaying it by three delays it by three.
Across nine captures in three configurations the record immediately preceding the first
`trampoline-entered` is always `work submitted`, a few hundred microseconds earlier; a two-second
quiet period inserted before the probe contains no record of any kind; and creating the work object
is not enough, since a one-second gap between `ThreadpoolWork::new` and `submit` passes in the same
silence. Captures and figures:
[measurements/2026-09-26-what-releases-the-stall/](measurements/2026-09-26-what-releases-the-stall/README.md).

The probe's own `wait created` is stamped after the first `trampoline-entered` in all nine, which is
what made the probe look like it could not be the cause. It is the **work** half of the probe that
precedes dispatch, not the wait half.

The report's "it arrived, N past the bound" is still measured from the *start of the post-mortem*,
which is after the probe has run, so it does not mean the delivery was N late.

**Ruled out: the thread pool's wait dispatch on its own.** A standalone experiment with no I/O ring
in it -- create an auto-reset event, `CreateThreadpoolWait`, `SetThreadpoolWait`, signal, wait for
the callback -- produced no stall in any configuration tried: 3000 trials under continuous wait
churn on three threads, 1500 trials where the churn runs concurrently with the signal and is then
joined before the victim is checked, and 1500 more where each churn cycle signals and drops its wait
immediately without waiting for the callback, which is what the trigger test does. Slowest ordinary
dispatch across those runs was 18.6us. Whatever the mechanism is, it needs the ring.

**Not established:** why the pool stops dispatching, and why a work submit is the one thing that
restarts it. Nothing measured so far observes the pool's own thread accounting; the interval between
an object being armed and its trampoline being entered is inside the pool, where this workspace's
trace cannot reach. The next experiments are queued in [CHECKLIST.md](CHECKLIST.md) under `M26.13`.

**Why it matters beyond these two tests.** The sabotage harness runs the whole suite once per case,
and a suite that fails for this reason is recorded as the case being `caught`. That is the dangerous
direction: a sabotage the tests did not really catch reads as a clean bill of health.