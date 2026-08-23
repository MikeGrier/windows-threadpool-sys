# windows-ioring-sys

Memory-safe Rust over the Windows `IoRing` submission/completion ring.

**Windows only.** Every item is behind `cfg(windows)`; the crate builds to an
empty shell on other platforms.

**Under construction.** The design is settled and recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md); the build-out is tracked in
[CHECKLIST.md](CHECKLIST.md).

Windows 11 and Server 2022 added `IoRing`, a submission/completion ring for file
I/O closer in shape to `io_uring` than to anything else Windows offers. This
crate raises those primitives into safe Rust with the minimum additional CPU and
memory cost: a completion hands the caller's buffer back without the crate having
allocated anything to track it.

## Scope: a file data plane, not a general completion backend

The kernel's operation table is fixed at seven entries -- no-op, read, write,
flush, register-files, register-buffers, and cancel. There is no ioctl operation,
no socket operation, and no directory-change operation, and unlike Linux's
`io_uring` -- which grew to roughly fifty opcodes including full socket support
-- Windows `IoRing` has not grown beyond file I/O.

So this crate does not replace
[`windows-overlapped-io-sys`](../windows-overlapped-io-sys/README.md), which
remains the crate for arbitrary I/O on arbitrary handles. Use this one for
high-volume file reads and writes; use that one for everything else. Neither can
subsume the other, and the division is the kernel's rather than this
repository's.

## Availability is a runtime question

`IoRing` ships with Windows 11 and Server 2022, but three separate facts about it
are decided at runtime rather than at compile time:

- which ring version the system supports;
- whether the ring is a real kernel ring or a **user-mode emulation** with no
  kernel benefit;
- whether the completion-event feature the thread-pool delivery path depends on
  is available at all.

All three are answerable without creating a ring, and this crate surfaces them
rather than hiding them -- a consumer reaching for a ring to maximize throughput
needs to know if they are getting an emulation.

## Choosing a delivery architecture

There are two coherent high-performance shapes, and they are mutually exclusive
on the hot path. Picking the wrong one costs more than any API detail in this
crate, so the trade-off is documented in full under "Two delivery architectures"
in [DESIGN-NOTES.md](DESIGN-NOTES.md), including why the NUMA node is the wrong
key for partitioning rings and why buffer placement likely matters more than
thread placement.

In brief: a shared queue with the system load-balancing across a thread pool is
convenient and quiesces to no threads at all, while shared-nothing execution
domains -- one pinned thread per domain owning its ring, its node-local buffer
pool, and its shard of the work -- is what the ring's API is actually shaped for.
This crate intends to support both as first-class, because most real applications
want the second on the data path and the first everywhere else.

## License

MIT. Copyright (c) Mike Grier.
