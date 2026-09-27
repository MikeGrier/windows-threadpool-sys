# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

## The M26.9 delivery stall still occurs; D-68 did not close it

**Found 2026-09-26**, by `M26.13`, which re-opened the mechanism. This entry replaces the claim in
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) that the stall was fixed.

**Tests:** `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting` and
`completions_queued_before_handover_are_still_delivered` in
[event_delivery.rs](tests/event_delivery.rs). They fail together, never singly, and only in parallel
with a co-running test that creates an `EventDelivery` and drops it promptly.

**Rate**, using that record's own reproducer against the current build: 2 failures in 600 runs and 2
in 900. The prior entry recorded 0 in 3600 after D-68's fix; that no longer holds.

**D-68's ordering is in effect and does not prevent it.** The trace of a captured failure shows
`wait armed` at 0.002286s and `setup-signalled` at 0.002288s -- arm first, then signal, exactly as
D-68 requires.

**It is not a lost wakeup. It is a delayed dispatch.** In the same capture the wait is armed and its
event signalled at 2.3ms, and `trampoline-entered` does not appear until **5.008724s**. The callback
then runs and every completion is delivered. The signature has therefore changed since M26.9, which
recorded `callbacks run: 0` and nothing after a further ten seconds; today the callbacks run and the
data arrives. D-68 appears to have converted a permanent loss into a delayed dispatch rather than
removing it.

**The delay ends when the test gives up.** Dispatch resumes at 5.0087s, microseconds after the 5s
`DELIVERY_BOUND` expires and the post-mortem begins, and both rings' waits then fire within 100us of
each other on one pool thread. Note that the report's "it arrived, 1.1us past the bound" is measured
from the *start of the post-mortem*, which is after the pool-liveness probe has already run -- so it
does not mean the delivery was 1.1us late.

**Ruled out: the thread pool's wait dispatch on its own.** A standalone experiment with no I/O ring
in it -- create an auto-reset event, `CreateThreadpoolWait`, `SetThreadpoolWait`, signal, wait for
the callback -- produced no stall in any configuration tried: 3000 trials under continuous wait
churn on three threads, 1500 trials where the churn runs concurrently with the signal and is then
joined before the victim is checked, and 1500 more where each churn cycle signals and drops its wait
immediately without waiting for the callback, which is what the trigger test does. Slowest ordinary
dispatch across those runs was 18.6us. Whatever the mechanism is, it needs the ring.

**Not established:** why dispatch is delayed. The remaining difference between the reproducer and
the isolation is the ring itself -- an event the kernel also signals, a callback that drains under a
mutex and re-arms itself, and `CloseIoRing` releasing the kernel's reference to that event. The next
experiments are to add those one at a time to the isolation, and to determine what actually releases
the stall at 5.0087s, since the pool-liveness probe's own `wait created` record is timestamped
*after* the first `trampoline-entered` and so cannot be the whole story.

**Why it matters beyond these two tests.** The sabotage harness runs the whole suite once per case,
and a suite that fails for this reason is recorded as the case being `caught`. That is the dangerous
direction: a sabotage the tests did not really catch reads as a clean bill of health.