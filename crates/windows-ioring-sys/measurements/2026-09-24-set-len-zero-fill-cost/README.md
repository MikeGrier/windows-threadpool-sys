# What `set_len` costs, and when -- 2026-09-24

Three runs of [set-len-zero-fill-spike.rs](../../design-sessions/spikes/set-len-zero-fill-spike.rs),
kept because they correct a claim this repository was making from documentation
rather than measurement.

## The question

`M25.3` zero-fills the log's extent with a real write before opening it
`NO_BUFFERING | OVERLAPPED`. The first draft of that code's documentation
justified the choice by asserting that `set_len` would leave every write in the
extending case, and separately described `SetFileValidData` as saving "a few
milliseconds of zeroing".

Review supplied a counter-recollection from optimizing `.cab` expansion some
years earlier: **setting the length too early forced the zero fill of large
files.** That is not the same model as "set_len defers the work", and neither
statement had been measured here, so both were put to the spike.

## What was run

Five cases per size, each on a fresh file in `%TEMP%`, timed separately, three
runs back to back. Full output in [runs.txt](runs.txt); one representative run:

| size | `set_len` | zero-fill | `set_len` + write@0 | `set_len` + write@end | `set_len` + sequential fill |
|---|---|---|---|---|---|
| 64 MiB | 0 ms | 19 ms | 0 ms | 67 ms | 20 ms |
| 256 MiB | 0 ms | 88 ms | 0 ms | 347 ms | 76 ms |
| 1024 MiB | 0 ms | 315 ms | 0 ms | 2304 ms | 287 ms |

The three runs agree closely; the 1 GiB `write@end` figure ranged 2304-2646 ms.

## What it shows

**`set_len` itself is free.** Under 2 ms at every size, including 1 GiB. It does
not zero eagerly.

**A write that lands at the valid data length is free too.** Writing one sector
at offset 0 of a `set_len`'d file costs nothing, because there is no gap in
front of it.

**A write that lands past the valid data length pays for the whole gap,
synchronously, inside that one write.** One sector at the end of a `set_len`'d
1 GiB file took roughly 2.3 seconds -- about **eight times** what writing the
entire extent sequentially costs. This is the recollection review supplied, and
it is the sharpest figure in the table: the zeroing is not avoided by `set_len`,
it is deferred onto whichever unlucky write first reaches past it, and it is far
more expensive there than it would have been up front.

**A sequential writer pays nothing extra for having set its length first.**
Filling the extent after `set_len` costs the same as zero-filling it outright
(287 ms against 315 ms at 1 GiB), because every write lands exactly at the valid
data length and no write ever has a gap in front of it.

## What this corrects

- **"`SetFileValidData` saves a few milliseconds of zeroing" was wrong** by two
  to three orders of magnitude. Zeroing is ~300 ms per GiB when done well and
  seconds per GiB when forced onto a seeking write. That cost is the entire
  reason the API exists and the reason databases hold
  `SE_MANAGE_VOLUME_NAME` to use it.

- **"`set_len` would leave every write extending" was the wrong mechanism** for
  this log. For a sequential writer the zeroing cost is identical either way.
  The measured reason the log zero-fills rather than `set_len`s is the *pending
  rate*, which is a separate measurement in
  [2026-09-24-set-len-vs-zero-fill/](../2026-09-24-set-len-vs-zero-fill/README.md),
  not a zeroing cost.

- **`set_len` as a pre-allocation strategy is safe only for strictly sequential
  writers**, and is a severe footgun for anything that seeks ahead -- which is
  what makes it a plausible-looking mistake rather than an obvious one.

## What it cannot tell you

These are wall-clock costs on one machine, one filesystem, one device. NTFS is
free to change how it tracks valid data length, and a different device would
move every figure. What the numbers support is a shape -- free, free, free,
catastrophic, free -- and the conditions under which the expensive case fires,
not a constant to design against.

## What it costs at this sample's own sizes

The figures above answer "what does zeroing cost per unit". Review then asked
what this sample actually pays: how large are the areas it zeroes?

Measured at the sample's real sizes with
[prealloc-cost.rs](prealloc-cost.rs), three runs of fifty fills each, in
[at-sample-sizes.txt](at-sample-sizes.txt):

| case | bytes | median |
|---|---|---|
| the log | 143,360 (140 KiB) | 8.4-9.5 ms |
| one strategy file | 8,421,376 (8.0 MiB) | 3.2-8.8 ms |

One whole run pre-allocates 24.2 MiB across four files -- the log plus one file
per strategy -- for 18-36 ms of zero-filling.

**These figures supersede an earlier capture, and are not a controlled
comparison against it.** The first capture ran a 1 MiB fill chunk where the
sample itself uses 64 KiB, so it did not measure the chunking of the code it
reports on; review caught that and the benchmark now matches the sample. The
re-run also happened on a machine that was concurrently building, so **two
things changed at once** and the difference between the two captures cannot be
attributed to the chunk size. The superseded figures are not reproduced here,
because a number nobody can act on is worse than no number.

**At the log's own size, throughput is far below the larger case.** 140 KiB
fills at roughly 17 bytes per microsecond against 950-2250 for 8 MiB, and the
maxima over fifty fills ranged 10-54 ms for the 140 KiB case against 11-16 ms
for the 8 MiB one -- a small write whose worst case exceeds a write sixty times
its size. What the file creation and flush contribute versus the writing is not
separated by this benchmark, which times the whole operation.

**At these sizes the three approaches differ by less than that variance.**
Explicit fill, `set_len` alone, and the touch-end trick all complete within the
spread of a single case's own repeats. The eightfold difference the table at
the top of this page shows appears at gigabyte extents, which is the scale the
sample's code is written to teach rather than the scale it runs at.

## Provenance

- Host: the development machine this repository is worked on; NTFS, ARM64
  Windows, single NUMA node.
- Date: 2026-09-24.
- Spike: [set-len-zero-fill-spike.rs](../../design-sessions/spikes/set-len-zero-fill-spike.rs),
  built `--release` in a scratch crate; no dependencies.
