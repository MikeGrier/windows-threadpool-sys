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

**Corrected 2026-09-26 by `M26.13`: this stall was not closed.** It still reproduces against the
current build at roughly one run in three hundred, with D-68's ordering in effect. An earlier
revision of this note said the fix "appears to have converted a permanent lost wakeup into a delayed
dispatch"; **that was wrong, and was corrected on 2026-09-27 by `M26.13.9`** -- the apparent delay
was an artifact of the test's own post-mortem probe, whose work submit releases the pool. With the
probe removed the delivery never arrives at all. The claim of 0 failures in 3600 runs below is
superseded; see [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md). The rest of this entry
is left as written, because its narrowing is still the best record of how the failure behaves.

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

## Resolved 2026-09-26 19:11:20 -04:00 -- the backlog was not a wakeup race; the signal was simply never raised

**Test:** `a_backlog_is_delivered_even_when_the_caller_attached_the_event_first` in
[tests/event_delivery.rs](tests/event_delivery.rs), no longer `#[ignore]`d.

**The recorded diagnosis was wrong.** The entry this replaces described a wakeup lost in a window
*after* arming, on the strength of two claims: that signalling unconditionally left the test failing
6 of 6, and that a 50 ms sleep between `wait.arm` and the signal made it pass 3 of 3. Re-measuring
contradicted both.

**What was measured.** A standalone experiment with no I/O ring in it -- create an auto-reset event,
`CreateThreadpoolWait`, `SetThreadpoolWait`, signal, wait for the callback -- lost **0 of 2000** in
arm-then-signal order, **0 of 2000** with the 50 ms pause, and **0 of 2000** in the signal-then-arm
order `SetThreadpoolWait` documents against. Against a real ring holding an eight-deep completion
backlog, with both earlier signals consumed: a manual `SetEvent` observed by a plain wait lost
**0 of 500** with no pool involved, and a pool wait armed exactly as `EventDelivery::new` arms one,
then signalled immediately, lost **0 of 500**. With the signal raised unconditionally the reproducer
passed **30 of 30**, and the whole `event_delivery` suite passed five times over. Moving that signal
to *before* the arm also passed 6 of 6, so the ordering the prior entry turned on makes no
difference to this failure.

**The actual cause** is the narrow one Copilot review reported on PR #108.
`attach_completion_event_unsignalled` reported the setup signal "still owed" only when that call had
performed the attachment, so a caller who attached the event earlier, consumed the signal attaching
raised, submitted, and consumed the signal the completions raised handed over a **non-empty** queue
with no wakeup pending -- and [D-19](DESIGN-NOTES.md#d-19)'s edge cannot re-arm without the queue
first returning to empty. The failure was deterministic, and the reproducer's own report said as
much before any of this: `callbacks run: 0` is a signal never raised, not one raised and lost.

**Fixed** by removing the flag: attaching and signalling remain two steps so a caller can arm in
between, but `IoRing::completion_event` and `EventDelivery::new` both raise the setup signal
unconditionally. Recorded as [D-77](DESIGN-NOTES.md#d-77), which also corrects the one clause of
[D-68](DESIGN-NOTES.md#d-68) that the measurements above contradict. Guarded by two sabotage cases
and by `a_repeat_call_signals_again_once_the_earlier_signal_has_been_consumed`, which is the only
test that consumes both earlier signals and so the only one able to observe the ring-level half at
all -- verified by sabotage to be the single test that fails when the old behaviour is restored.

**How the earlier measurement probably went wrong, offered as reconstruction rather than finding.**
The same trap was hit during this investigation: restoring a file with `Copy-Item` preserves its
mtime, cargo then judged the crate unchanged, skipped the rebuild, and a test was run against a
*stale binary* built from the patched source. It was caught only because the result was impossible
-- a test passing that could not pass. A stale binary would produce exactly the reported "the
obvious repair does not work", and would also make an unrelated pause look like the active
ingredient.

**Still open:** `M26.9`'s original intermittent stall was real and its fix is retained, but the
mechanism D-68 offered for it does not survive these measurements, so that mechanism is once again
unexplained.
## Resolved 2026-10-01 21:47:17 -04:00 -- the M26.9 delivery stall, closed by the draining teardown

**What closed it.** `windows-threadpool-sys` stopped cancelling at teardown and drained instead
(`WaitForThreadpoolWaitCallbacks` with `fCancelPendingCallbacks` FALSE), decided as [Teardown drains
rather than cancels](../../DESIGN-NOTES.md#teardown-drains) and implemented as `M-T4`. The entry
below is the record as it stood while the failure was live, kept verbatim.

**Measured twice, on two different builds.**
[2026-09-29-the-fix-on-the-real-path](measurements/2026-09-29-the-fix-on-the-real-path/README.md)
took it on the real `EventDelivery` path against the `M-T4` build: 33 failures in 10000 reverted
runs, 0 in 70000 drained.
[2026-10-01-the-fix-still-holds-after-the-self-heal](measurements/2026-10-01-the-fix-still-holds-after-the-self-heal/README.md)
re-took it (`M26.15`) after `M-T6` added the default-on self-heal and `EventDelivery` gained its own
draining `Drop`: control 12 in 4000, current 0 in 12000.

**This file has carried a fix for this stall once before, and that one was wrong.** The
2026-09-25 entry above attributed it to arm-before-signal ordering (`D-68`); `M26.13` later
reproduced the stall against that build and overturned it. Two things are different this time, and
they are the reason for recording the distinction rather than simply claiming more confidence: the
entry condition was isolated to a specific teardown by decomposition rather than inferred, and each
measurement ran a control on the same tree that still failed, so a zero could be told apart from a
reproducer that had stopped reproducing.

**Still not a root cause.** Why closing a wait handle behind its own disarm leaves the pool without
a worker is unexplained, and no instrument tried has been able to see it -- see
[STALL-TIMELINE.md](STALL-TIMELINE.md) and the parked `M26.14.4`. What changed is that this crate no
longer takes the path that reaches it.
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

**The pool has idle workers while stalled, and does not dispatch to them.** *Overturned 2026-09-27 by `M26.13.17`: the parked workers belong to a second worker factory, and the pool under test has none. The paragraph below is kept as the record of how the mistake was made -- a stack shows `TppWorkerThread`, which names the worker routine every pool shares, not the factory it serves.* Corrected 2026-09-27 by `M26.13.11`: a full dump taken while stalled shows **three** threads parked in `ntdll!ZwWaitForWorkViaWorkerFactory` under `ntdll!TppWorkerThread`. An earlier reading of this entry said the pool had *no* worker and created one on the work submit; the thread count was right and the inference was wrong. [measurements/2026-09-27-the-workers-are-there/](measurements/2026-09-27-the-workers-are-there/README.md). The count evidence below stands as counts. `M26.13.6` counted
the process's threads in the post-mortem: 6 while stalled, 8 or 9 immediately after the work submit,
in every capture. Two readings it rules out rather than supports: this workspace's own callbacks are
not occupying the threads -- across 27 captures, **zero** trampolines are entered during the stall,
so none is inside a closure -- and marking the delivery callbacks with
`SetThreadpoolCallbackRunsLong` does not prevent it (12 failures in 4000, against a control that has
measured 13 and 21 in two separate 4000-run measurements, so no effect is claimed).
[measurements/2026-09-27-the-pool-has-no-worker/](measurements/2026-09-27-the-pool-has-no-worker/README.md).

**Tests:** `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting` and
`completions_queued_before_handover_are_still_delivered` in
[event_delivery.rs](tests/event_delivery.rs). Both are always *stalled* together, and it happens
only in parallel with a co-running test that creates an `EventDelivery` and drops it promptly.

**They usually both *report* failure, but not always, and the exception is instructive.** An earlier
version of this line said "never singly", which is wrong: across the 80 captures committed under
[measurements/](measurements/), 79 report both victims and
[one reports a single victim](measurements/2026-09-27-the-pool-never-starts/stall-3888.txt) while
the other test passes. That capture's own trace shows why, and it is not a second phenomenon. The
pool was dead for the full five seconds there too -- the first callback of any kind is at
5.003027s. What differs is only the race at the end: the two victims' deadlines are a fraction of a
millisecond apart, the first to expire runs the post-mortem probe, the probe's work submit releases
the pool about 0.3ms later, and the second victim's deliveries arrived at 5.003056s, inside its own
deadline. So whether the second victim reports depends on whether its remaining margin exceeds the
release latency, which is one more way the diagnostic probe repairs the fault it is measuring.

**An annotated, record-by-record walk through one captured occurrence is in
[STALL-TIMELINE.md](STALL-TIMELINE.md)**, including a reference table mapping every trace event to
the Win32 call behind it. Read that first if you are coming to this cold.

**Rate**, using that record's own reproducer against the current build: 2 failures in 600 runs and 2
in 900. The prior entry recorded 0 in 3600 after D-68's fix; that no longer holds. Re-measured on
2026-09-26 with the fuller trace, in three configurations: 3 in 1200, 3 in 1312, and 3 in 1070; and
on 2026-09-27 across five more, from 3 in 525 to 3 in 2105.

**D-68's ordering is in effect and does not prevent it.** The trace of a captured failure shows
`wait armed` at 0.002286s and `setup-signalled` at 0.002288s -- arm first, then signal, exactly as
D-68 requires.

**It is a permanent hang, not a delayed dispatch.** Corrected 2026-09-27 by `M26.13.9`, which
falsified this entry's own earlier claim. The "delay" was an artifact of the instrument: the
post-mortem's pool-liveness probe submits work, that submit is what releases the pool, and only
*then* did the delivery arrive -- so every capture showed data arriving and the stall looked late
rather than lost. With the probe removed and the post-mortem extended to sixty seconds, the
delivery **never arrives**: `callbacks run: 0`, no trampoline is entered, and the trace holds
nothing between 0.002s and 65.02s in 3 of 3 captures.
[measurements/2026-09-27-it-never-self-releases/](measurements/2026-09-27-it-never-self-releases/README.md).

So `M26.9`'s original signature -- a permanent lost wakeup -- was right, and D-68 did not convert it
into anything milder. What D-68 changed, if anything, is not established here.

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

**No Win32 call blocks during it either.** `M26.13.7` bracketed every Win32 call in
[windows-threadpool-sys](../windows-threadpool-sys/src/trace.rs) that blocks or takes a pool lock,
and measured 707 of them across 24 captured stalls: every one returned, and the slowest in the whole
dataset is 220us. The arming pair that precedes the silence -- `CreateThreadpoolWait` then
`SetThreadpoolWait` -- takes 2us and 1us respectively.
[measurements/2026-09-27-no-win32-call-blocks/](measurements/2026-09-27-no-win32-call-blocks/README.md).

**A timer armed while the pool was healthy also stops.** `M26.13.12` armed a self-rearming four-second timer at process start, due a full second before the test's deadline. In 10 of 10 captures it did not fire when due and fired only when the work submit released the pool, about a second late -- which is also its own positive control, since it does fire. It involves no event, no handle, no ring and no wait, and it was registered before anything went wrong, so the fault is in dispatch rather than in registration.
[measurements/2026-09-27-a-timer-armed-while-healthy-also-stops/](measurements/2026-09-27-a-timer-armed-while-healthy-also-stops/README.md).

**In a stalled run the pool dispatches nothing at all, from process start.** `M26.13.13` shortened the standing heartbeat to 100ms, making it a clock. Its first expiry at 0.1s is missed, about 50 consecutive expiries are missed, and across 18 captures **not one** pool callback of any kind -- wait, timer, work or I/O -- is dispatched before the release. So this is not a pool that runs and then wedges; the five seconds is a period during which it never starts.
[measurements/2026-09-27-the-pool-never-starts/](measurements/2026-09-27-the-pool-never-starts/README.md).

**Nothing whatsoever happens in the process during the window.** `M26.13.14` shortened the heartbeat again to 15.625ms -- the default Windows tick, the finest period reachable without `timeBeginPeriod` changing the machine's timer behaviour under the measurement. It misses 320 consecutive expiries, and in 13 of 13 captures the last setup record (`delivery setup-signalled`, about 2.5ms) and the first post-mortem record (about 5.01s) are **adjacent lines**. The trace brackets every Win32 call and carries a vectored exception handler, so the window contains no callback, no syscall and no exception. The heartbeat is also armed *before* the trigger's first record, which brackets the onset to the first 15.7ms.
[measurements/2026-09-27-at-the-system-tick-it-still-never-starts/](measurements/2026-09-27-at-the-system-tick-it-still-never-starts/README.md).

**The submit alone is what releases it, and the backlog was already queued.** `M26.13.15` moved only the `SubmitThreadpoolWork` call, by inserting a sleep between creating the work object and submitting it: 0, 250, 500, 1000 and 2000ms, 4000 runs each, 99 captures. Delivery-minus-submit stays at 0.25 to 0.54ms in every arm while delivery-minus-create tracks the delay across a 2000ms span. In 99 of 99 the first callback dispatched anywhere in the process is the stalled delivery's **wait**, served 29 to 67us *ahead of* the work item whose submit woke the worker -- so the wait callback was queued and unserved, not unnoticed.
[measurements/2026-09-27-the-submit-is-what-releases-it/](measurements/2026-09-27-the-submit-is-what-releases-it/README.md).

**Correction to the mechanism, and what the open question now is.** This record previously called `SubmitThreadpoolWork` "a user-mode queue push", contrasted against three kernel-delivered paths. That was an unverified label and is wrong in the half that matters. Disassembling all 189 `ntdll!Tp*`/`Tpp*` functions shows `TppWorkPost` pushes in user mode **and then calls `NtReleaseWorkerFactoryWorker`** -- one of only four functions in the whole thread pool that does, and none of the four is on the wait, timer or I/O path. Those three register for kernel delivery (`NtCreateWaitCompletionPacket`, `NtAssociateWaitCompletionPacket`, `NtSetTimer2`) and rely on the factory releasing a worker by itself when a packet arrives. So the open question is not "user mode versus kernel mode" but: **the worker factory holds parked workers and a queued packet and does not put them together until something explicitly asks it to.** Why is inside the factory, where this workspace's trace cannot reach.

**The parked workers are never used, and a passing run uses them.** `M26.13.16` snapshotted the process's thread set at the stall and tested the serving thread for membership. In 12 of 12 captures **no thread that was alive at the stall ever runs a callback**: the backlog is served by one or two threads created after the fact. The same snapshot taken in a passing run, before any completion can arrive, finds the same six threads -- and the delivery is served by one that already existed, in 30 of 30. So `M26.13.11`'s literal claim survives (the threads are there, parked) but the inference drawn from it does not: a free worker sat available for the whole five seconds, in the same state as the one that serves the delivery in a passing run, and the pool dispatched only once a *new* thread existed. What this cannot separate is whether the new thread was created because the parked ones were unusable or merely as a side effect of the submit's `TppAdjustRunningThreadGoalWithLock`.
[measurements/2026-09-27-the-parked-workers-are-never-used/](measurements/2026-09-27-the-parked-workers-are-never-used/README.md).

**The default pool has no worker at all, and the parked three were never its own.** `M26.13.17` read `NtQueryInformationWorkerFactory` for **every** worker factory in the process, at the stalled moment and before the probe submits. There are two. The default pool (`ThreadMaximum` 768) reports `TotalWorkerCount` **0** while stalled in 12 of 12, against **1** while healthy in 20 of 20. The other (`ThreadMaximum` 3) holds exactly three workers, all waiting, and is identical in both arms. **This overturns `M26.13.11`:** `TppWorkerThread` is the worker routine for every pool in a process, so the dump's three parked stacks could never say which factory they served, and they answer to the second one. The pool under test has no worker -- which is what `M26.13.6` said from thread counts and the dump was taken to disprove. It also explains `M26.13.16` rather than leaving it strange: the parked threads were never candidates. While stalled the factory reports `Paused` false, `Shutdown` false, `MayCreate` **true**, `ThreadMinimum` 0 and `LastThreadCreationStatus` 0, so nothing has told it to stop and nothing has failed.
[measurements/2026-09-27-the-default-pool-has-no-worker-at-all/](measurements/2026-09-27-the-default-pool-has-no-worker-at-all/README.md).

**The pool's first worker is never created.** `M26.13.18` moved the hook install into a `.CRT$XCU` static initialiser, so it runs before `main` and before the harness has a thread -- which also makes it free, since there is nothing to suspend. With the hooks in place from process start: in a healthy run the kernel makes a worker **0.24 to 0.31ms** after the delivery is armed, which announces itself with `NtWorkerFactoryWorkerReady`, parks, and is handed the packet at once. In a stalled run there is **no `ready` and no `park` on that factory at all** before the release, in 8 of 8. So the stall is not a worker that fails to wake or a callback that runs and goes missing: the thread to run it is never made. The same run refutes the `AlreadySignaled` race that motivated hooking the wait registration -- `NtAssociateWaitCompletionPacket` reports the flag **false** for every association made before the first callback, in both arms.
[measurements/2026-09-27-the-factory-never-makes-its-first-worker/](measurements/2026-09-27-the-factory-never-makes-its-first-worker/README.md).

**The kernel's own record agrees, from an instrument the hooks do not touch.** `M26.13.19` ran an ETW kernel trace (`PROC_THREAD`) over 900 runs. Of the 900 processes, **exactly one** has a gap of more than a second anywhere in its thread activity, and it is the run that failed: its first pool worker is created **5008.736ms** after the last test thread, against a healthy range of **0.232 to 17.928ms** (mean 1.298) across the other 899. The failing process then makes three workers inside 373us, which is why it is one of only three in the trace to reach ten threads. This matters because every previous statement about the missing worker came from hooks this workspace planted in `ntdll` -- an instrument reporting on the mechanism it modifies. The kernel logged the same absence independently, in the same runs, while those hooks were installed in all 900.
[measurements/2026-09-27-the-kernel-agrees-no-thread-is-made/](measurements/2026-09-27-the-kernel-agrees-no-thread-is-made/README.md).

**The trigger's *teardown* is what poisons the process, not its setup.** `M26.14.1` made the trigger selectable and ran five arms of 4000. Built and deliberately **never dropped**, it produces **0** failures against a live control's 11 in the same session. A ring created and dropped with no `EventDelivery` over it also produces **0**. So it takes the delivery *and* taking it back down: a `TP_WAIT` armed on the ring's completion event and then disarmed, drained and closed 32us later -- before the pool has dispatched anything, and about 2.4ms before either victim arms. Four cycles instead of one give 26 rather than 11, so it is not a once-per-process event the first cycle exhausts; that arm also lengthens the trigger, so it is a direction rather than a dose-response.
[measurements/2026-09-28-the-teardown-is-what-poisons/](measurements/2026-09-28-the-teardown-is-what-poisons/README.md).

**Closing the wait too soon after disarming it is the poison, and two teardowns prevent it.** `M26.14.2` finished the decomposition with three arms interleaved at 20000 runs each, timing balanced to 1.3%. Against a control's **10** failures, a **1ms gap between the disarm and the close** gives **0**, and replacing the cancelling drain with a **true** drain (`WaitForThreadpoolWaitCallbacks` with `fCancelPendingCallbacks` FALSE rather than TRUE) also gives **0**; each zero has probability 4.5e-5 under the control's rate. The disarm alone is innocent (0 in 15000 cumulative). The control that makes the sleep meaningful is `hand-sleep-after`, which does the identical sleep *after* the teardown and still fails -- so the gap must sit between those two calls specifically, not merely delay the trigger. **This is a workaround with a mechanism-shaped hint, not a diagnosis**: a sleep that makes a race disappear is evidence of a race, not an explanation, and no threshold is established.
[measurements/2026-09-28-closing-too-soon-after-the-disarm/](measurements/2026-09-28-closing-too-soon-after-the-disarm/README.md).

**Not established:** why the pool will create a worker for a submitted work item but not for a wait,
timer, or I/O callback that is already queued. That is the whole of what is left, and it is inside
the pool, where this workspace's trace cannot reach. Four more readings were ruled out on
2026-09-27 -- a blocked or contending Win32 call, an exception raised and swallowed, our own
callbacks holding threads, and a thread minimum -- so what remains is narrow rather than open. The
next experiments are queued in [CHECKLIST.md](CHECKLIST.md) under `M26.13`.

**Why it matters beyond these two tests.** The sabotage harness runs the whole suite once per case,
and a suite that fails for this reason is recorded as the case being `caught`. That is the dangerous
direction: a sabotage the tests did not really catch reads as a clean bill of health.
