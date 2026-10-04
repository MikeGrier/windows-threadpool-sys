# Are exceptions being raised during the stall? -- 2026-09-27

No. A vectored exception handler installed for the whole traced run saw **no
exception at all** during the stall window, in 18 of 18 captures.

## Why it was worth asking

A first-chance exception that some later handler swallows is invisible to
almost everything: it does not appear in a debugger's default view, it leaves
no trace in a log, and it can cost real time. The stall window is five seconds
in which this workspace's own trace records nothing whatsoever, so "something
is happening that we cannot see" is exactly the shape of hypothesis worth
testing, and a vectored handler is the cheapest way to test it.

## The instrument

`AddVectoredExceptionHandler(CALL_FIRST, ...)`, installed by
[windows-threadpool-sys](../../../windows-threadpool-sys/src/trace.rs)'s trace
module the moment the trace turns on. Each exception is recorded as
`exception raised` with the `NTSTATUS` code in the first payload slot and
`ExceptionRecord->ExceptionAddress` -- the instruction that raised -- in the
second.

The handler returns `EXCEPTION_CONTINUE_SEARCH`, so dispatch proceeds exactly
as it would have. Three properties make it safe to run inside a trace that a
callback may already be holding the lock of: every `OnceLock` is read rather
than initialised, the buffer lock is taken with `try_lock`, and the
overflow-to-stderr announcement is suppressed. Each of those costs a dropped
record rather than a deadlock, which is the right trade for an observer whose
purpose is to change nothing.

**Installing it is what starts the trace's clock**, so `0.000000s` is by
construction the moment the observer went live. The absence of rows therefore
means no exception was raised, not that the observer was late.

## What was observed

| | |
|---|---|
| failures | 18 in 4000 runs |
| captures containing **no** exception record | 17 |
| captures containing one | 1 |
| exception records inside the stall window | **0**, in all 18 |

The single record is in [stall-2130.txt](stall-2130.txt):

```text
      5.009137s t1324556 postmortem             second-wait-begin                 0      0
      5.009144s t1324556 postmortem             second-wait-ended                 8      0
      5.009784s t1324560 postmortem             second-wait-begin                 0      0
      5.009785s t1324556 exception              raised                       3765269347 140716655329738
      5.009790s t1324560 postmortem             second-wait-ended                 7      0
```

`3765269347` is `0xE06D7363`, the C++ exception code a Rust panic uses on MSVC
targets, and it is raised at 5.0098s -- **after** that thread's delivery had
already arrived at 5.0091s. It is one failing test thread's own panic, captured
in the other thread's dump because the two were racing. That also explains why
it appears in one capture and not eighteen: every failure panics, but the panic
normally happens after its own dump has been formatted, so it lands in the
transcript only when the other thread is still writing.

[stall-0681.txt](stall-0681.txt) and [stall-3988.txt](stall-3988.txt) are kept
as examples of the other seventeen, which contain no exception record at all.

## What follows

The stall window is not merely free of this workspace's callbacks -- it is free
of exceptions. Nothing is being raised and swallowed in it.

Two further notes:

- **The instrument does not perturb the measurement.** 18 failures in 4000 runs
  sits in the range the same configuration has already produced without it (13,
  21 and 14 in 4000). That is worth stating because a thread-count snapshot on
  the setup path *did* perturb it badly, recorded in
  [the-pool-has-no-worker](../2026-09-27-the-pool-has-no-worker/README.md).
- **It is a negative result, and negative results from an unexercised
  instrument are worthless**, so the handler carries its own guard:
  `the_exception_observer_notes_a_first_chance_exception` raises a real
  first-chance exception through `OutputDebugStringA` and asserts it was
  recorded. It caught a real defect while being written -- the handler recorded
  nothing at all until the installer was changed to bring the clock and the
  buffer into existence itself, because the handler deliberately refuses to
  initialise them.