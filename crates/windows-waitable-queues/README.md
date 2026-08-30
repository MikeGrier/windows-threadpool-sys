# windows-waitable-queues

Bounded producer/consumer queues whose readiness is a waitable Windows `HANDLE`.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

**Status: skeleton.** The shapes are not implemented yet. The decisions they will
be built against are in [DESIGN-NOTES.md](DESIGN-NOTES.md), and the work is
tracked in [CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) at the
workspace root.

## Why

Rust has good concurrent queues. What none of them offers on Windows is the one
property this crate is named for: **you cannot wait on them alongside a kernel
object.**

`crossbeam-channel` blocks in `recv`, but parks on its own internal primitive and
exposes no `HANDLE`; its `Select` is built purely from channel operations, with no
way to register a foreign OS object. `crossbeam-queue` does not block at all.

So a thread that needs to wake on

> a message arrived **or** my I/O completed **or** shutdown was signalled

cannot express that wait. It has to poll one source while blocking on another,
which either burns a core or adds latency.

On Windows a `HANDLE` is the universal waitable currency -- `WaitForSingleObject`,
`WaitForMultipleObjects`, `MsgWaitForMultipleObjects`, a thread-pool wait, and
alertable waits all take one. A queue whose readiness *is* a `HANDLE` composes
with everything the platform can wait on. One that hides its readiness behind a
private primitive composes with nothing.

## Plural, and no `Queue` type

This is a family of shapes, not one queue: they differ in producer and consumer
cardinality, in how they store items, and in what they do when full. None is
canonical, so there is deliberately no type named `Queue` -- a consumer names the
shape it wants.

Each shape splits into a **producer handle** and a **consumer handle**, and
cardinality is carried by whether those handles are `Clone`:

| Shape | Producer | Consumer |
|---|---|---|
| SPSC | not `Clone` | not `Clone` |
| MPSC | `Clone` | not `Clone` |
| MPMC | `Clone` | `Clone` |

So "single producer" is a fact the compiler enforces, not a sentence in a doc
comment.

## What it will not do

- **It will not overwrite.** A full queue fails, or a reservation guarantees a
  slot. Overwrite-oldest is right for telemetry, where a lost entry is a lost
  sample; here an entry may be an I/O submission, where a lost entry is a lost
  operation.
- **It will not allocate on push.** Bounded shapes allocate once, at
  construction.
- **It will not create a kernel object you never use.** The doorbell is created
  lazily, so a consumer that only polls allocates none.

## Licence

Copyright (c) Mike Grier.
