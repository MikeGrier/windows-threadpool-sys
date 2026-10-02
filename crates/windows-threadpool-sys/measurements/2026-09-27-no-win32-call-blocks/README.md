# No Win32 call blocks during the stall -- 2026-09-27

707 bracketed Win32 calls measured across 24 captured stalls. The slowest
single call in the whole dataset is **220 microseconds**. Nothing blocks,
nothing contends, and the stall is not a call that failed to return.

## What was added since the last run

Three instruments, all present here at once:

- the pool-object lifecycle records (`M-T1.1`): creation, every arming or
  submission, and teardown, for every pool object kind;
- `syscall-enter` / `syscall-leave` (`M-T3.1`), bracketing every Win32 call
  that blocks or takes a pool lock, so a call that blocked is an interval
  rather than a late timestamp;
- the vectored exception observer (`M-T2.1`).

## Buffer headroom, checked first

The trace grew, so the buffer was re-measured before the run rather than after
-- a capture that silently truncated would have wasted it. This crate's own lib
tests, with every target on, went from 14061 records to **47997**: roughly
triple, and only a third of a buffer spare at the old capacity of 65536. Raised
to 262144.

The binary that actually captures is nowhere near either figure: **153 records**
per capture, and identically 153 in all 24.

## What was observed

24 failures in 4000 runs, which is in the range this configuration has produced
all day (13, 21, 14, 18 in 4000).

**No Win32 call blocked.** Every `syscall-enter` had its matching
`syscall-leave`, in all 24 captures -- there is no call that went in and did not
come out.

| Call | measured | slowest | median |
|---|---|---|---|
| `SubmitThreadpoolWork` | 48 | 220 us | 85 us |
| `CreateThreadpoolWait` | 120 | 105 us | 3 us |
| `CloseThreadpoolWork` | 48 | 39 us | 2 us |
| `CreateThreadpoolWork` | 48 | 39 us | 3 us |
| `WaitForThreadpoolWorkCallbacks` | 47 | 26 us | 1 us |
| `SetThreadpoolWait` | 191 | 19 us | 2 us |
| `WaitForThreadpoolWaitCallbacks(cancel)` | 63 | 11 us | 1 us |
| `CloseThreadpoolWait` | 72 | 9 us | 3 us |
| `SetThreadpoolWait(disarm)` | 70 | 2 us | 1 us |

**No exception was raised**, in any of the 24 -- consistent with
[2026-09-27-exceptions-during-the-stall](../2026-09-27-exceptions-during-the-stall/README.md).

**The arming completed promptly, and then nothing happened.** The last eight
records before the silence, from
[stall-0109.txt](stall-0109.txt):

```text
      0.002177s t1347632 syscall-enter          CreateThreadpoolWait                      0      0
      0.002179s t1347632 syscall-leave          CreateThreadpoolWait                      0      0
      0.002180s t1347632 wait                   created                              2305822795440    236
      0.002180s t1347632 syscall-enter          SetThreadpoolWait                    2305822795440    236
      0.002181s t1347632 syscall-leave          SetThreadpoolWait                    2305822795440    236
      0.002181s t1347632 wait                   armed                                2305822795440    236
      0.002182s t1347632 delivery               armed                                     0      0
      0.002183s t1347632 delivery               setup-signalled                         232      8
```

`CreateThreadpoolWait` returns in 2 microseconds and `SetThreadpoolWait` in 1,
the event is signalled with eight completions waiting -- and the next record of
any kind is 5.0105 seconds later, when the test gives up.

## What this rules out

The stall is not, and these are now measured rather than assumed:

- a Win32 call that blocked -- all 707 returned, slowest 220 us;
- a contended pool lock at arming time -- the arming pair is 1 microsecond;
- an exception raised and swallowed -- none was raised at all;
- a callback of ours holding a thread -- no trampoline is entered.

## One observation, offered as an observation

`SubmitThreadpoolWork` is the slowest call in the table by an order of
magnitude, at a median of 85 microseconds against 1 to 3 for everything else.
Every one of those 48 measurements is the post-mortem probe's submit -- the call
that `M26.13.3` established is what ends the stall, and that `M26.13.6` measured
gaining the process two or three threads. What that costs is consistent with
work being done inside it that the other calls do not do. It is not evidence of
what that work is.