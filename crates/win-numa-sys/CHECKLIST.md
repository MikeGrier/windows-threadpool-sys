# Checklist: win-numa-sys

Memory-safe Rust over the Windows NUMA APIs. See [README.md](README.md) for what the crate is.

## M1 -- Finish removing the duplication the crate was created to remove

The crate exists because two independent `VirtualAllocExNuma` implementations had appeared in
crates that do not depend on each other. Creating it moved one of them; this milestone removes the
other, and until it does the duplication is **relocated rather than removed**, which is worth saying
plainly rather than counting the crate as done.

- [ ] **N-1.1** -- **Collapse `windows-placement-probe`'s allocator onto this crate.**
  `peer_index_cache.rs` has its own `VirtualAllocExNuma` + `VirtualFree` pair, and that crate does
  not depend on `windows-ioring-sys`, so it never saw the one that was hoisted in `M22.3`.

  **It is not a drop-in, and the differences are the work.** That allocator also faults every page
  in -- committed pages are demand-zero, so until something writes to them no physical page has been
  drawn from the preferred node -- and then *observes* which node the pages actually landed on. It
  is typed over its own `Slot` rather than bytes, and it has a second origin (an ordinary heap
  allocation) that shares the same `Drop`.

  Decide what of that is general before moving any of it. Page-faulting looks general: any consumer
  who cares where pages landed has to do it, and this crate's own documentation already tells them
  so. The typed element and the dual origin look specific to the probe.

- [ ] **N-1.2** -- **Decide whether `QueryWorkingSetEx` observation moves here**, which `N-1.1` will
  force a view on. `observed_node_of_region`, `observed_node`, `working_set_flags` and the
  `working_set` bit constants live in `windows-placement-probe` today. "Which node is this page
  actually on" is a Windows NUMA concept and passes this crate's bar; against that, those helpers
  carry their own tests for the bit layout, and `working_set_flags` exists *separately from*
  `observed_node` precisely so a test can check the layout against a field whose value it already
  knows. Moving them means moving that care too, not just the code.

  Note what this would make possible, since it is the reason to consider it at all: a caller could
  then ask this crate whether a placement request was honoured, rather than being told by its
  documentation that success proves nothing and left to write `QueryWorkingSetEx` themselves.

## M2+ -- Parked

- [ ] **M2+.1** -- **Publish.** The crate is a path dependency of `windows-ioring-sys`, which *is*
  published, so it has to reach crates.io before that crate's next release or the release fails. It
  is already registered in `release-please-config.json` and in the publish workflow's tag patterns
  -- both of which this repository has previously been bitten by omitting, silently -- so what
  remains is the decision to cut `0.1.0`, not the plumbing.
