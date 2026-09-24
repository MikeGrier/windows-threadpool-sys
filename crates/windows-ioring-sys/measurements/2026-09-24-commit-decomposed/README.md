# The three-way comparison, with the commit decomposed -- 2026-09-24

Fifteen runs of `examples/epoch_log` after `M25.3` gave it a pre-allocated
`NO_BUFFERING | OVERLAPPED` handle and `M25.4` split a commit's cost into parts.
Per-run figures in [runs.tsv](runs.tsv).

## The question

`M20.6` found that the harness's published commit latency was **entirely
deferral** -- it measured how long the next epoch's appends took, not the
commit -- and that the three strategies were therefore being compared on a
number that could not distinguish them. It left `M25.5` to re-run the
comparison once there was a commit to measure, with one question open: whether
`AlternatingRings` earns its permanent doubled arena registration on any ground
other than blast radius, which this harness structurally cannot exhibit.

## A correction this capture forced before it could answer anything

`M25.4`'s first numbers had `HostSequenced` committing roughly **six times
cheaper** than the other two. That was an artifact of where the clock started.
`HostSequenced` waits for every write in userspace before pushing an unordered
flush, and the commit clock began at the *submit* -- so its host round trip fell
outside every measured part. The cost had not gone anywhere; nothing was looking
at it.

A `prepare` part now covers whatever a strategy must do before its flush can be
pushed. With it, `HostSequenced`'s commit is not six times cheaper; it is within
noise of the others. **A reader of the uncorrected figures would have drawn the
opposite of the right conclusion**, which is the same failure mode `M20.6`
found, one layer down.

## What fifteen runs show

Medians, with the full range beside them:

| | rec/s | commit p50 (us) | commit p99 (us) |
|---|---|---|---|
| covering-flush | 7066 (6499-8896) | 1203 (605-1341) | 1953 (1597-4150) |
| host-sequenced | 7570 (5854-10414) | 1090 (375-1356) | 1896 (1494-7073) |
| alternating-rings | 7477 (6441-9133) | 1149 (436-1487) | 2172 (1409-34453) |

Where each strategy spends its commit, and how long it defers:

| | prep p50 (us) | submit p50 (us) | deferral p50 (us) |
|---|---|---|---|
| covering-flush | 0 | 1203 (604-1340) | 6958 (2200-7750) |
| host-sequenced | 886 (173-1071) | 198 (171-272) | 6753 (1537-8237) |
| alternating-rings | 0 | 1149 (435-1487) | 15041 (4916-17650) |

**Throughput and total commit cost: the spread across strategies is smaller
than the spread within one.** The medians sit within a few percent of each
other, every range overlaps every other range, and the run-to-run range of a
single strategy is wider than the gap between strategies. That comparison is
the test the sample's own output tells a reader to apply.

**Where the cost sits does not vary between runs.** `HostSequenced` spends its
commit in `prep` and almost nothing in `submit`; the covering strategies do the
reverse. The ordering holds in all fifteen runs. A single blended number cannot
show this, which is what `M25.4` was for.

**`AlternatingRings` defers about twice as long as the other two**, in every
run. The figure `M20.6` found misleading was deferral, so the strategy that
defers across two epochs reported the worst commit latency at the same
throughput as the others.

**`block` is zero at the median for all three**, in every run. That does not
establish inline completion -- an operation that pended and finished during a
deferral of several milliseconds reads identically from here.

## The open question, and what the data says about it

`M25.5` asked whether `AlternatingRings` earns its doubled arena registration on
some ground other than blast radius. What the fifteen runs show about it:

- its throughput median sits between the other two and inside both their ranges;
- its commit-cost median likewise;
- its deferral median is the longest of the three, in every run;
- the largest single commit p99 in the capture is its 34,453 us.

**What the harness cannot show, as a matter of its construction**: a
blast-radius difference. `M20.6` established that each lane registers its own
arena of the same size, so the per-ring bound on outstanding operations is
identical whether one ring or two are used. No run of this harness can separate
the strategies on the property `AlternatingRings` exists for.

So the record is: the ground the strategy was built on is not measurable here,
and the fifteen runs above are what was found on the grounds that are. What
follows for the strategy's status is a decision for the engineer; the conditions
under which it would pay are written down in
[strategy.rs](../../examples/epoch_log/strategy.rs).

## What this cannot tell you

One machine, one device, one workload of thirty-two epochs of sixty-four small
records. The run-to-run spread is large enough that a single run of this sample
says very little, which is why fifteen were taken and why the ranges are printed
beside the medians. None of it is a statement about the platform: Windows
specifies nothing about when a ring operation completes relative to
`SubmitIoRing`, and the log is required to be correct either way.

## Provenance

- Host: the development machine this repository is worked on; ARM64 Windows,
  NTFS, single NUMA node.
- Date: 2026-09-24.
- Binary: `cargo run -p windows-ioring-sys --example epoch_log --release`,
  fifteen consecutive runs on an otherwise idle machine.
