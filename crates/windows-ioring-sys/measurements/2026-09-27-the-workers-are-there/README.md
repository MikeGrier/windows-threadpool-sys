# The pool's workers are there, parked and idle -- 2026-09-27

A full process dump taken **while stalled**, before anything poked the pool.
It shows three thread-pool worker threads alive and waiting for work.

This **corrects** `M26.13.6`, which read a rising thread count as "the pool has
no worker and makes one when work is submitted". The workers were there all
along.

## How it was captured

The test's post-mortem spawns a debugger against its own process at the moment
of failure, before the liveness probe runs -- so the state captured is the
faulted one, not the repaired one:

```text
q:\dbg\cdb.exe -pvr -p <pid> -c ".dump /ma <path>; qd"
```

`-pv` attaches **non-invasively**, which injects no thread into the target;
`r` resumes its threads; `qd` quits and detaches, leaving the process running,
so the run continues normally and the control probe still happens afterwards.
Confirmed: the process survived and produced its usual report. The dump was
taken once per process, at 5.006s, and took 7.2 seconds to write.

## What the dump shows

Six threads. Three of them are the pool:

```text
   1  Id: 18c6e0.18c734
      ntdll!ZwWaitForWorkViaWorkerFactory+0x14
      ntdll!TppWorkerThread+0x37e
      kernel32!BaseThreadInitThunk+0x17
      ntdll!RtlUserThreadStart+0x2c

   2  Id: 18c6e0.18c708   (identical)
   3  Id: 18c6e0.18c724   (identical)
```

The other three are the main thread and the two victim test threads, which the
dump helpfully shows by name -- one parked in `WaitOnAddress` (the mpsc
channel), one in `WaitForMultipleObjects`.

Full stacks in [stalled-process-stacks.txt](stalled-process-stacks.txt); the
matching trace capture, showing `dump-begin` at 5.006s and `dump-end` at
12.160s, is in [stall-capture.txt](stall-capture.txt). The dump itself is 33 MB
and is not committed.

`!handle 0 2 TpWorkerFactory` reports two worker factories in the process.

## What it corrects

`M26.13.6` measured the process holding 6 threads while stalled and 8 or 9
immediately after the work submit, and read that as the pool having no worker
and creating one. The count was right; **the reading was wrong**. Three of those
six are pool workers, and they are parked in
`ZwWaitForWorkViaWorkerFactory` -- the kernel's "give me work" wait. They are
idle and available.

So the pool is not starved of threads. It has idle workers, our wait is armed,
its event can be signalled successfully
([2026-09-27-not-a-missed-wake](../2026-09-27-not-a-missed-wake/README.md)), and
no callback is handed to any of them.

## What it does not establish

Why. Two things are now known to be true at the same time -- idle workers exist,
and queued callbacks are not dispatched to them -- and nothing here explains the
gap between them.

One observation worth carrying, stated as an observation: of the four ways into
this pool, the three that fail during the stall (a wait's event signalling, a
timer expiring, an I/O completing) are all delivered **by the kernel** into the
worker factory, while the one that works, `SubmitThreadpoolWork`, is a
user-mode queue push. Whether that distinction is the mechanism is not
established by this dump, and the dump cannot settle it: it captures state, not
the delivery path.

> **Correction, 2026-09-27.** The last sentence of that paragraph stands, but
> "user-mode queue push" is wrong in the half that matters and was never
> verified when written. `SubmitThreadpoolWork` (`TppWorkPost`) pushes in user
> mode *and then* calls `NtReleaseWorkerFactoryWorker` -- and it is one of only
> four functions in all of `ntdll`'s thread pool that does. The three failing
> paths call none of them. So the distinction is not user mode against kernel
> mode; it is that the work path is the only one that **explicitly asks the
> factory to release a worker**. See
> [the-submit-is-what-releases-it](../2026-09-27-the-submit-is-what-releases-it/README.md).