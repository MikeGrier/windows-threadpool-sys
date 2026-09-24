# Append batching -- 2026-09-22

Twenty runs of `examples/epoch_log`, ten with each append path submitting one
write per record and ten with them batched, kept so the question `M22.1` asked
can be re-read without paying for the runs again.

## The question

Review finding `E-1` observed that both append paths built a `Batch`, pushed a
single write, and submitted -- so a sample whose job is to teach `Batch` never
amortised a submission. It raised a second possibility beyond the teaching
defect: that the fixed per-record submission cost was **a term every strategy
paid equally**, and therefore a shared constant capable of flattening the
three-way comparison whose headline result is that the strategies are
indistinguishable.

`M20.6` is gated on that, so the measurement is the point of the item rather
than a bonus.

## What was run

The unmodified sample, ten times before the change and ten after:

```powershell
cargo run -q -p windows-ioring-sys --example epoch_log
```

The "before" runs were taken by stashing the change and restoring it
afterwards, so both sets come from one machine in one sitting rather than from
two builds separated by other work.

Each file here is one run, trimmed to the strategy block. The sample's own
workload: 32 epochs of 64 records, each run replayed.

## Provenance

- commit: `6cba2886` (the change under measurement was uncommitted at the time)
- CPU: AMD EPYC 7763 64-Core Processor
- OS: Microsoft Windows 11 Enterprise 10.0.26200
- arena: 8 slots; so batching turns 64 submissions per epoch into 8

## What the runs show

**Throughput: no change that can be distinguished from noise.** Median
records/sec moved by between 1% and 5%, in a spread whose run-to-run range
within a single strategy is 1.17x to 1.57x. A 5% median shift inside a 57%
range is not a result.

**Commit p50: a real reduction, and the one finding here that separates.** For
`covering-flush` the ten before-values and the ten after-values barely overlap
-- only one after-value exceeds the lowest before-value. The same direction
holds for the other two strategies. This is the expected shape rather than a
surprise: batching removes seven of every eight `SubmitIoRing` calls from the
append path, which shortens the interval between the last append and the flush
being reached. Throughput is bound by the device flush and does not move;
latency is not, and does.

> **Corrected 2026-09-24 by `M25.6`: the paragraph above is measuring the
> append path, not the commit.** `M20.6` established that the figure this
> harness published as "commit p50" was **entirely deferral** -- the interval
> from pushing a flush to the harness next looking, which is how long the *next
> epoch's appends* took. Batching made those appends faster, so the number fell.
> The reduction is real and its stated mechanism is even correct as written
> ("shortens the interval between the last append and the flush being reached");
> what is wrong is the label, and therefore the conclusion that it "separates".
>
> **It is not an independent finding from the throughput result above.** Both
> are the same fact seen twice: the append path got faster, and the run is
> flush-bound, so the change appears in the metric that is not flush-bound and
> not in the one that is. Reporting one as "no change that can be distinguished
> from noise" and the other as "the one finding here that separates" reads as
> two results and is one.
>
> **What this does not disturb** is the question the capture was taken to
> answer. `E-1` asked whether a shared per-record submission cost was flattening
> the three-way comparison; the cross-strategy spread did not shrink, and that
> conclusion stands. See
> [2026-09-24-commit-decomposed/](../2026-09-24-commit-decomposed/README.md) for
> the comparison re-run once a commit could actually be measured.

**The cross-strategy spread did not shrink.** It sits at or below the
run-to-run range of a single strategy both before and after -- which is the
sample's own stated test for whether the choice is dominated by the device
flush.

## What this settles, and what it does not

**Settles:** `E-1`'s second possibility is **not supported**. Removing the
shared per-record submission cost did not change the comparison, so the
sample's existing conclusion -- that the three strategies are indistinguishable
-- survives a confound that was specifically raised against it. `M20.6` can be
settled on the grounds it already had.

> The mechanism this paragraph originally gave for that conclusion -- "because
> each pays one device flush per epoch, and everything they differ about lands
> two orders of magnitude below it" -- is the same claim corrected above, and
> `M25.5` has since measured those differences at *hundreds* of microseconds
> rather than tens. The conclusion is unchanged, because they remain smaller
> than the run-to-run spread; "below the noise" and "two orders of magnitude
> below the flush" are simply different claims, and only the first held.

**Does not settle:** whether `CommitStrategy::AlternatingRings` earns its cost.
That is a question about what the strategy buys in *correctness* and in
bounding a commit's blast radius, not about throughput, and no number here
speaks to it. It remains `M20.6`'s to answer.

**A caveat that belongs with the numbers rather than under them:** ten runs on
one machine with one device. The variance is wide enough that the throughput
half of this would not survive a smaller sample, and the latency half is stated
as "the distributions barely overlap" rather than as a percentage for the same
reason.
