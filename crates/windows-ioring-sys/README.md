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
use windows_ioring_sys::{Batch, IoRing, SharedFile};
use std::os::windows::io::OwnedHandle;

let file = std::fs::File::open(r"C:\some\file.bin")?;
let shared = SharedFile::new(OwnedHandle::from(file));
let mut ring = IoRing::new(8, 8)?;

let token = {
    let mut batch = Batch::new(&mut ring);
    let token = batch.read(&shared, vec![0_u8; 4096], 0, Default::default())?;
    batch.submit_and_wait(1, 5_000)?;
    token
};

let completion = ring.try_pop()?.expect("a completion is ready");
completion.result()?;
let (buffer, _file) = token.claim_if(&completion).expect("token claims its own completion");
println!("read {} bytes", buffer.len());
# Ok::<(), std::io::Error>(())
```

`EventDelivery` wires completions to the thread pool instead, so no thread
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

**Model B's wakeup source is separable from Model B's identity.** What makes it
Model B is who owns, submits, and drains -- one pinned thread, no sharing on the
data path -- not what that thread blocks on. There are two answers for the
latter, and both are Model B:

- **Fused submit-and-wait**, blocking in `Batch::submit_and_wait`. Use it when
  the domain's only I/O is ring I/O: nothing to re-arm, nothing to multiplex.
- **A multiplexed wait**, blocking in `WaitForMultipleObjects` over
  `IoRing::completion_event` alongside other handles. Use it when the domain
  must also service a shutdown event, a socket, an overlapped operation, or a
  timer. `IoRing::completion_event` hands back an owned *duplicate* of the
  ring's event, so the caller keeps its ring. See
  [examples/model_b_multiplexed.rs](examples/model_b_multiplexed.rs) for the
  whole shape end to end, including shutdown with I/O still outstanding.

Picking the second is not "Model A with extra steps" and costs none of the
locality that motivated Model B. It does inherit one contract: the ring's event
is **edge-triggered on the completion queue going empty to non-empty**, so a
waiter must drain to empty before waiting again, on every pass, and must treat a
wake with nothing to pop as normal. That is measured rather than documented by
Win32, and getting it wrong hangs rather than merely slows -- read
`IoRing::completion_event`'s own docs before using it.

**The `drain_preceding` barrier stops at the ring's edge.** This is the reason
the multiplexed shape has to exist. `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` orders
SQEs against SQEs and is powerless in both directions across the ring boundary:
it can neither make a ring operation wait for an overlapped one nor make an
overlapped operation wait for ring operations. A consumer mixing ring and
non-ring I/O -- the normal case, not an exotic one -- must therefore enforce
that ordering in its own code, and the multiplexed wait is what lets it do so
without surrendering the ring or parking a thread in a blocking drain. The
barrier is also ring-wide and spans submissions, so a drained flush stalls the
whole ring for its duration; see `PushOptions::drain_preceding`.

Most real applications want Model B on the hot data path and Model A everywhere
else -- the control plane, background work, cold paths -- where the thread
pool's quiescence is worth more than locality. This crate supports both as
first-class; neither is a degraded form of the other.

## Durability

Three facts, all measured rather than documented by Win32, and all of them
things a consumer gets wrong by default. `Batch::flush`, `FlushCoverage`,
`WriteCaching` and `FlushMode` state them in full; this is the summary that
stops a reader from never looking.

1. **There is no FUA.** `BuildIoRingWriteFile`'s entire flag set is
   `{FILE_WRITE_FLAGS_NONE, FILE_WRITE_FLAGS_WRITE_THROUGH}`, and write-through
   is a cache directive to the OS, not a device-level guarantee -- whether it
   becomes a Force Unit Access bit depends on the driver, the volume, and the
   device's write-cache setting. A completed write-through write may still be
   sitting in a volatile device cache.
2. **The flush operation is the only durability primitive the ring has**, and
   only in a mode that syncs the device -- `FlushMode::NoSync` deliberately
   does not, which makes it the one mode that commits nothing.
3. **A flush without the barrier covers nothing.** An unflagged flush is an
   ordinary operation competing with the writes before it, and it frequently
   wins, so its completion proves nothing about them. This is why
   `Batch::flush` requires a `FlushCoverage` rather than defaulting: the
   obvious spelling was a silent data-loss bug, invisible until power is lost.
   Note that seeing your flush land last on your hardware is not evidence you
   can omit the barrier -- which direction the reordering shows in is
   device-dependent.

So **durability is a property of an epoch, never of an individual write**,
because the ring offers no per-write primitive to make it one: stream the
writes, close the epoch with one covering flush, and wait on the flush rather
than on the writes. The barrier that makes this correct is also a ring-wide
stall, so the correct construction is also the expensive one. "Durability on
the ring" in [DESIGN-NOTES.md](DESIGN-NOTES.md) has the full shape and the three
ways to pay for it.

## Cargo features

| Feature | Default | What it adds |
|---|---|---|
| `threadpool` | on | `EventDelivery` (Model A), and with it the dependency on `windows-threadpool-sys`. |

`EventDelivery` is the only item in this crate that needs a thread pool, so a
Model B consumer -- one pinned thread per domain, parked in
`Batch::submit_and_wait`, owning its own ring -- otherwise links a dependency it
never calls. Turning the feature off drops that dependency entirely:

```toml
[dependencies]
windows-ioring-sys = { version = "0.1", default-features = false }
```

The gate is justified on layering rather than runtime cost: linking
`windows-threadpool-sys` creates no threads, because the Win32 default pool is a
process-wide facility instantiated lazily on first use. A ring wrapper simply
does not intrinsically depend on a thread pool. Default-on keeps the change
additive, so no existing consumer has to do anything.

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
