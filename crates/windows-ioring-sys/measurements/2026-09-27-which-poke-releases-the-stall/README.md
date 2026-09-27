# Which pool poke releases the M26.9 delivery stall -- 2026-09-27

Fifteen captured stalls, three in each of five configurations. They answer
`M26.13`'s experiment 3 -- whether any pool poke releases the stall or only a
work submit -- and in answering it they move the fault out of this crate
entirely.

Taken with the trace from `M26.13.1`, the buffer headroom from `M26.13.2`, and
the pool-object lifecycle records from
[windows-threadpool-sys](../../../windows-threadpool-sys/COMPLETED-CHECKLIST.md#m-t11)'s
`M-T1.1`, which this experiment could not have been read without.

## The question

`M26.13.3` established that the stall ends when a work item is queued to the
pool and at no other time: not on a timer, not on its own, and not when the
work object is merely created. That left open whether a work submit is special
or whether any pool activity would do.

## Method

One temporary edit to [event_delivery.rs](../../tests/event_delivery.rs),
reverted afterwards. In the post-mortem -- which runs only on a failing run,
after the test has already given up -- both failing threads perform, in step:

```text
quiet 2s      baseline: confirms the stall is still live and silent
poke          exactly one, selected by the M26_POKE environment variable
observe 2s    did anything dispatch?
control       a work submit, known from M26.13.3 to release the stall
observe 1s    it must appear here if it did not appear above
```

The control matters as much as the poke: without it, a poke that did nothing
and a stall that had already ended would look identical.

`delivery callback-entered` is the decisive record, because only
`EventDelivery`'s own callback emits it -- it cannot be confused with the
poke's objects dispatching. The poke's objects are deliberately held alive
until after the control, since dropping a pool object is itself a poke and
`ThreadpoolIo`'s drop would block forever on a pool that is not dispatching.

Observed rates, all three-failure runs: none 3 in 525, wait 3 in 1307, timer 3
in 612, io 3 in 2105, work 3 in 859.

## What was observed

The table is generated from the captures rather than transcribed from them.

| Poke | Capture | Poked at | Control at | Poke's own callback ran? | Delivery released? |
|---|---|---|---|---|---|
| none | [none/stall-0308.txt](none/stall-0308.txt) | 7.0165s | 9.0168s | n/a | no, not until 9.0171s |
| none | [none/stall-0380.txt](none/stall-0380.txt) | 7.0161s | 9.0164s | n/a | no, not until 9.0168s |
| none | [none/stall-0525.txt](none/stall-0525.txt) | 7.0146s | 9.0149s | n/a | no, not until 9.0152s |
| wait | [wait/stall-0276.txt](wait/stall-0276.txt) | 7.0095s | 9.0096s | no, not until 9.0100s | no, not until 9.0100s |
| wait | [wait/stall-0470.txt](wait/stall-0470.txt) | 7.0130s | 9.0133s | no, not until 9.0136s | no, not until 9.0136s |
| wait | [wait/stall-1307.txt](wait/stall-1307.txt) | 7.0143s | 9.0144s | no, not until 9.0147s | no, not until 9.0148s |
| timer | [timer/stall-0524.txt](timer/stall-0524.txt) | 7.0166s | 9.0167s | no, not until 9.0171s | no, not until 9.0171s |
| timer | [timer/stall-0543.txt](timer/stall-0543.txt) | 7.0135s | 9.0136s | no, not until 9.0140s | no, not until 9.0139s |
| timer | [timer/stall-0612.txt](timer/stall-0612.txt) | 7.0135s | 9.0139s | no, not until 9.0143s | no, not until 9.0142s |
| io | [io/stall-0589.txt](io/stall-0589.txt) | 7.0179s | 9.0184s | no, not until 9.0187s | no, not until 9.0187s |
| io | [io/stall-1212.txt](io/stall-1212.txt) | 7.0101s | 9.0103s | no, not until 9.0106s | no, not until 9.0106s |
| io | [io/stall-2105.txt](io/stall-2105.txt) | 7.0060s | 9.0061s | no, not until 9.0064s | no, not until 9.0064s |
| work | [work/stall-0007.txt](work/stall-0007.txt) | 7.0176s | 9.0183s | yes, 7.0180s | **yes**, 7.0179s |
| work | [work/stall-0302.txt](work/stall-0302.txt) | 7.0133s | 9.0142s | yes, 7.0136s | **yes**, 7.0136s |
| work | [work/stall-0859.txt](work/stall-0859.txt) | 7.0073s | 9.0078s | yes, 7.0076s | **yes**, 7.0076s |

Three readings, and the second is the one that moved the investigation.

- **Only a work submit releases it.** A fresh wait armed and signalled, a fresh
  timer due in a millisecond, and a real overlapped read that completed all
  leave the delivery stalled for the full two-second observe window. The work
  submit releases it within microseconds. Fifteen captures, no exceptions.
- **The poke's own callback does not run either.** This is not a property of
  the ring's wait, or of waits. A brand-new wait, a brand-new timer, and a
  brand-new I/O object -- created and established *during* the stall, with no
  connection to the ring -- each sit undispatched for two seconds and then run
  only after the work submit. Whatever is stalled is the pool's dispatch of
  every callback kind, not anything belonging to this crate.
- **The poke was established, not merely requested**, and the trace says so
  directly: `timer created` and `timer armed` at 7.016s against
  `timer trampoline-entered` at 9.017s; `io created` and `io started` at
  7.017s against `io trampoline-entered` at 9.018s; `wait created` and
  `wait armed` at 7.009s against its trampoline at 9.010s. Those `created` and
  `armed` records are exactly what `M-T1.1` added. Without them this
  experiment would have shown only late trampolines, which is equally
  consistent with a poke that never armed.

One further detail, visible in every capture of every configuration: when
dispatch resumes, **one** pool thread runs everything queued, in order --
the delivery waits first, which had been queued since about two milliseconds
into the run, then the work items.

## What these captures do not say

They do not say why the pool stops dispatching, or why a work submit is the
one thing that restarts it. Nothing here observes the pool's own thread
accounting: the trace records what this workspace's wrappers do, and the
interval between an object being armed and its trampoline being entered is
inside the pool, where these records cannot reach. That is
[CHECKLIST.md](../../CHECKLIST.md) -> `M26.13`'s experiment 1.