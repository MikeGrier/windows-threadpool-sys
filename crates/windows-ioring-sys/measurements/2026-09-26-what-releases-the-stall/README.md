# What releases the M26.9 delivery stall -- 2026-09-26

Nine captured stalls, three in each of three configurations, kept because they
answer a question that two earlier rounds of this investigation could only
guess at: the stall does not end on a timer and does not end on its own. It
ends when a work item is queued to the process thread pool.

Taken for `M26.13.3` with the fuller trace added by `M26.13.1`. Host: the
development machine; build: `cargo test -p windows-ioring-sys --all-features`
(which turns the `trace` feature on) at the commit that added `M26.13.2`.

## The question

`M26.13` recorded that dispatch resumes "at the five-second mark", microseconds
after the delivery test's own `DELIVERY_BOUND` expires, and that the obvious
suspect -- the pool-liveness probe the test runs on failure -- was not supported
cleanly, because the probe's own `wait created` record is stamped *after* the
first `trampoline-entered`. Three readings were open: a five-second timer
somewhere, the test thread waking, or something the probe does.

## The reproducer

The compiled `event_delivery` binary, run directly with a filter selecting the
two delivery tests and the co-running create-and-drop that `M26.9` narrowed the
trigger to:

```powershell
$env:WINDOWS_THREADPOOL_TRACE = '*'
& $exe completions_ dropping_with --nocapture
```

looped until it exits non-zero. Observed rates, which are in the same range as
the one in three hundred recorded before this trace existed:

| Configuration | Failures | Runs |
|---|---|---|
| as shipped | 3 | 1200 |
| quiet 2s | 3 | 1312 |
| quiet 2s and create/submit gap | 3 | 1070 (stopped at the third) |

## What was varied

Two temporary edits to [event_delivery.rs](../../tests/event_delivery.rs), each
reverted after its run. Neither changes the delivery path; both only delay what
the test does *after* it has already given up.

1. **quiet 2s** -- a `sleep(2s)` and a pair of trace records inserted as the
   first thing in the give-up branch of `recv_one`, before the `outstanding()`
   call and before the probe. Nothing touches the ring or the pool during it.
2. **create/submit gap** -- the above, plus a `sleep(1s)` in `pool_liveness`
   between `ThreadpoolWork::new` and `work.submit()`, so the work object exists
   for a full second before anything is queued on it.

## What was observed

In every one of the nine captures the last record before the gap is the third
ring's `delivery setup-signalled`, a couple of milliseconds into the run, and
the record immediately preceding the first `wait trampoline-entered` is
`work submitted`.

The table below is generated from the captures rather than transcribed from
them. Five of nine figures were wrong when it was first typed by hand, which is
why.

| Capture | Work submitted | First dispatch | Submit to dispatch |
|---|---|---|---|
| [as-shipped/stall-0022.txt](as-shipped/stall-0022.txt) | 5.011465s | 5.011808s | 343us |
| [as-shipped/stall-0452.txt](as-shipped/stall-0452.txt) | 5.006713s | 5.006999s | 286us |
| [as-shipped/stall-1021.txt](as-shipped/stall-1021.txt) | 5.009703s | 5.010024s | 321us |
| [quiet-2s/stall-0048.txt](quiet-2s/stall-0048.txt) | 7.006335s | 7.006632s | 297us |
| [quiet-2s/stall-0152.txt](quiet-2s/stall-0152.txt) | 7.017464s | 7.017758s | 294us |
| [quiet-2s/stall-1312.txt](quiet-2s/stall-1312.txt) | 7.006805s | 7.007078s | 273us |
| [quiet-2s-and-create-submit-gap/stall-0007.txt](quiet-2s-and-create-submit-gap/stall-0007.txt) | 8.011610s | 8.011896s | 286us |
| [quiet-2s-and-create-submit-gap/stall-1032.txt](quiet-2s-and-create-submit-gap/stall-1032.txt) | 8.006449s | 8.006753s | 304us |
| [quiet-2s-and-create-submit-gap/stall-1070.txt](quiet-2s-and-create-submit-gap/stall-1070.txt) | 8.016568s | 8.016875s | 307us |

Three things the columns say directly:

- **The stall moves with the probe, not with the clock.** Delaying the probe by
  two seconds delays the end of the stall by two seconds; delaying it by three
  delays it by three. The coincidence with `DELIVERY_BOUND` is a coincidence of
  *when the probe runs*.
- **Nothing releases it during the quiet period.** In the six captures that have
  one, no record of any kind appears between `quiet-begin` and `quiet-end`. The
  test thread waking from `recv_timeout`, on its own, is followed by two full
  seconds of the same silence that preceded it.
- **Creating the work object is not enough.** In the three captures with the
  create/submit gap, `work created` is followed by a full second of silence and
  dispatch follows `work submitted`.

Two further details, both visible in every capture:

- The probe's fresh wait is created *after* dispatch has already resumed, which
  is the observation `M26.13` recorded and could not place. It is the work half
  of the probe that precedes dispatch, not the wait half.
- Once dispatch resumes, everything queued runs: both rings' wait callbacks and
  both probe work items, in a few hundred microseconds, and the delivery tests
  then receive all eight completions each.

## What these captures do not say

They do not say why a queued wait callback is not dispatched until a work item
is submitted. Nothing here observes the pool's own thread accounting -- the
trace records what this workspace's wrappers do, and the gap between
`SetThreadpoolWait` returning and the trampoline being entered is inside the
pool, where these records cannot see. The next experiments are queued in
[CHECKLIST.md](../../CHECKLIST.md) under `M26.13`.