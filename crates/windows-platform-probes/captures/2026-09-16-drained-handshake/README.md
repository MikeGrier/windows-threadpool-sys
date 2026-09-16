# Queue contention, drained regime, after the M4.3 readiness handshake

Three whole-probe invocations taken to answer one question: does closing the
undrained opening at the start of each drained run change what the drained rows
say?

**This is a second capture, not a replacement for the first.** The figures in
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) that predate `M4.3` measured a probe
whose producers could begin pushing before the consumer had run at all. These
measured a probe where no producer begins timing until the consumer has executed
its pop path at least once. That is the guarantee, stated exactly: continuous
draining is not guaranteed and no flag could express it, since the consumer can
be descheduled afterwards as it can at any point in the run. Both are real
measurements of two different pieces of code, so they are kept side by side and
each is labelled with the instrument that produced it.

## Attribution

| | |
|---|---|
| Host | `x86_64 16p/8c smt+ L2[2,2,2,2,2,2,2,2] ec[0:16] numa[16]` |
| Profile | release |
| Sampling | 50,000 pushes per producer, median of 5 repetitions, one untimed warmup pass |
| Runs | 3 whole-probe invocations, in [run1.txt](run1.txt), [run2.txt](run2.txt), [run3.txt](run3.txt) |
| Instrument | `probe-queue-contention`, built from `68198359` (the commit that made the consumer drain once before announcing readiness) |
| Taken | 2026-09-16 18:12 UTC-04:00 |

The host is the same machine as the capture the crate README carries, so the two
are comparable; nothing here says anything about any other hardware, and the
banner's `numa[16]` is a single node holding all sixteen processors.

## Reading it

[summary.txt](summary.txt) is the output of [summarise.js](summarise.js) over the
three runs, regenerated with:

```
node summarise.js run1.txt run2.txt run3.txt
```

It is committed so the derivation can be checked rather than taken on trust, and
so nothing downstream has to retype a figure. The script derives two things the
runs do not state individually: the across-run median per producer count, and
the same-code control span -- which is a relation *between* two tables, since
`reserving_mpsc` in the comparison table and `32/32` in the layout table are the
same configuration measured twice in the same run.
