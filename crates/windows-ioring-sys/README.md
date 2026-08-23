# windows-ioring-sys

Memory-safe Rust over the Windows `IoRing` submission/completion ring.

**Windows only.** Every item is behind `cfg(windows)`; the crate builds to an
empty shell on other platforms.

Windows 11 and Server 2022 added `IoRing`, a submission/completion ring for file
I/O closer in shape to `io_uring` than to anything else Windows offers. This
crate raises those primitives into safe Rust with the minimum additional CPU and
memory cost: a completion hands the caller's buffer back without the crate having
allocated anything to track it.

## Example

Submit a read, then pop its completion once it is ready:

```rust,no_run
use windows_ioring_sys::{Batch, IoRing};
use std::os::windows::io::AsRawHandle;

let file = std::fs::File::open(r"C:\some\file.bin")?;
let mut ring = IoRing::new(8, 8)?;

let token = {
    let mut batch = Batch::new(&mut ring);
    let token = batch.read(file.as_raw_handle(), vec![0_u8; 4096], 0, Default::default())?;
    batch.submit_and_wait(1, 5_000)?;
    token
};

let completion = ring.try_pop()?.expect("a completion is ready");
completion.result()?;
let buffer = token.claim_if(completion.user_data()).expect("token claims its own completion");
println!("read {} bytes", buffer.len());
# Ok::<(), std::io::Error>(())
```

[`EventDelivery`] wires completions to the thread pool instead, so no thread
ever calls `try_pop` itself; see `examples/model_a_delivery.rs` for that shape,
and the "Choosing a delivery architecture" section below for when to reach for
each.

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

**Model A -- shared queue, kernel load-balances.** `EventDelivery` wires the
ring's completion event to a `ThreadpoolWait` from `windows-threadpool-sys`, so
no thread of yours ever blocks on I/O. Start here; see
[examples/model_a_delivery.rs](examples/model_a_delivery.rs) for a full worked
example that submits without waiting and receives every completion on a pool
thread.

**Model B -- shared-nothing execution domains.** One pinned thread per domain,
owning its ring, its buffer pool, and its shard of the work, parked directly in
`Batch::submit_and_wait` -- the fused submit-and-wait *is* the event loop. This
is the shape `IoRing`'s own API is built for.

Most real applications want Model B on the hot data path and Model A everywhere
else -- the control plane, background work, cold paths -- where the thread
pool's quiescence is worth more than locality. This crate supports both as
first-class; neither is a degraded form of the other.

## Topology guidance

This crate does not partition anything for you (D-8): it makes a ring cheap and
correct, makes its affinity explicit, and leaves sizing a Model B execution
domain to the caller.

- **Size a domain by last-level (L3) cache, not by NUMA node.** Node count is a
  firmware setting a process cannot see, and most real deployments are
  virtualized, where NUMA topology is often invisible entirely. See
  [examples/l3_domains.rs](examples/l3_domains.rs) for a runnable enumeration,
  built on the safe `GetLogicalProcessorInformationEx` wrapper in
  [`windows-topology-sys`](../windows-topology-sys/README.md).
- **Processor groups are a hard floor.** A thread's affinity is a
  `GROUP_AFFINITY` and a ring's waiter lives in exactly one group, so above 64
  logical processors the partition is forced whether or not it is wanted.
- **Buffer placement likely dominates thread placement.** `VirtualAllocExNuma`
  on the node closest to the device, registered once into that domain's ring
  via `Batch::register_buffers`, is very likely the highest-leverage locality
  decision available -- independent of everything above about completion
  routing.

`examples/ring_copy` is where these three points become runnable policy: it
copies one file to another through per-domain rings, sized by a named
`ByL3`/`ByNode`/`ByPackage`/`ByCore`/`Single` policy, with buffers placed via
`VirtualAllocExNuma` and a `--placement local|remote` switch to make the
placement effect measurable. It is a **sample**, not library surface -- this
crate itself depends on no partitioning policy and does not depend on
`windows-topology-sys`; only the sample does.

## License

MIT. Copyright (c) Mike Grier.
