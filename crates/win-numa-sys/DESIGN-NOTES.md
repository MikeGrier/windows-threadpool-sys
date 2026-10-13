# Design notes: win-numa-sys (Tier 1)

Current canonical decisions for this crate. See [README.md](README.md) for what it is and
[CHECKLIST.md](CHECKLIST.md) for what is planned.

Repository-level decisions this crate sits under: the `win-` prefix and what `-sys` promises, both in
the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix).

## Decision index

| ID | Decision |
|---|---|
| <a id="n-d-1"></a>N-D-1 | **Both ways of arriving at a node are offered; choosing between them is not.** A caller may *declare* a node (`NumaBuffer::new`) or *discover* one (`volume_numa_node`), and may qualify what a discovered answer is worth (`highest_numa_node`). What the crate refuses is a constructor that does both in one step -- there is no `NumaBuffer::for_file`. |
| <a id="n-d-2"></a>N-D-2 | **`NumaBuffer` implements `io-buffers`' `IoBuf` and `IoBufMut`, and this crate still defines no buffer trait.** The assistant's choice, 2026-10-12, queued for the engineer's review in durable-ioring's `DI-9.7`; it is `IB-1.2`, from [io-buffers' IB-D-4](../io-buffers/DESIGN-NOTES.md#ib-d-4). **What it ends:** this crate had declined to implement any buffer trait, because the ring crate and `windows-overlapped-io-sys` each had their own and a third copy would have made the duplication harder to resolve; the ring crate implemented its own trait for `NumaBuffer` by the orphan rule. Once the trait is a neutral crate's, neither the ring crate nor that crate owns both trait and type, so the implementations move to the crate that owns the type; this crate depends on `io-buffers` for it. **What it keeps:** the crate defines no trait, takes no opinion on buffers beyond this one implementation, and has no policy about where memory is placed. **What it costs:** a dependency, on a portable crate with no dependencies of its own, and a release ordering -- `io-buffers` must be on crates.io before this crate's next release, which is what the publish workflow's step that waits for workspace siblings is for. The two implementations are two lines each and their tests assert the correspondence between the trait methods and the inherent accessors, which is where a delegation wired to the wrong thing would show. |

## N-D-1: both ways of arriving at a node, and no shortcut between them

*Recorded 2026-09-23 by `M23.2` in
[windows-ioring-sys/CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md), which asked this crate's
predecessor whether it should accept a declared storage node as an input.*

### What is offered

- **Declare.** `NumaBuffer::new(len, Some(node))` allocates on a node the caller names. The caller
  may have got that node from anywhere -- a topology walk, a configuration file, a measurement, a
  coin toss. This crate does not ask.
- **Discover.** `volume_numa_node(handle)` asks a handle's volume which node it reports, and returns
  it. It does not allocate anything.
- **Qualify.** `highest_numa_node()` answers how many nodes exist, so a caller can tell a real choice
  from the only choice available.

Both paths have consumers in this workspace already, which is the evidence that neither is
speculative: `examples/epoch_log` discovers from its log file's volume, and `examples/ring_copy`
declares a node it computed from the processor topology.

### What is refused, and why

**There is no `NumaBuffer::for_file(handle, len)`** -- no call that queries a node and allocates on
it in one step. It is the obvious convenience and it is the wrong shape:

- **It hides the answer.** On a single-node machine, "placed on the node the volume named" and "no
  preference" are the *same allocation*, so a caller could not tell whether the query had found
  anything. That is the failure the epoch-log sample's report line exists to prevent, and it is the
  2026-08-30 session's *report, do not route* position applied one layer down.
- **It fuses two failure domains.** Allocation can fail; the query can fail. A combined call has to
  decide what happens when only the query fails -- allocate unplaced, which silently does something
  other than asked, or return an error, which fails an allocation that would have succeeded. Neither
  is right for every caller, so neither should be baked in. Split, the caller decides.
- **The answer is worth more than one buffer.** A node can inform a thread's affinity, a second
  allocation, a report, or a plan. Tying the query to one constructor means the second consumer
  writes it again -- which is how this crate came to exist.
- **And the convenience is already there.** `NumaBuffer::new(len, volume_numa_node(h).ok())` type-
  checks as written, because the query returns exactly what the constructor takes. The shortcut would
  save one line.

**An argument that no longer applies, recorded so it is not re-made:** when this was first argued,
the buffer lived in `windows-ioring-sys` and a `for_file` constructor would have dragged
`Win32_System_Ioctl` into a crate that otherwise touched only memory. That was a real cost then. It
is not one now -- `volume_numa_node` lives here and the feature is already declared -- so the
argument is void and the four above are what the decision rests on.

### Why discovery is offered at all

The discovered answer is weak, and `volume_numa_node`'s own documentation says so: it answers for a
*volume*, a volume may span devices, and then a single reported node is a fiction rather than an
answer. A defensible reading is that a query this weak should not be offered.

It is offered anyway, because the alternative is worse. Refusing it does not stop a consumer needing
the answer; it makes each one write the `DeviceIoControl` themselves. That is not hypothetical --
this crate exists because `VirtualAllocExNuma` had been written three times in this workspace on
exactly that logic, and the epoch-log sample had hand-rolled this very FSCTL. The honest move is to
provide it **and say plainly what it is worth**, which puts the caveat where the caller will read it
rather than leaving them to discover the limitation themselves.

### What this preserves

[D-8](../windows-ioring-sys/DESIGN-NOTES.md#d-8) -- locality is the consumer's decision -- is intact
and is the reason the shape is what it is. This crate supplies a fact and an allocator. It does not
map a file to a node, does not shard anything, and does not choose on a caller's behalf. A consumer
who wants those things composes them from what is here, on hardware this workspace has never seen.
