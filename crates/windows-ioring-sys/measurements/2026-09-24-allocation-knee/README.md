# Where the allocator's knee is, and what chunk size costs -- 2026-09-24

Three runs of [allocation-knee-spike.rs](../../design-sessions/spikes/allocation-knee-spike.rs),
kept because they settled a constant that had been picked rather than measured.

## The question

`M25.3`'s zero-fill loop needed a chunk size, and 1 MiB was chosen without
measuring. Review then offered a rule of thumb: **avoid contiguous allocations
over 64 KB from the general heap**, on the grounds that most heaps move large
blocks to a separate space anyway, and that the threshold is a useful place to
be asked "does this really need to be contiguous?" -- with the open question of
whether 64 KB is too conservative now and 1 MB is the better number.

Two things had to be measured to answer that: where this allocator actually
changes behaviour, and whether a larger chunk fills any faster.

## Q1 -- where does the allocator stop using the heap?

`VirtualQuery` on each of sixty-four live allocations of one size, counting
distinct `AllocationBase` values. Blocks carved from a heap segment share their
segment's base; a block with its own reservation has a base of its own.

| size | distinct reservations / 64 | verdict |
|---|---|---|
| 4 KB | 1 | shares a heap segment |
| 16 KB | 2 | shares a heap segment |
| 32 KB | 3 | shares a heap segment |
| 64 KB | 4 | shares a heap segment |
| 128 KB | 5 | shares a heap segment |
| 256 KB | 6 | shares a heap segment |
| 512 KB | 5 | shares a heap segment |
| **1024 KB** | **64** | **own reservation each** |
| 4096 KB | 64 | own reservation each |

The knee is between 512 KB and 1 MB. Below it, allocations share segments;
at 1 MB and above, every allocation gets a reservation of its own.

**Two earlier instruments failed**, and both are recorded in the spike because
both look reasonable:

- Testing for exact 64 KB **alignment** reported "none" at every size to 4 MB.
  `HeapAlloc`'s large-block path does call `VirtualAlloc`, but writes a header
  at the start and returns a pointer past it -- so a large block is aligned
  *plus a constant*, never exactly aligned.
- Counting distinct **offsets** within a 64 KB region declined with size (64 at
  4 KB, 15 at 1 MB) but never reached 1. Suggestive is not decisive.

## Q2 -- does a bigger chunk fill faster?

A 64 MiB zero-fill, seven times per chunk size, median of each run:

| chunk | MiB/s across three runs |
|---|---|
| 4 KB | 438, 727, 708 |
| 16 KB | 1609, 1678, 1692 |
| 32 KB | 2025, 2165, 1747 |
| 64 KB | 2439, 2608, 2306 |
| 128 KB | 2368, 2733, 2704 |
| 256 KB | 2441, 2596, 2865 |
| 512 KB | 2929, 2764, 2999 |
| 1024 KB | 2817, 2668, 3297 |
| 4096 KB | 2935, 2511, 2988 |

The climb from 4 KB to about 64 KB is large and unambiguous. Above 64 KB the
ranges overlap: 64 KB spans 2306-2608 and 4 MB spans 2511-2988. Three runs
cannot separate them.

## What was decided

`logfile::create_preallocated`'s chunk went from 1 MiB to **64 KiB**. On this
evidence 1 MiB was the worst of the plausible values: it is the first size with
no measurable throughput benefit *and* the first size that guarantees its own
reservation and teardown on every call.

## What this says about the rule of thumb

**64 KB is conservative relative to the allocator** -- the knee here is eight to
sixteen times higher than that, so the premise that "most heaps move large
allocations off to special spaces" holds, but not at 64 KB on this one.

**It is not conservative relative to throughput**, which is what makes it a good
default anyway: past 64 KB there was nothing left to gain in this workload, so
the habit costs nothing to keep.

**And 1 MB is a poor candidate to replace it with**, at least here -- it is
precisely the boundary. A number chosen to be "safely large" lands exactly where
the allocator's behaviour changes.

The rule's other half is not a measurement and is not challenged by any of this:
a threshold that makes someone ask "does this really need to be contiguous?" is
doing design work, not allocator work.

## A decision these measurements argue for and review overruled

Everything above points at removing the allocation instead of sizing it: a
`static` array of zeros needs no heap, no knee, and no justification, and its
pages arrive demand-zero from the loader.

**That was declined for a reason no measurement here could produce.** A `static`
sits at a fixed offset within the module, so any leak of a module base also
gives away the address of a large, writable, zero-filled region -- present for
the life of the process whether or not a log is ever opened, and no longer
protected by ASLR once the base is known. A transient heap allocation has an
unpredictable address and a lifetime bounded by the fill.

It is recorded here, and at the constant's definition, because the `static` is
the obvious "optimization" for a reader who has only the figures on this page.

## What this cannot tell you

One machine, one toolchain, one allocator, one workload. Rust's default
allocator on Windows is the system heap, and a process using the segment heap,
a different global allocator, or a different Windows version may put the knee
elsewhere. The filling throughput is a buffered write to one device and says
nothing about any other. Re-run the spike rather than quoting these figures.

## Provenance

- Host: the development machine this repository is worked on; ARM64 Windows,
  NTFS, single NUMA node.
- Date: 2026-09-24.
- Spike: [allocation-knee-spike.rs](../../design-sessions/spikes/allocation-knee-spike.rs),
  built `--release` in a scratch crate.
