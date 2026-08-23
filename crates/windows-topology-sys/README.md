# windows-topology-sys

Safe enumeration of Windows processor, cache, and memory topology.

**Windows only.** Every item is behind `cfg(windows)`; the crate builds to an
empty shell on other platforms.

## Why this crate exists

`GetLogicalProcessorInformationEx` is the Win32 entry point for topology, and
the `windows` crate exposes it as typed but still `unsafe` FFI: a raw output
pointer, two-call sizing, and records that must be walked by their own
self-reported `Size` rather than indexed as a slice. Several of the records
also declare a trailing array as length 1 (`PROCESSOR_RELATIONSHIP::GroupMask:
[GROUP_AFFINITY; 1]`) while actually holding as many entries as `GroupCount`
reports -- reading past element 0 is exactly what correct use of the API
requires, and exactly what Rust calls undefined behavior if done through the
declared type. None of that is solved by a thin `unsafe fn` wrapper; it has to
be solved by walking the buffer correctly.

This crate does that walk once, safely, and hands back owned records.

## Scope

Safe enumeration, not an opinionated topology model. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) for the full reasoning, including a
cross-check against Linux's topology model and an explicit list of what this
crate deliberately does not attempt (devices, HMAT-style attributed
distances, queue affinity).

## License

MIT. Copyright (c) Mike Grier.
