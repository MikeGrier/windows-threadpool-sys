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

**The cross-strategy spread did not shrink.** It sits at or below the
run-to-run range of a single strategy both before and after -- which is the
sample's own stated test for whether the choice is dominated by the device
flush.

## What this settles, and what it does not

**Settles:** `E-1`'s second possibility is **not supported**. Removing the
shared per-record submission cost did not change the comparison, so the
sample's existing conclusion -- that the three strategies are indistinguishable
because each pays one device flush per epoch, and everything they differ about
lands two orders of magnitude below it -- survives a confound that was
specifically raised against it. `M20.6` can be settled on the grounds it
already had.

**Does not settle:** whether `CommitStrategy::AlternatingRings` earns its cost.
That is a question about what the strategy buys in *correctness* and in
bounding a commit's blast radius, not about throughput, and no number here
speaks to it. It remains `M20.6`'s to answer.

**A caveat that belongs with the numbers rather than under them:** ten runs on
one machine with one device. The variance is wide enough that the throughput
half of this would not survive a smaller sample, and the latency half is stated
as "the distributions barely overlap" rather than as a percentage for the same
reason.
