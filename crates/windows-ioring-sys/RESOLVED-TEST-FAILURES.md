# Resolved test failures: windows-ioring-sys

Append-only. Entries arrive here from [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md)
in the same commit that removes them from it.

## Resolved 2026-08-28 21:03:52 -04:00 -- buffer registration completes with ERROR_NOACCESS

**Tests:** four in [tests/registration.rs](tests/registration.rs), all failing on the same call:

- `a_read_addressing_a_registered_file_and_a_registered_buffer_round_trips`
- `a_registered_buffers_from_a_different_ring_is_rejected`
- `a_second_file_or_buffer_registration_on_the_same_ring_is_refused`
- `dropping_a_registration_with_an_operation_in_flight_leaks_rather_than_frees`

**Root cause:** a live use-after-free in shipped 0.1.2, not a test defect.
`BuildIoRingRegisterBuffers` reads its `IORING_BUFFER_INFO` array when the registration op *runs*
-- during a later `SubmitIoRing` -- not when the `Build*` call returns. `Batch::register_buffers`
built that array in a local `Vec` and dropped it before submitting, so the kernel read freed heap
and the registration completed with `HRESULT_FROM_WIN32(ERROR_NOACCESS)` (`0x800703E6`).

Because `register_buffers` is a **safe** `pub fn`, safe code could cause the kernel to dereference
a dangling pointer. The observed symptom was benign (a reported error), but a reallocation landing
on those bytes first would have registered whatever addresses happened to be there.

**How it was established:** a spike (`.scratch/ioring-bufreg-spike`) crossed the two hypotheses that
produce this error code -- array lifetime and buffer alignment -- rather than testing either alone:

| | align 1 | page-aligned |
|---|---|---|
| infos dropped before submit | `ERROR_NOACCESS` | `ERROR_NOACCESS` |
| infos alive through submit | `S_OK` | `S_OK` |

Alignment was the initial suspicion and is **disproved** in both directions: an align-1 `Vec<u8>`
succeeds when the array is alive, and a page-aligned buffer still fails when it is dropped. Two
further measurements shaped the fix: the array may be released once `SubmitIoRing` returns (so the
requirement is "alive until submit", not "until the completion is observed"), and
`BuildIoRingRegisterFileHandles` genuinely *does* read its `handles` array synchronously -- which is
why the file-registration tests always passed, and why the crate's rustdoc had generalized the
synchronous claim from one registration to the other.

**Fix:** the `IORING_BUFFER_INFO` array is now owned by the `IoRing` for its remaining life, and the
SQE is built from that pointer rather than from a local about to go out of scope. Held by the ring
rather than the `Batch` because a failed submit leaves the SQE queued as ring state (D-5), so a
later unrelated submit can be what finally runs it. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> [D-32](DESIGN-NOTES.md#d-32).

**Regression cover:** `a_buffer_registration_survives_heap_churn_between_the_push_and_the_submit`
allocates hard between the push and the submit, so a future regression finds its freed bytes reused
rather than conveniently intact. Verified by sabotage: reverting the fix makes that test fail with
the original `0x800703E6`, and restoring it makes it pass.

## Resolved 2026-09-06 16:19:04 -04:00 -- the "flaky" flush-barrier test was asserting a false claim

**Test:** `flush_barrier::a_covering_flush_waits_for_preceding_writes_and_an_unordered_one_does_not`
in [tests/flush_barrier.rs](tests/flush_barrier.rs).

**What it was recorded as.** Flaky under a full-workspace run, green in isolation. Seen on
2026-09-03 (2899 of 2900 passed) and again on 2026-09-06 (978 of 979). Both times the test passed
alone and the observing change had not touched it, so it was filed as load sensitivity in a real-I/O
test, with the open question being whether to make the assertion load-independent or mark the test
serial.

**What it actually was.** Neither. The test asserted two guarantees and the second one is false.
`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` drains operations queued *before* it -- solidly, in every one of
about 4,500 trials -- but does **not** hold back operations queued after it, which
[DESIGN-NOTES.md](DESIGN-NOTES.md) `D-24` claimed and `D-47` now corrects. Post-flush writes overtake
the flush at 0.03%-0.8%, and in the worst trial all 32 did.

**How it was settled.** Purpose-built stress instruments that repeat the sequence under quiet,
contended, and concurrent-ring conditions, keeping a per-trial event log -- submissions,
completion-queue drain rounds, and every pop with its phase and timing -- so a violation could be
read after the fact instead of leaving only a counter. Those instruments are deliberately **not** in
the change that carries this entry: correcting shipped documentation should not wait on reviewing a
new test harness, so they follow separately. Three findings came out of them:

- **Contention is not the cause.** The idle run failed. That was the leading hypothesis and it was
  wrong.
- **Ring depth is not the cause.** 128, 256 and 512 were indistinguishable.
- **The violation is real, not an observation artifact.** Every one was confined to a single drain of
  the completion queue, so it is the order the kernel posted, not the order we happened to sample.

**Resolution.** The false assertion is removed; the counter behind it is kept and reported, so the
rate stays visible and a platform that later held the line would show up as a run of zeros. The
documentation that stated the guarantee -- `D-24`, the prose under "Durability on the ring", and the
rustdoc on `FlushCoverage::CoversPrecedingOperations` -- is corrected rather than quietly dropped,
since a caller may have relied on it.

**Worth keeping from this.** A rare failure in a test of a *platform guarantee* should be
characterised before it is stabilised. The two natural repairs -- loosen the assertion, or mark the
test serial -- would both have suppressed the only evidence that shipped documentation was wrong,
and "flaky test" and "the platform does not do what we wrote down" produce the same symptom.

## Resolved 2026-09-21 22:08:22 -04:00 -- the unidentified 	ests/bounded_pop.rs failure

**Recorded and resolved the same day, which is the honest framing:** it was recorded as an unresolved
failure on the grounds that fixing it did not belong in a push of finished milestones. That was a
scheduling preference dressed as a blocker. The mechanism and the fix were both understood at the time of
recording; nothing was actually blocking.

**The failure.** One `cargo test --all-features` run reported `FAILED: 4 passed; 1 failed` in a 2.12s
target matching [bounded_pop.rs](tests/bounded_pop.rs) by shape. The test name and panic message were not
captured. It never reproduced -- 15 isolated runs and 4 full-suite runs were green.

**The cause, and why no amount of re-running would have settled it.** Those tests needed an operation
still pending when a short bound expired, and got it from a 128 MiB unbuffered, overlapped read. That is
a *margin*, not a guarantee: `FILE_FLAG_NO_BUFFERING` bypasses the system cache but not the drive\'s own,
so the test was asking "will this device take longer than 5 ms?" -- a question about someone else\'s
hardware, whose answer may differ between two runs on the same machine.

**The fix: an operation that cannot complete, rather than one that is merely slow.** The read is now
issued against an **overlapped named pipe that nobody has written to**. It is pending because no byte
exists to satisfy it, and it completes exactly when the test writes one. There is no device, no cache and
no margin in the question.

Confirmed by probe before being adopted, since neither half was safe to assume: `IoRing` does accept a
pipe handle for `read_raw`, and `pop_within(20ms)` against an unwritten pipe returns `Ok(None)` with
`outstanding == 1`.

Where a delay is genuinely needed -- `run_down` polls in 50 ms steps, so forcing it to observe an expired
poll means releasing the read later than that -- it comes from a `thread::sleep`, whose guarantee runs the
safe way round: a sleep may overshoot, never undershoot. No assertion depends on an operation *finishing*
within any bound.

**Verified:** 25 consecutive runs of the target and 3 full `--all-features` suite runs, all green. Both
sabotages still bite exactly as before the rewrite -- reverting the timeout mapping turns all 5 red, and
making `RingWait::block` always fail turns 3 red -- so the rewrite kept every bit of the discriminating
power it had.

It is also **11x faster** (0.20s against 2.26s) and allocates no 128 MiB fixtures, which was the larger
part of what the file cost to run.

## Resolved 2026-09-25 14:19:35 -04:00 -- event_delivery stalled because the wait was armed after the event was signalled

`SetThreadpoolWait` documents that "you must re-register the event with the wait object before
signaling it each time to trigger the wait callback". `EventDelivery::new` did the reverse: it took an
already-signalled event from `IoRing::completion_event` and armed its wait afterwards.

Three properties compounded to make the dropped signal permanent rather than late. The event is
auto-reset, so the signal was consumed rather than left pending for the arming to observe. It is
edge-triggered on the completion queue going empty to non-empty, so a ring whose queue was already
non-empty was signalled by nothing else. And the setup signal exists to serve exactly that case, so it
was the only wakeup such a ring would ever get.

Fixed by attaching the event unsignalled and raising the setup signal after arming; see
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> D-68. Measured on the reproducer recorded below: 0 failures in
3600 runs after the change, against 5 in 600 and 2 in 600 for the two co-running triggers before it.

The investigation as it stood when the cause was found follows.

### event_delivery's threadpool tests time out at roughly one run in eighty

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

### Narrowed 2026-09-25: it requires parallel test execution, and a co-running create-and-drop

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
[`windows_threadpool_sys::wait::ThreadpoolWait`](../windows-threadpool-sys/src/wait.rs).

### Narrowed further 2026-09-25: the pool is alive, and the stall is permanent by design

A configurable trace was added for this (see below) and the flake **still reproduces with it on**,
which is the first thing to check for a timing-dependent fault.

**The default pool is not wedged.** A probe runs at the moment of failure, before anything else: it
submits a plain work item and separately creates, arms and signals a **brand-new** wait on a
brand-new event. Measured at a captured stall: `work item ran: true; a fresh wait ran: true`, both
within two seconds. So the pool dispatches, and its wait mechanism works. Whatever is broken is
specific to the waits already registered.

**Those waits were created and armed.** The trace shows, for every ring in a failing run:
`setup-signalled` -> `event-attached` -> `wait created` -> `wait armed`, all within microseconds --
and then `trampoline-entered` **never appears at all**, for any of them, for the rest of the process.

**The ring's setup signal is raised on the ring's own handle, before the wait is armed on a
duplicate of it.** The trace records both handle values, and in the captures examined the failing
ring's handles were not recycled values of the dropped ring's.

**Why the stall is permanent rather than merely late, which the trace explains.** The completion
event is edge triggered ([D-19](DESIGN-NOTES.md#d-19)): it fires when the queue goes from empty to
non-empty. A stalled ring has eight completions sitting in its queue, so the queue never returns to
empty and **no further signal will ever be raised**. The setup signal -- the one deliberate wakeup
that exists precisely to cover a backlog -- is therefore the only signal that ring will ever get.
Lose it once and delivery for that ring is dead for good. That is consistent with every capture:
zero callbacks, nothing after ten more seconds, and both rings affected together.

**So the open question is narrow: why does an armed wait not observe a signal raised before it was
armed?** An auto-reset event signalled with no waiter stays signalled, so arming afterwards should
consume it and fire. It does, on better than 99% of runs.

**What is deliberately not concluded.** A mechanism suggests itself -- the pool's internal wait
thread multiplexes handles, and a concurrent close could plausibly disturb the set it is watching,
which would fit a fresh wait working while existing ones do not. That is a hypothesis with no
evidence behind it yet, and it is recorded here as one so the next person does not mistake it for a
finding. The experiment that would settle it is whether re-arming a stalled wait recovers it;
`EventDelivery` does not currently expose its wait, so that needs either a test-only accessor or the
probe moved into `windows-threadpool-sys`.

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

### Turning the trace on, and narrowing it

The trace is **compiled out** unless the `trace` feature is on, because the instrument for a
timing-dependent fault must not change the schedule it is measuring. When on it is still off at run
time until `WINDOWS_THREADPOOL_TRACE` names the targets wanted, so a session can record one
subsystem rather than everything:

```powershell
$env:WINDOWS_THREADPOOL_TRACE = 'wait,delivery'   # or 'wait', or '*'
cargo test -p windows-ioring-sys --features trace --test event_delivery
```

Recording does not format and does not allocate: an entry is a timestamp, a thread id, two
`&'static str` labels and two `u64` slots, formatted only when a dump is asked for. The dump is
included in the stall report automatically, so a captured failure carries its own trace.

Targets currently emitted: `wait` (create, arm, trampoline entry, the three phases of drop) and
`delivery` (the ring's setup signal with its outstanding count, event attach, arm, callback entry
and exit).

**The flake still reproduces with the trace on**, which was checked before drawing anything from it.

### What a stalled run now records

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
