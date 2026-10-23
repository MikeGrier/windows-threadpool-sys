# Completed checklist: rc-errors

Completed items from [CHECKLIST.md](CHECKLIST.md), append-only.

## Moved 2026-10-23 17:21:49 -04:00 -- RCE-1.1: the allocator interface

### <a id="rce-11"></a>RCE-1.1 -- The allocator interface: Allocator, AllocError and Global, mirroring upstream, with the NOTICE crediting the standard library. *(completed 2026-10-23 17:21:49 -04:00)*

Per [RCE-D-3](DESIGN-NOTES.md#rce-d-3) and [RCE-D-9](DESIGN-NOTES.md#rce-d-9). The item as it stood at completion:

**The allocator interface**: `unsafe trait Allocator` with `allocate` and `deallocate`, `AllocError`, and `Global` over
`alloc::alloc`, mirroring upstream's names and signatures, and usable as `dyn Allocator + Sync`. `Global` returns a
dangling, aligned block for a zero-sized layout and frees none, and reports failure without panicking. The `NOTICE` file
crediting the standard library.

What was done. [allocator.rs](src/allocator.rs) holds the three, with the trait's safety contract written for this crate
rather than copied. [NOTICE](NOTICE) states what is derived and carries the Rust project's MIT notice, read from
rust-lang/rust's `LICENSE-MIT` on 2026-10-23. The tests cover `AllocError`'s text and value, `Global` over ten layouts
(sizes 1 to 65,536, alignments 1 to 8,192) written and read back, the zero-sized layout at five alignments, a layout of
`isize::MAX` refused without a panic, distinct non-overlapping blocks, `Global` and two allocators of the tests' own reached
through `&'static (dyn Allocator + Sync)`, and a block freed on another thread; `Global`'s doc example is a doctest. A
sabotage of the dangling pointer's address (`layout.align()` to `1`) failed the zero-sized test, and was reverted; the
manifest entries are RCE-1.4's.
