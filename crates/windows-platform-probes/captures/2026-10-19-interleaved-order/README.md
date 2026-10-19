# Queue contention, interleaved against sequential

Fourteen whole-probe invocations taken to answer one question: when the passes of
every configuration at a producer count are interleaved, does the same-code control
come out narrower than when each configuration's passes run back to back?

The control is `reserving_mpsc` against `reserving(32/32)`: the same code measured
twice in one run. Back to back, its two halves are measured several configurations
apart. Interleaved, every configuration takes one pass per round, so both halves'
passes fall within the same rounds. The decision is
[The queue probe interleaves its passes](../../DESIGN-NOTES.md#d-interleaved-order).

## Attribution

| | |
|---|---|
| Host | `x86_64 16p/8c smt+ L2[2,2,2,2,2,2,2,2] ec[0:16] numa[16]` |
| Profile | release |
| Sampling | 50,000 pushes per producer, median of 5 repetitions, one untimed warmup pass |
| Order | 7 runs `--order sequential`, 7 runs `--order interleaved`, alternating, sequential first |
| Runs | [sequential-run1.txt](sequential-run1.txt) to `sequential-run7.txt`, [interleaved-run1.txt](interleaved-run1.txt) to `interleaved-run7.txt` |
| Instrument | `probe-queue-contention`, built from the source the commit adding this directory carries; the only source change after the runs is test-only |
| Taken | 2026-10-19, 15:26 to 15:42 -04:00 |

The host banner is the one the
[2026-09-16 capture](../2026-09-16-drained-handshake/README.md) carries, so the two
are the same machine; nothing here says anything about any other hardware, and
`numa[16]` is a single node holding all sixteen processors.

**The runs alternate orders deliberately.** Seven sequential runs followed by seven
interleaved ones would put any drift across the quarter hour between the two
batches, and that drift would read as a difference between the orders.

## Reading it

[summarise.js](summarise.js) and [isolated.js](isolated.js) are byte-for-byte copies
of the 2026-09-16 capture's scripts, which parse this report format unchanged. Their
outputs, per order:

| | sequential | interleaved |
|---|---|---|
| isolated | [sequential-isolated.txt](sequential-isolated.txt) | [interleaved-isolated.txt](interleaved-isolated.txt) |
| drained | [sequential-drained.txt](sequential-drained.txt) | [interleaved-drained.txt](interleaved-drained.txt) |

regenerated from this directory with, for each order:

```
node isolated.js <order>-run1.txt ... <order>-run7.txt
node summarise.js <order>-run1.txt ... <order>-run7.txt
```

Each script requires every input to carry the same `sampling:` line, and the order
is part of that line, so the two orders cannot be summarised together by accident.

**What the runs showed, per producer count.** In the isolated regime, the control's
observed range across the seven runs was narrower interleaved than sequential at 1,
2, 16 and 32 producers, and about as wide at 4 and 8. In the drained regime the
comparison went both ways across producer counts. Each range is seven observations,
which is a range and not a band, so these say what these fourteen runs did, on this
host, and not what a fresh pair of sweeps would do.
