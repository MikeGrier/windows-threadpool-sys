# The kernel agrees: no thread is made -- 2026-09-27

An ETW kernel trace over 900 runs, confirming
[the-factory-never-makes-its-first-worker](../2026-09-27-the-factory-never-makes-its-first-worker/README.md)
from an instrument that shares nothing with it.

That finding came from inline hooks on `ntdll`. This one is the **kernel's own
thread-creation record**, collected by the NT Kernel Logger's `PROC_THREAD`
flag, and it needs no cooperation from `ntdll` or from this workspace's code.

## The result

Per-process figures are in [thread-creation.csv](thread-creation.csv).

| | first pool worker created, relative to the last test thread |
|---|---|
| **healthy**, 899 processes | **0.232 -- 17.928 ms**, mean 1.298 |
| **failing**, 1 process | **5008.736 ms** |

Of the 900 processes in the trace, **exactly one** has a gap of more than a
second anywhere in its thread activity, and it is the run that failed. The
slowest healthy process is 17.9 ms; the failing one is 280 times further out,
with nothing in between.

The two timelines, from [timelines.txt](timelines.txt):

```text
healthy                          failing
  +    0.000 ms   main             +    0.000 ms   main
  +   45.275 ms                    +   42.433 ms
  +   45.366 ms                    +   42.525 ms
  +   45.481 ms                    +   42.621 ms
  +  183.331 ms   test thread      +  200.320 ms   test thread
  +  183.391 ms   test thread      +  200.383 ms   test thread
  +  183.438 ms   test thread      +  200.425 ms   test thread
  +  183.807 ms   POOL WORKER      + 5209.161 ms   POOL WORKER
  +  187.175 ms   POOL WORKER      + 5209.361 ms   POOL WORKER
                                   + 5209.534 ms   POOL WORKER
```

A healthy run makes its first pool worker 0.376 ms after the last test thread
starts. The failing run makes none for 5.0087 s and then makes three inside
373 us -- which is consistent with the release path raising the pool's
running-thread goal on the way to `NtReleaseWorkerFactoryWorker`
([the-submit-is-what-releases-it](../2026-09-27-the-submit-is-what-releases-it/README.md)),
and is why the failing process is one of only three in the trace to reach ten
threads at all.

## Why a second instrument was worth the trouble

Everything previously known about the missing worker came from hooks this
workspace planted in `ntdll`. That is a good instrument and a
self-interested one: it reports on the very mechanism it modifies, and a
reader is entitled to ask whether the hooks are why the worker is missing.

They are not. The kernel logged the same absence, in the same runs, through a
path the hooks do not touch -- and it logged it for 899 other processes that
behaved normally while the hooks were installed in every one of them.

## What it still does not settle

The same thing as before: whether the wait-completion packet reached the I/O
completion port. `PROC_THREAD` records threads, not queues, and there is no
public event for a packet arriving at a port or for a worker factory being
activated -- checked against all 1198 registered providers, `xperf -providers
KF`, and the manifests of all 40 `Microsoft-Windows-Kernel-*` providers. So
"the factory was given the packet and did not act" and "the packet never
arrived" remain the two live readings.

## How it was run

```powershell
sudo xperf -on PROC_THREAD -f .scratch\etw\kernel.etl
# 900 runs of the reproducer, NOT elevated, exactly as every prior measurement
sudo xperf -stop -d .scratch\etw\merged.etl
xperf -symbols -i merged.etl -o dump.csv -a dumper
```

Elevation is required for the NT Kernel Logger and for nothing else: the
reproducer itself ran unprivileged, so the conditions match every earlier arm.
One failure in 900, which is the usual rate. The failing process was
identified by matching the thread ids in its own user-mode trace
([stalled-user-mode-trace.txt](stalled-user-mode-trace.txt)) against the
trace's process table; thread ids are recycled heavily across 900 short-lived
processes, so the match is the process holding *all seven* of them, not any
one of them.

`PROC_THREAD` alone was used. `DISPATCHER` would add `ReadyThread`, naming who
readied whom, and is the obvious next pass -- it was left out here to keep a
machine-wide trace small and short.
