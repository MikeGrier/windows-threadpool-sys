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

The runs' regime banner reads `a consumer popping continuously`, which is what
`68198359` printed; a later commit changed that label to `a consumer looping on
pop`, because the handshake guarantees the consumer's pop path has run once, not
that it is scheduled without gaps. A later commit also rewrote the paragraph
under the comparison table, which used to read the point estimate as a verdict
and now directs the reader to the interval.

Both are prose the probe prints around its tables, not measurements, so the runs
below are not retaken for them: nothing about what was measured moved. Expect the
committed runs to differ from a fresh one in wording of this kind, and compare
the figures rather than the surrounding text.

## Reading it

[summary.txt](summary.txt) is the output of [summarise.js](summarise.js) over the
three runs, regenerated with:

```
node summarise.js run1.txt run2.txt run3.txt
```

[isolated.txt](isolated.txt) is the output of [isolated.js](isolated.js) over the
same three runs, regenerated with:

```
node isolated.js run1.txt run2.txt run3.txt
```

The two cover different regimes and are kept apart for that reason: `summarise.js`
derives the **drained** tables, `isolated.js` the **isolated** ones.

The isolated figures matter beyond this directory because the queue crate's
documentation makes a claim about them -- that the whole push path was measured
as slower under `Wide` at every producer count. That claim rests on a **separate**
seven-run sweep whose raw runs were never committed, and on the crate's own
attributed table, which is a **third** capture built from `fecd352`. This
directory is neither of those: it is three runs from `68198359`, and what
`isolated.js` provides is an independent cross-check that can actually be run,
against figures that otherwise have none. Where this and the crate's table
disagree, the crate's table is the attributed figure for that crate; this one is
evidence about how far such a figure moves.

Both are committed so the derivation can be checked rather than taken on trust, and
so nothing downstream has to retype a figure. The scripts derive what the runs do
not state individually: the across-run median per producer count, and the
same-code control -- which is a relation *between* two tables, since
`reserving_mpsc` in the comparison table and `32/32` in the layout table are the
same configuration measured twice in the same run.

**It reports per producer count and emits no verdict, deliberately.** An earlier
version pooled every control observation into one band and asked whether each
layout median fell inside it. It answered `true`, and the pooling is what
produced that answer: the control is not independent of producer count -- about
0.82-0.98x at one producer against 0.95-1.23x at thirty-two here -- so a pooled
band is wider than any count's own, and containment follows from the method
rather than from the data. Comparing per count does not rescue a verdict either,
because three runs give three control observations per count, and the range of
three samples is not a band to judge anything against.

So three runs do not settle the drained comparison in either direction. This
capture reports figures; the claim that nothing separates in the drained regime
rests on the seven-run sweep, which this does not replace.
