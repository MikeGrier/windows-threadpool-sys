# The stall, record by record

A single captured occurrence of the `M26.9` delivery stall, annotated. It exists
because the raw captures are forty kilobytes of repeated transcript each and the
findings are spread across four measurement directories -- neither is something
you can read back and forth against.

**Every record below is copied verbatim from one capture**, not retyped:
[stall-0189.txt](measurements/2026-09-27-the-pool-has-no-worker/every-failure/stall-0189.txt),
taken 2026-09-27 with `WINDOWS_THREADPOOL_TRACE` set to everything and the
post-mortem thread counts in place. It is representative rather than special: at
the time of writing the signature it shows had appeared in 14 of 14 failures
examined without capping.

The trace facility, its targets and its filter are documented on
[windows-threadpool-sys](../windows-threadpool-sys/src/trace.rs)'s `trace`
module.

## How to read a row

Every row has the same six fields, and none of them is labelled in the dump:

```text
      5.323957s t1253864 wait                   trampoline-entered           1783077454640      0
      \_______/ \______/ \__________________/   \______________________/     \___________/  \___/
          1         2            3                          4                      5          6
```

| # | Field | Meaning |
|---|---|---|
| 1 | elapsed | seconds since the trace's own clock started, which is the first time anything was recorded |
| 2 | thread | `GetCurrentThreadId` of whoever recorded it |
| 3 | target | the subsystem: `wait`, `work`, `io`, `timer`, `timer-periodic`, `exception` from the pool crate; `delivery` from this crate; `postmortem` and `experiment` from the test |
| 4 | event | what happened |
| 5 | slot a | first payload, **meaning depends on the event** |
| 6 | slot b | second payload, likewise; `0` where the event carries only one value or none |

The two payload slots are plain `u64`s with no per-event labelling, which is
what keeps recording free of formatting and allocation. The price is that they
can only be decoded by looking the event up in the table at the bottom of this
document.

### Where a callback begins and ends

Two nested pairs, and telling them apart matters:

- **`<target> trampoline-entered` / `trampoline-left`** bracket the **pool
  callback** -- the `extern "system"` function the thread pool itself calls.
  Between them, a pool thread is inside our code.
- **`delivery callback-entered` / `callback-left`** bracket the **body of
  `EventDelivery`'s closure**, which sits *inside* the wait's trampoline pair.
  Between them, the ring is being drained.

So a normal delivery nests like this, all on one pool thread:

```text
wait trampoline-entered        <- the pool entered our callback
  delivery callback-entered    <- our closure began
    ... drain, re-arm, drain ...
  delivery callback-left
wait trampoline-left           <- the pool callback returned
```

An entry with no matching exit means a callback is still running or blocked. No
entry at all means the pool never dispatched. Those are different faults, which
is why both ends are recorded.

### The two different `armed` events

They are easy to confuse and mean different things:

- **`wait armed`** is the **Win32 call**: `SetThreadpoolWait` has just been
  issued. It is recorded at the one place that call is made, so it covers the
  first arming, the cleanup-group arming, and every re-arm. Slot a is the
  `PTP_WAIT`, slot b the handle now being watched.
- **`delivery armed`** is a **milestone in `EventDelivery::new`**: construction
  has finished its arming step. It carries no payload. It follows `wait armed`
  by a microsecond or two and tells you *who* did the arming, not that a second
  arming happened.

The same shape applies to `timer armed`, which is `SetThreadpoolTimer`, with
slot b carrying the period in milliseconds -- zero for a one-shot.

## The cast

Three `EventDelivery` objects exist in this process, each over its own ring, all
on the **default process thread pool**. The reproducer runs three tests in
parallel:

| Thread | Test | Role |
|---|---|---|
| t1253532 | `dropping_with_nothing_outstanding_does_not_hang` | the **trigger**: builds a delivery over an empty ring and drops it immediately |
| t1253540 | `completions_queued_before_handover_are_still_delivered` | a **victim** |
| t1253536 | `completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting` | a **victim** |

t1253864 and t1253860 appear later. They are pool threads, and they do not exist
-- or at least never run anything -- until the very end.

## Phase 0 -- the trigger builds a delivery and tears it straight back down

```text
      0.000000s t1253532 delivery               event-attached                  240      0
      0.000020s t1253532 wait                   created                      1783077420592    240
      0.000022s t1253532 wait                   armed                        1783077420592    240
      0.000023s t1253532 delivery               armed                             0      0
      0.000041s t1253532 delivery               setup-signalled                 236      0
      0.000042s t1253532 wait                   drop-begin                   1783077420592    240
      0.000043s t1253532 wait                   suppress-and-disarm          1783077420592      1
      0.000045s t1253532 wait                   disarmed                     1783077420592      0
      0.000046s t1253532 wait                   drop-drained                 1783077420592      0
      0.000054s t1253532 wait                   drop-closed                  1783077420592    240
```

`CreateThreadpoolWait`, `SetThreadpoolWait`, `SetEvent`, then the whole teardown
sequence, inside 54 microseconds. Note that the handle the wait watches (240) is
**not** the handle the setup signal is raised on (236): the ring keeps its own
handle and hands out a duplicate, which is [D-20](DESIGN-NOTES.md#d-20), and the
signal must go to the ring's own or the kernel stops signalling it
([D-77](DESIGN-NOTES.md#d-77)).

Nothing here is wrong. This is the sequence `M26.9` identified as the trigger,
and it is still only a correlation -- see `M26.13`'s remaining experiment 2.

## Phase 1 -- the two victims arm their waits and signal

```text
      0.002249s t1253540 delivery               event-attached                  260      0
      0.002271s t1253540 wait                   created                      1783077418208    260
      0.002273s t1253540 delivery               armed                             0      0
      0.002273s t1253540 wait                   armed                        1783077418208    260
      0.002274s t1253540 delivery               setup-signalled                 256      0
      0.002347s t1253536 delivery               event-attached                  240      0
      0.002352s t1253536 wait                   created                      1783077454640    240
      0.002353s t1253536 wait                   armed                        1783077454640    240
      0.002353s t1253536 delivery               armed                             0      0
      0.002355s t1253536 delivery               setup-signalled                 232      8
```

Both follow the order [D-68](DESIGN-NOTES.md#d-68) requires: `wait armed`
precedes `delivery setup-signalled`, by microseconds. The last one carries
`8` in its second slot -- eight completions already queued in that ring,
waiting for the callback that is about to not happen.

**This is the last record for five seconds.**

## Phase 2 -- nothing

Between 0.002355s and 5.014184s the trace is empty. Not "quiet": empty. No
trampoline is entered, so no callback of ours is running or blocked; no wait is
armed or disarmed; nothing is created or closed.

That emptiness is the finding, and it is what the entry/exit pairing exists to
make legible -- an entry with no exit would mean a callback was stuck inside a
closure, and there is no entry at all.

## Phase 3 -- the test threads give up, and the probe queues work

```text
      5.014184s t1253540 postmortem             delivery-wait-expired             0      0
      5.014184s t1253536 postmortem             delivery-wait-expired             0      0
      5.320013s t1253540 experiment             threads-at-stall                  6      0
      5.320016s t1253540 postmortem             outstanding-begin                 0      0
      5.320019s t1253540 postmortem             outstanding-read                  8      0
      5.320020s t1253540 postmortem             probe-begin                       0      0
      5.320032s t1253540 work                   created                      1783077444592      0
      5.320142s t1253540 work                   submitted                    1783077444592      0
      5.322985s t1253536 experiment             threads-at-stall                  6      0
      5.322986s t1253536 postmortem             outstanding-begin                 0      0
      5.322988s t1253536 postmortem             outstanding-read                  8      0
      5.322988s t1253536 postmortem             probe-begin                       0      0
      5.323000s t1253536 work                   created                      1783077375616      0
      5.323001s t1253536 work                   submitted                    1783077375616      0
```

Read this one carefully, because the order matters and it is the heart of the
diagnosis:

1. `delivery-wait-expired` -- `recv_timeout(5s)` returned `Err` on both
   victims. The 5 seconds is the **test's own bound**, not a pool timer.
2. `threads-at-stall 6` -- the process holds six threads.
3. `outstanding-read 8` -- the ring still has its eight completions, and the
   ring mutex was free, so nothing was holding it.
4. `work created` then `work submitted` -- `CreateThreadpoolWork` and
   `SubmitThreadpoolWork`, from the liveness probe.

Dispatch resumes within about a millisecond of the **second** of those two
submits, and under four of the first -- all three timestamps are in the block
above. Delaying the probe delays the resumption by exactly as much, which is
what makes the five-second figure a coincidence of when the probe runs rather
than a timer
([2026-09-26-what-releases-the-stall](measurements/2026-09-26-what-releases-the-stall/README.md)).

## Phase 4 -- one pool thread appears and drains everything

```text
      5.323957s t1253864 wait                   trampoline-entered           1783077454640      0
      5.323970s t1253864 delivery               callback-entered                  0      0
      5.324017s t1253864 wait                   rearm-entered                1783077454640      0
      5.324021s t1253864 wait                   armed                        1783077454640    240
      5.324022s t1253864 wait                   rearm-left                   1783077454640    240
      5.324023s t1253864 delivery               callback-left                     0      0
      5.324023s t1253864 wait                   trampoline-left              1783077454640      0
      5.324025s t1253864 work                   trampoline-entered           1783077444592      0
      5.324060s t1253864 work                   trampoline-left              1783077444592      0
      5.324061s t1253864 work                   trampoline-entered           1783077375616      0
      5.324065s t1253540 postmortem             work-probe-answered               1      0
      5.324066s t1253540 work                   drop-begin                   1783077444592      0
      5.324067s t1253540 work                   drop-drained                 1783077444592      0
```

t1253864 has never appeared before. It runs a victim's wait callback, which
drains the ring, re-arms, and drains again; then it runs **both** probe work
items. A second pool thread, t1253860, appears immediately after and takes the
other victim.

The order is the point: the wait callbacks queued at 2.3 milliseconds run
**before** the work items queued at 5.32 seconds. They were waiting for a
thread, not for an event.

## Phase 5 -- the probe finishes, and the counts confirm it

```text
      5.324090s t1253540 wait                   created                      1783077471872    184
      5.324092s t1253540 wait                   armed                        1783077471872    184
      5.324097s t1253860 wait                   trampoline-entered           1783077418208      0
      5.324097s t1253864 work                   trampoline-left              1783077375616      0
      5.324100s t1253860 delivery               callback-entered                  0      0
      5.324101s t1253864 wait                   trampoline-entered           1783077471872      0
      5.324103s t1253864 wait                   trampoline-left              1783077471872      0
      5.324109s t1253540 postmortem             wait-probe-signalled            184      0
      5.324111s t1253540 postmortem             wait-probe-answered               1      0
      5.324118s t1253540 wait                   drop-begin                   1783077471872    184
      5.324119s t1253540 wait                   suppress-and-disarm          1783077471872      1
      5.324119s t1253540 wait                   disarmed                     1783077471872      0
      5.324121s t1253540 wait                   drop-drained                 1783077471872      0
      5.324129s t1253536 postmortem             work-probe-answered               1      0
      5.324132s t1253536 work                   drop-begin                   1783077375616      0
      5.324134s t1253540 wait                   drop-closed                  1783077471872    184
      5.324134s t1253536 work                   drop-drained                 1783077375616      0
      5.324141s t1253860 wait                   rearm-entered                1783077418208      0
      5.324147s t1253536 work                   drop-closed                  1783077375616      0
      5.324155s t1253860 wait                   armed                        1783077418208    260
      5.324156s t1253860 wait                   rearm-left                   1783077418208    260
      5.324157s t1253860 delivery               callback-left                     0      0
      5.324157s t1253860 wait                   trampoline-left              1783077418208      0
      5.324160s t1253540 postmortem             probe-left                        1      1
      5.324166s t1253864 wait                   trampoline-entered           1783077418208      0
      5.324167s t1253864 delivery               callback-entered                  0      0
      5.324167s t1253536 wait                   created                      1783077471216    268
      5.324170s t1253864 wait                   rearm-entered                1783077418208      0
      5.324172s t1253536 wait                   armed                        1783077471216    268
      5.324173s t1253864 wait                   armed                        1783077418208    260
      5.324173s t1253864 wait                   rearm-left                   1783077418208    260
      5.324174s t1253864 delivery               callback-left                     0      0
      5.324174s t1253864 wait                   trampoline-left              1783077418208      0
      5.324197s t1253536 postmortem             wait-probe-signalled            268      0
```

`wait-probe-answered 1` and `work-probe-answered 1`: both probes ran. The
second thread count comes a little later in the capture, once the probe has
finished:

```text
      5.598768s t1253540 experiment             threads-after-poke                8      0
      5.602153s t1253536 experiment             threads-after-poke                8      0
```

Eight, where there had been six. The pool made workers when work was submitted
to it, and not before.

## What the timeline rules out

Each of these was measured, not inferred from this capture alone; the
measurement is linked.

| Reading | Status |
|---|---|
| a five-second timer somewhere | ruled out -- the stall moves with the probe, not the clock ([what-releases-the-stall](measurements/2026-09-26-what-releases-the-stall/README.md)) |
| the test thread waking ends it | ruled out -- a 2s quiet period before the probe contains nothing |
| creating a work object ends it | ruled out -- a 1s gap between `new` and `submit` passes in silence |
| any non-work poke ends it | ruled out -- a wait, a timer and an I/O completion all fail to ([which-poke](measurements/2026-09-27-which-poke-releases-the-stall/README.md)) |
| it is specific to waits, or to the ring's wait | ruled out -- those pokes' **own** callbacks do not run either |
| a thread minimum prevents it | ruled out -- the no-minimum private arm is already clean ([private-pool](measurements/2026-09-27-private-pool-does-not-stall/README.md)) |
| our callbacks occupy the pool's threads | ruled out -- zero trampolines entered during the stall, in 27 captures |
| announcing them `runs_long` prevents it | ruled out -- no measurable effect ([the-pool-has-no-worker](measurements/2026-09-27-the-pool-has-no-worker/README.md)) |
| an exception is being raised and swallowed during the window | ruled out -- a vectored exception handler saw **none**, in 18 captures ([exceptions-during-the-stall](measurements/2026-09-27-exceptions-during-the-stall/README.md)) |
| a Win32 call is blocked, or contending on a pool lock | ruled out -- 707 bracketed calls across 24 captures all returned, slowest 220us ([no-win32-call-blocks](measurements/2026-09-27-no-win32-call-blocks/README.md)) |
| the reproducer's own 5000ms `submit_and_wait` timeout is the five seconds | ruled out -- changed to 4000ms, and dispatch still resumed at five seconds in 14 of 14 ([the-submit-timeout-is-not-it](measurements/2026-09-27-the-submit-timeout-is-not-it/README.md)) |
| a wakeup was missed, as in D-19/D-68/D-77 | ruled out -- re-signalling the very event the wait is armed on releases nothing, 5 of 5, with each SetEvent's success recorded ([not-a-missed-wake](measurements/2026-09-27-not-a-missed-wake/README.md)) |
| the pool is starved of threads | **not ruled out after all** -- the dump's three parked workers belong to a *second* worker factory (`ThreadMaximum` 3, identical in passing runs). The pool under test reports `TotalWorkerCount` **0** while stalled against **1** while healthy, in 12 and 20 captures ([the-default-pool-has-no-worker-at-all](measurements/2026-09-27-the-default-pool-has-no-worker-at-all/README.md)) |
| the parked workers wake and serve the backlog | ruled out -- **no thread alive at the stall ever runs a callback**, in 12 of 12; a passing run serves the delivery on a pre-existing thread in 30 of 30 ([the-parked-workers-are-never-used](measurements/2026-09-27-the-parked-workers-are-never-used/README.md)). They were never candidates: they are the other factory's |
| the factory has been paused, shut down, or forbidden to create | ruled out -- while stalled it reports `Paused` false, `Shutdown` false, `MayCreate` **true**, `ThreadMinimum` 0, `LastThreadCreationStatus` 0 |
| the fault is in *registering* with the pool, not dispatching | ruled out -- a timer armed while the pool was healthy, due inside the stall window, fires only at the release, 10 of 10 ([a-timer-armed-while-healthy-also-stops](measurements/2026-09-27-a-timer-armed-while-healthy-also-stops/README.md)) |
| the pool runs for a while and then wedges | ruled out -- with a 100ms heartbeat, **no** pool callback of any kind is dispatched before the release, in 18 of 18: it never starts ([the-pool-never-starts](measurements/2026-09-27-the-pool-never-starts/README.md)) |
| *anything at all* happens in the process during the window | ruled out -- at the 15.625ms system tick the last setup record and the first post-mortem record are **adjacent lines**: no callback, no syscall, no exception, in 13 of 13 ([at-the-system-tick](measurements/2026-09-27-at-the-system-tick-it-still-never-starts/README.md)) |
| the onset is somewhere late in the five seconds | ruled out -- bracketed to the first 15.7ms: a timer armed at 0.00004s, before the trigger's first record, is due at 15.7ms and never fires |
| something other than the submit in the probe releases it | ruled out -- delaying only the `SubmitThreadpoolWork` call by 0 / 250 / 500 / 1000 / 2000ms moves the delivery with it: delivery-minus-submit stays at 0.25-0.54ms across all 99 captures ([the-submit-is-what-releases-it](measurements/2026-09-27-the-submit-is-what-releases-it/README.md)) |
| the stalled callback is unnoticed rather than queued | ruled out -- the released worker serves the five-second-old **wait** 29-67us *ahead of* the work item whose submit woke it, in 99 of 99 |
| `SubmitThreadpoolWork` differs by being user-mode | ruled out -- it makes a syscall too; what is unique is that `TppWorkPost` calls `NtReleaseWorkerFactoryWorker`, which 185 of the 189 `ntdll` thread-pool functions and all three failing paths do not ([ntdll-census.txt](measurements/2026-09-27-the-submit-is-what-releases-it/ntdll-census.txt)) |
| it happens off the default process pool | never observed -- 0 in 12000 runs across three private-pool arms |

**What is left is one question**, now stated in the pool's own terms: the
default pool's worker factory holds **no worker at all** while stalled, is not
paused, is not shut down, reports `MayCreate` true and no failed creation, and
owes a queued wait-completion packet -- and it does not make a worker for it
until `NtReleaseWorkerFactoryWorker` asks. Only `TppWorkPost` and three
siblings call that release, and none of the wait, timer or I/O paths do: they
register for kernel delivery and rely on the factory acting by itself. Why it
does not is the open question, and it is inside the factory, where this
workspace's trace cannot reach. Queued as M26.13 experiment 1 in
[CHECKLIST.md](CHECKLIST.md).

**And one inference boundary, twice corrected.** That the process gains threads
when dispatch resumes is measured. It was first read as "the six it holds while
stalled contain no idle pool worker", then contradicted by a dump showing three
parked in `ZwWaitForWorkViaWorkerFactory`, and the counters now say the first
reading was right about *this pool*: the parked three answer to a different
worker factory, and `TppWorkerThread` on a stack cannot distinguish them. The
thread growth has a mechanism rather than a mystery: `TppWorkPost` calls
`TppAdjustRunningThreadGoalWithLock` on the way to releasing a worker, so the
submit raises the pool's running-thread goal as a side effect of being a submit.

The lesson is worth more than the correction. A parked `TppWorkerThread` says a
pool has a worker; it does not say **which** pool, and in a process with more
than one factory that distinction decides the answer. Count workers by asking
each factory, not by reading stacks.
That says where the threads come from, not why the idle ones were not used.

## Reference: every event the trace can emit, and the API behind it

| Target | Event | Slot a | Slot b | Win32 call, or what it brackets |
|---|---|---|---|---|
| `wait` | `created` | `PTP_WAIT` | handle | `CreateThreadpoolWait` returned |
| `wait` | `armed` | `PTP_WAIT` | handle | `SetThreadpoolWait(wait, handle, timeout)` |
| `wait` | `disarmed` | `PTP_WAIT` | -- | `SetThreadpoolWait(wait, NULL, NULL)` |
| `wait` | `trampoline-entered` / `-left` | `PTP_WAIT` | wait result | the `PTP_WAIT_CALLBACK` body |
| `wait` | `rearm-entered` | `PTP_WAIT` | -- | before the suppression lock, which can block |
| `wait` | `rearm-left` | `PTP_WAIT` | handle | after the re-arming `SetThreadpoolWait` |
| `wait` | `rearm-suppressed` | `PTP_WAIT` | count | teardown refused the re-arm |
| `wait` | `suppress-and-disarm` | `PTP_WAIT` | count | suppression raised, then disarmed, under one lock |
| `wait` | `drop-begin` / `-drained` / `-closed` | `PTP_WAIT` | handle (ends) | `WaitForThreadpoolWaitCallbacks`, then `CloseThreadpoolWait` |
| `work` | `created` | `PTP_WORK` | -- | `CreateThreadpoolWork` returned |
| `work` | `submitted` | `PTP_WORK` | -- | `SubmitThreadpoolWork` |
| `work` | `trampoline-entered` / `-left` | `PTP_WORK` | -- | the `PTP_WORK_CALLBACK` body |
| `work` | `drop-begin` / `-drained` / `-closed` | `PTP_WORK` | -- | `WaitForThreadpoolWorkCallbacks`, then `CloseThreadpoolWork` |
| `timer` | `created` | `PTP_TIMER` | -- | `CreateThreadpoolTimer` returned |
| `timer` | `armed` | `PTP_TIMER` | period ms | `SetThreadpoolTimer`; period 0 means one-shot |
| `timer` | `disarmed` | `PTP_TIMER` | -- | `SetThreadpoolTimer(timer, NULL, 0, 0)` |
| `timer` | `trampoline-entered` / `-left` | `PTP_TIMER` | -- | the `PTP_TIMER_CALLBACK` body |
| `timer` | `rearm-requested` | `PTP_TIMER` | delay ms | the callback asked; applied after it returns |
| `timer` | `rearm-entered` / `-left` / `-suppressed` | `PTP_TIMER` | -- | the trampoline applying that request |
| `timer` | `drop-*` | `PTP_TIMER` | -- | as for `wait` |
| `timer-periodic` | `created` | `PTP_TIMER` | period ms | `CreateThreadpoolTimer` returned |
| `timer-periodic` | `trampoline-entered` / `-left` | `PTP_TIMER` | -- | the tick callback body |
| `timer-periodic` | `drop-*` | `PTP_TIMER` | -- | stop, then drain, then `CloseThreadpoolTimer` |
| `io` | `created` | `PTP_IO` | handle | `CreateThreadpoolIo` returned |
| `io` | `started` | `PTP_IO` | `OVERLAPPED` | `StartThreadpoolIo`, before the operation is issued |
| `io` | `start-cancelled` | `PTP_IO` | `OVERLAPPED` | `CancelThreadpoolIo`: no callback will arrive |
| `io` | `trampoline-entered` / `-left` | `OVERLAPPED` | io result | the `PTP_WIN32_IO_CALLBACK` body |
| `io` | `rundown-begin` / `-ended` | `PTP_IO` | outstanding | `run_down` |
| `io` | `drop-*` | `PTP_IO` | handle (ends) | drain, `CloseThreadpoolIo`, free |
| `delivery` | `event-attached` | event handle | -- | the duplicate the ring handed back |
| `delivery` | `armed` | -- | -- | `EventDelivery::new` finished arming |
| `delivery` | `setup-signalled` | event handle | outstanding | `SetEvent` on the ring's **own** handle |
| `delivery` | `callback-entered` / `-left` | -- | -- | the drain-rearm-drain body |
| `postmortem` | `delivery-wait-expired` | -- | -- | the test's `recv_timeout` returned `Err` |
| `postmortem` | `outstanding-begin` / `-read` | -- / count | -- | taking the ring mutex, and the answer |
| `postmortem` | `probe-begin` / `probe-left` | -- / work ran | -- / wait ran | the liveness probe |
| `postmortem` | `work-probe-answered` | 1 or 0 | -- | its work item reported in |
| `postmortem` | `wait-probe-signalled` | event handle | -- | `SetEvent` on a fresh probe event |
| `postmortem` | `wait-probe-answered` | 1 or 0 | -- | its wait reported in |
| `postmortem` | `second-wait-begin` / `-ended` | -- / microseconds | -- | the post-mortem's further wait |
| `experiment` | `threads-at-stall` / `-after-poke` | thread count | -- | a Toolhelp snapshot, post-mortem only |
| `exception` | `raised` | exception code | raise address | a first-chance exception, seen by a vectored handler that declines to handle it |
| `syscall-enter` / `syscall-leave` | the Win32 function's own name | the object | a discriminating argument | the call boundary -- see below |

### `syscall-enter` / `syscall-leave`

A separate target, off unless asked for, that brackets **every** Win32 call in
`windows-threadpool-sys` that blocks or touches the pool's own synchronisation:
the `WaitForThreadpool*Callbacks` family, `CloseThreadpoolCleanupGroupMembers`,
`CloseThreadpool`, `SetThreadpoolThreadMinimum` (which creates threads), every
`Create*`, `Set*`, `Submit*`, `Close*`, `Start*` and `Cancel*` on a pool object,
`CancelIoEx`, and the caller-supplied wait close routine.

It exists because a single record stamped *after* a call returns cannot tell
"this was issued late" from "this took four seconds to return". Which of these
contend is not documented, so they are bracketed rather than assumed cheap.

Narrow to it with `WINDOWS_THREADPOOL_TRACE='syscall'`, which matches both
halves. A capture narrowed to anything else is unchanged by its existing.

The exception code is the raw `NTSTATUS`, printed in decimal like every other
slot. Two values seen so far: `1073807366` is `0x40010006`
(`DBG_PRINTEXCEPTION_C`, what `OutputDebugStringA` raises), and `3765269347` is
`0xE06D7363`, the C++ exception code that a Rust panic uses on MSVC targets.
Slot b is `ExceptionRecord->ExceptionAddress` -- the instruction that raised.

**The observer is live from `0.000000s` by construction**, because installing it
is what starts the trace's clock. So the absence of `exception raised` rows in a
capture means no exception was raised in the traced run, not that the observer
was late.

Several of those appear in no capture here because the reproducer builds no
timer and no `TP_IO`; they are listed so a future capture of something else
can be read with the same table.

## Reproducing it

```powershell
cargo test -p windows-ioring-sys --all-features --test event_delivery --no-run
$env:WINDOWS_THREADPOOL_TRACE = '*'
# loop the compiled binary; roughly one run in 250 fails on the machine this
# was measured on
& $exe completions_ dropping_with --nocapture
```

The trace prints only on a failure, as part of the stall report. Rates, and the
conditions the failure needs, are in
[UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md).