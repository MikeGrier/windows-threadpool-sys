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

**Not established:** why the pool will create a worker for a submitted work item but not for a wait,
timer, or I/O callback that is already queued. That is the whole of what is left, and it is inside
the pool, where this workspace's trace cannot reach. Four more readings were ruled out on
2026-09-27 -- a blocked or contending Win32 call, an exception raised and swallowed, our own
callbacks holding threads, and a thread minimum -- so what remains is narrow rather than open. The
next experiments are queued in [CHECKLIST.md](CHECKLIST.md) under `M26.13`.

**Why it matters beyond these two tests.** The sabotage harness runs the whole suite once per case,
and a suite that fails for this reason is recorded as the case being `caught`. That is the dangerous
direction: a sabotage the tests did not really catch reads as a clean bill of health.