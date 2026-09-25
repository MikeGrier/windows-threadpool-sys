# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

None currently, in the sense that no test fails a routine run. One **intermittent** failure is
recorded below, because it is the kind that reports as somebody else's defect.

## event_delivery's threadpool tests time out at roughly one run in eighty

**Found 2026-09-24**, while widening the seeded sweeps to 2048.

**What happens.** `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting`
and `completions_queued_before_handover_are_still_delivered` in
[event_delivery.rs](tests/event_delivery.rs) each wait on a channel with
`recv_timeout(Duration::from_secs(5))` for a completion delivered through the Windows thread pool.
Occasionally the completion does not arrive inside that bound and the test panics with `Timeout`.

**Measured rather than estimated**, because the rate is the whole point: **1 failure in 80
consecutive runs** of the compiled test binary when first found. It resisted every targeted attempt
to provoke it at that stage -- zero failures after roughly 18,000 ring create/close cycles, after
repeated property-suite and calibration runs, and under a concurrent `cargo build` saturating the
machine -- so it was neither ring-resource pressure nor CPU load. The narrowing below found what it
actually needs.

## Narrowed 2026-09-25: it requires parallel test execution, and a co-running create-and-drop

The instrumentation described further down paid for itself immediately. One captured occurrence plus
four follow-up experiments moved this from "cause unknown" to a minimal reproducer. Every figure
here comes from running the compiled `event_delivery` binary directly.

**It does not happen serially.** With `--test-threads 1`: **0 failures in 1000 runs**. In parallel:
**7 in 1000**. At the parallel rate a thousand serial runs would expect about seven, so zero is
evidence rather than a quiet stretch.

**Every occurrence is identical**, across all seven captures:

- **both** delivery tests fail in the same process, never just one;
- `callbacks run: 0` -- the pool never invoked the callback, not once, for either ring;
- `delivered: 0 of 8` and `outstanding: 8` -- nothing was ever popped;
- the post-mortem finds nothing after a further ten seconds.

So it is **not** a slow device and **not** a single lost wakeup. No callback runs at all, for both
rings, from the start, and the delivery never arrives.

**The two delivery tests alone do not cause it**: 0 failures in 1000 runs with a filter selecting
only those two. A third test has to be running. Adding them one at a time, 600 runs each:

| Co-running test | Failures in 600 |
|---|---|
| `dropping_with_nothing_outstanding_does_not_hang` | 5 |
| `new_succeeds_and_the_ring_stays_reachable_for_pushes` | 2 |
| `teardown_with_operations_in_flight_neither_hangs_nor_closes_the_ring_early` | 0 |

The two that trigger it both create an `EventDelivery` over a ring with **nothing outstanding** and
drop it promptly; the one that does not is the one holding operations in flight. That is a
correlation across three tests, not a mechanism, and it is recorded as such.

**Reproducer**, about half a minute:

```powershell
$ed = 'target\debug\deps\event_delivery-<hash>.exe'   # the build with 6 tests; check with --list
$fail = 0
for ($i=1; $i -le 600; $i++) {
  & $ed completions_ dropping_with 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { $fail++ }
}
"$fail failures of 600"
```

**Where this goes next, and why it left this crate.** Both delivery tests pass `env: None` to
`EventDelivery::new`, so both register their wait on the **default process threadpool** through
[`windows_threadpool_sys::wait::ThreadpoolWait`](../windows-threadpool-sys/src/wait.rs). The failure
is that one object's lifecycle appears to stop *other, unrelated* armed waits from ever firing,
which nothing in `windows-ioring-sys` explains on its own. That crate's `Drop` was read and only
touches its own object, so the mechanism is **not yet established** -- and this record stops there
rather than guessing past it.

**Why it is worth recording despite being rare.** The sabotage harness runs the whole suite once per
case, and the manifest currently holds 41 cases. At the measured rate that is about a **40% chance
that any given sweep contains at least one corrupted result** -- and the corruption is the dangerous
direction: a sabotage the suite did not really catch is reported as `caught`, which reads as a clean
bill of health. Both instances seen so far landed on cases whose patches **provably cannot** affect
event delivery -- a failure-code bitmask in the resolver, and a prose reword inside an example's
contract text -- which is how they were recognised as false rather than believed.

**How to tell a false `caught` from a real one.** Read the per-case transcript under
`.scratch/sabotage/`; a genuine detection names a test related to the patch, while this one names
one of the two tests above and prints the stall report described next. Do not conclude a sweep is
clean or dirty from the summary table alone while this is open.

**Explicitly not caused by the 2048 sweep widening**, though that is when it was noticed. The rate
was measured on the `event_delivery` binary, which uses neither the resolver nor any seeded sweep,
so its behaviour is independent of those constants. Three sweeps at the previous sizes had passed
earlier the same day, which is unsurprising at this rate rather than evidence of a change.

## What a stalled run now records

Added 2026-09-25. The original failure said only `Timeout`, which ruled nothing out -- that is why
the investigation above could only proceed by elimination. Both tests now print a report on the way
out, to stderr and into the panic message, so `cargo test`'s captured output and the sabotage
harness's per-case transcript both carry it. It states:

- **delivered, of how many expected** -- whether the stall was immediate or partway through.
- **callbacks run** -- how many times the pool actually invoked the callback. Equal to delivered
  means everything the callback received reached the test thread; greater means the gap is between
  the callback and the channel. This is the first fork in the diagnosis and nothing else supplies
  it.
- **outstanding** -- the ring's own count, with the caveat that makes it readable: it decrements on
  pop and the pop happens *inside* the callback, so on its own it cannot separate "the kernel has
  not finished" from "the callback never ran".
- **arrival times and inter-arrival gaps** -- whether deliveries were steady and then stopped, or
  slow throughout.
- **a post-mortem** -- after the bound expires the test waits a further ten seconds and says whether
  the delivery arrived late or never came at all. Those have different causes, and no other datum
  separates them.

Two properties of that reporting are deliberate. The immediate facts are printed **before** the
post-mortem wait, so they survive the sabotage harness killing a run that exceeds its hang bound --
losing the report to the very timeout it exists to explain would be the worst outcome. And the
post-mortem is ten seconds rather than thirty so a failing run stays inside that bound: measured, a
forced stall completes in about seventeen seconds against a bound of roughly thirty.

**The report states observations and stops.** An earlier draft ended with a verdict, and a
forced-failure run showed the verdict was wrong -- it blamed something upstream of the channel when
the injected fault was in the callback body, which the counters it had just printed already ruled
out.

**Queued as `M26.9`** in [CHECKLIST.md](CHECKLIST.md).
