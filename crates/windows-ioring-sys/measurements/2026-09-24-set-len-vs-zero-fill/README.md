# `set_len` against a zero-fill -- 2026-09-24

Sixteen runs of the `write-pending-spike` with a fifth condition added, kept so
the question can be re-read without paying for the runs again.

## The question

`M25.3` opens the log over an extent that has been **zero-filled** -- a real
write of zeros -- and its documentation asserted that using
[`std::fs::File::set_len`] instead would be a silent regression, on the grounds
that only a write advances NTFS's valid data length.

**That was asserted from documentation, not measured**, and it was challenged in
review with a specific counter-hypothesis: that one of these combinations
already does the right thing and zero-fills on the caller's behalf without
requiring the write. The spike is the apparatus that can answer it, so a
condition E was added to it rather than the claim being argued.

## What was run

`design-sessions/spikes/write-pending-spike.rs`, built `--release` in a scratch
crate the way `tools/run-numa-spikes.ps1` builds the NUMA spikes, and run
sixteen times back to back on an otherwise idle machine. Each run is 500 trials
per condition; each trial is 8 writes of 4096 bytes plus a flush, submitted as
one batch, and counts as "pended" if fewer than 9 completions were queued when
`SubmitIoRing` returned.

The conditions, unchanged except for the new one:

- **A** buffered, no `OVERLAPPED`
- **B** buffered + `OVERLAPPED`
- **C** `NO_BUFFERING` + `OVERLAPPED`, extending the file
- **D** `NO_BUFFERING` + `OVERLAPPED`, over a zero-filled extent
- **E** `NO_BUFFERING` + `OVERLAPPED`, over a `set_len` extent *(new)*

Per-run counts are in [runs.tsv](runs.tsv). Summary over the sixteen:

| condition | min | median | max | runs >= 250/500 |
|---|---|---|---|---|
| A buffered sync | 0 | 0 | 0 | 0/16 |
| B buffered overlapped | 0 | 0 | 1 | 0/16 |
| C nobuffer extending | 1 | 268 | 446 | 9/16 |
| D nobuffer zero-filled | 121 | 471.5 | 500 | 12/16 |
| E nobuffer set_len | 1 | 268 | 494 | 9/16 |

## What the numbers say, and what they do not

**Buffering is the separation that replicates.** A and B pended once in sixteen
runs between them, over 16,000 trials. Every `NO_BUFFERING` condition pended in
most runs. That is the one distinction in this table large enough to survive the
run-to-run variance.

**C and E are not distinguishable here.** Identical medians, overlapping ranges,
and neither consistently above the other -- run 7 has C at 269 and E at 3, run 8
has C at 1 and E at 469. So the data is consistent with a `set_len` extent
behaving like an extending one, and it does **not** establish that it *is* one.

**D is higher than C and E, and much less than the earlier record implies.** Its
median is around 470 against 268, and its floor over sixteen runs is 121 where
theirs is 1. But D's own range reaches down to 121, C reaches up to 446, and E
to 494, so the distributions overlap substantially and a single run of either
can land anywhere in the other's range.

## The correction this forced

The `M25` checklist preamble says condition D "pended reliably", and the spike's
own prose says D "was the only condition that pends". **Neither replicates.**
Both descend from a single run in which D reported 500/500 and C reported
5/500; the spike's own header already warned that two runs minutes apart gave C
as 5/500 and then 271/500, and sixteen runs make clear that the C/D separation
is a difference of degree that a single pair of numbers dramatically overstates.

This does not undo `M25.3`. The log is opened `NO_BUFFERING | OVERLAPPED` over a
zero-filled extent, and that configuration has the highest observed pending rate
and the highest floor of the five. What changes is the confidence the prose may
express: the original reading of "only D pends at all" is an artifact of one
run, and the honest statement is that buffering is what decides whether
operations pend here at all, while the extent's preparation shifts a rate that
varies enormously run to run for reasons outside this program.

**And none of it is a contract.** Windows specifies nothing about when a ring
operation completes relative to `SubmitIoRing`. The log is required to be
correct whichever way it goes, which is why `M25`'s standing constraint forbids
anything depending on an operation pending -- a constraint this measurement
makes more rather than less important.

## Provenance

- Host: the development machine this repository is worked on; single NUMA node,
  NTFS, ARM64 Windows.
- Date: 2026-09-24.
- Spike: `design-sessions/spikes/write-pending-spike.rs` at the commit that
  added condition E.
- Files written to `%TEMP%`, one per condition, recreated per run.
