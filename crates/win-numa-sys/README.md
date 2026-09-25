# win-numa-sys

Memory-safe Rust over the Windows NUMA APIs.

Windows provides some NUMA concepts; this crate builds library notions on top of them. Whether a
client uses them is up to the client.

## What is here

- **`NumaNode`** -- a node number, as Windows numbers them. A newtype because a node *number* and a
  *count* of nodes are both small integers and are trivially swapped at a call site.
- **`NumaBuffer`** -- an owned allocation made with `VirtualAllocExNuma` and released with
  `VirtualFree`, so a buffer can be placed on a chosen node rather than wherever the default
  allocator lands it.
- **`highest_numa_node()`** and **`volume_numa_node(handle)`** -- the two questions Windows will
  answer about nodes, wrapped so a caller can find a node to pass without writing the FFI.

## What is not here, and will not be

**Any opinion about which node anything should use.** This crate reports what Windows says and
allocates where it is told. It does not map a file to a node, does not shard anything, and does not
choose a node on a caller's behalf.

The `-sys` suffix is that promise. In this workspace the suffix means thin over Win32, memory-safe,
adding no policy -- see the repository's
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#the-waitable-queues-crate-is-named-plural-and-carries-no-sys-suffix),
where a sibling crate drops the suffix for failing exactly that test.

## The name

The first crate here to take `win-` rather than `windows-`. `windows` is Microsoft's namespace, and
a crate published as `windows-numa-sys` today is a name they may reasonably want tomorrow; see
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix). Existing crates keep their
names for now.

## A node argument is a preference, not an instruction

`VirtualAllocExNuma`'s parameter is `nndPreferred`, and the name is the contract. A successful
allocation is **not** evidence that the pages landed on the node that was asked for. Two things
follow, both of which have already caught someone in this workspace:

- Committed pages are demand-zero, so until something writes to them no physical page has been drawn
  from the preferred node at all. Measuring placement means faulting the pages in first.
- An *invalid* node is refused -- but `u32::MAX` is not a test of that, because it is the API's own
  no-preference sentinel and is accepted by design. A measurement that asks for `u32::MAX` and sees
  it succeed has measured the sentinel, not a range check.

Observing where pages actually landed needs `QueryWorkingSetEx`, which lives in
`windows-placement-probe` today; whether it moves here is [CHECKLIST.md](CHECKLIST.md) `N-1.2`.

## Buffer traits live with whoever owns them

`NumaBuffer` implements no I/O buffer trait, because this crate defines none. `windows-ioring-sys`
and `windows-overlapped-io-sys` each already have their own `IoBuf`/`IoBufMut` pair, and a third
copy here would have made that duplication harder to resolve rather than easier.

A consumer that owns such a trait implements it for `NumaBuffer` -- the orphan rule permits exactly
that, since the trait is theirs -- over the inherent `as_ptr`, `as_mut_ptr` and `len`.
`windows-ioring-sys` does so, and keeps re-exporting `NumaBuffer` so `windows_ioring_sys::NumaBuffer`
still resolves.
