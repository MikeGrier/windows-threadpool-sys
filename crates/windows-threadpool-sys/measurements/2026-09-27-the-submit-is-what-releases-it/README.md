# The submit is what releases it, and the backlog was already queued -- 2026-09-27

Two questions, answered together.

1. **What specifically restarts delivery?** Earlier runs established that a work
   submit releases the pool and that no other poke does, but the probe that
   submits also creates a work object, reads the ring's count, and writes to
   stderr first. That correlation had never been reduced to one call.
2. **Is `SubmitThreadpoolWork` "a user-mode queue push"?** That phrase was
   written into the record on 2026-09-26 as an unverified label. It is wrong in
   the half that matters, and this corrects it.

## 1. The delay series

A configurable sleep was inserted into the post-mortem probe **between
`CreateThreadpoolWork` and `SubmitThreadpoolWork`**, so the submit moves and
nothing else does. Five arms of 4000 runs each, delay in {0, 250, 500, 1000,
2000} ms, producing **99 captures**. Per-capture figures are in
[correlation.csv](correlation.csv).

| delay | captures | delivery minus **submit** | delivery minus **create** |
|---|---|---|---|
| 0 ms | 17 | 0.25 -- 0.54 ms | 0.27 -- 0.56 ms |
| 250 ms | 19 | 0.25 -- 0.39 ms | 250.37 -- 250.94 ms |
| 500 ms | 23 | 0.25 -- 2.82 ms | 500.43 -- 503.46 ms |
| 1000 ms | 16 | 0.25 -- 0.36 ms | 1000.43 -- 1001.15 ms |
| 2000 ms | 24 | 0.25 -- 0.39 ms | 2000.33 -- 2001.12 ms |

The right-hand column tracks the delay across a 2000 ms span. The middle column
does not move: the delivery lands a third of a millisecond after the submit
whether the submit happens immediately or two seconds later.

`delivery` here is the `wait trampoline-entered` record of a wait that was armed
in the first few milliseconds of the process -- the stalled `EventDelivery`
itself, not a probe object. `submit` is the `syscall-enter SubmitThreadpoolWork`
bracket, so it is the instant of the call rather than the probe's surrounding
bookkeeping.

The negative half was already measured and is not repeated here: a work object
created and left unsubmitted for a second releases nothing
([what-releases-the-stall](../2026-09-26-what-releases-the-stall/README.md)),
and with no submit at all the delivery never arrives inside sixty seconds
([it-never-self-releases](../2026-09-27-it-never-self-releases/README.md)).

## 2. The backlog was already queued, and the wait is served first

In **99 of 99** captures the first callback dispatched anywhere in the process
is the delivery's **wait**, not the work item whose submit caused the release --
the work trampoline follows it by 29 to 67 us (mean 39). Ordering figures are in
[ordering.csv](ordering.csv). One released worker then drains everything, in 91
of 99 captures across two threads and in 8 across one:

```text
7.013410 t1963976   syscall-enter    SubmitThreadpoolWork
7.013410 t1963972   syscall-enter    SubmitThreadpoolWork
7.013419 t1963972   work             submitted
7.013503 t1963976   work             submitted
7.013785 t1962108   wait             trampoline-entered      <- the stalled delivery
7.013787 t1962108   delivery         callback-entered
7.013818 t1962108   wait             trampoline-left
7.013819 t1962108   work             trampoline-entered      <- the probe's own item
7.013838 t1962108   work             trampoline-entered
7.013857 t1962108   wait             trampoline-entered      <- the other delivery
```

A worker that had to be told to wake up, then finds the five-second-old wait
callback ahead of the work item that woke it, is a worker taking the front of an
existing queue. This is the discriminator M26.13 experiment 1 asked for: the
stalled callback is **queued and unserved**, not unnoticed.

**The limit.** The trace cannot see the completion port, so this shows the wait
is *ordered ahead of* the work, not the instant the kernel enqueued it. A
consistent 39 us lead in 99 of 99 is a queue-order fact rather than a coin flip,
but "queued when the event was signalled" remains the reading rather than the
observation.

## 3. The correction: `SubmitThreadpoolWork` makes a syscall, and it is the interesting one

`ntdll.dll 10.0.26100.9278`, every `Tp*`/`Tpp*` function disassembled and
grepped -- the census is in [ntdll-census.txt](ntdll-census.txt).

`SubmitThreadpoolWork` is `TpPostWork` -> `TppWorkPost`, which does a user-mode
queue push under the pool's SRW lock and then calls, in order,
`TppAdjustRunningThreadGoalWithLock`, `NtAlertThreadByThreadId`, and
**`NtReleaseWorkerFactoryWorker`**.

Of the 189 thread-pool functions in `ntdll`, exactly four call
`NtReleaseWorkerFactoryWorker`: `TppWorkPost`, `TpPostTask`,
`TppPrepareDirectParams`, and `TppWorkCallbackPrologRelease`. The wait, timer
and I/O paths call none of them. What they do instead is register for kernel
delivery -- `TpSetWaitEx` and `TppSetupNextWait` associate a wait-completion
packet, the timer queue associates one and arms `NtSetTimer2` -- after which the
kernel posts to the worker factory's port. `TppWorkerThread`, meanwhile, parks
in `NtWaitForWorkViaWorkerFactory`, which is exactly where the dump in
[the-workers-are-there](../2026-09-27-the-workers-are-there/README.md) found
three threads.

So the distinction is not user mode against kernel mode; the submit path makes a
syscall too. It is that **the work path is the only one of the four that
explicitly asks the worker factory to release a worker.** The three that fail
rely on the factory releasing one by itself when a packet arrives.

`TppAdjustRunningThreadGoalWithLock` on the same path also accounts for an
observation that had no explanation: the process gaining threads at the release
while three workers sat idle.

**This is static evidence about a code path, not a measurement of this fault.**
It says what each API does; it does not say why the factory failed to release a
worker for a packet it had. Nothing here establishes the cause.

## How it was run

The three reproducer tests in
[tests/event_delivery.rs](../../../windows-ioring-sys/tests/event_delivery.rs) were left untouched;
`pool_liveness()` was temporarily given a sleep, governed by
`IORING_STALL_SUBMIT_DELAY_MS`, between creating the work object and submitting
it. The binary was looped 4000 times per arm with `WINDOWS_THREADPOOL_TRACE='*'`
and every non-zero exit captured. The patch is not committed; the two sample
captures, the two CSVs and the census are the artifact. The census was taken
with `cdb -z C:\Windows\System32\ntdll.dll`.
