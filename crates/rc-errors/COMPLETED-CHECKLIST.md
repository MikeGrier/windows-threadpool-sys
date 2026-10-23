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

## Moved 2026-10-23 19:03:04 -04:00 -- RCE-1.2: the shared error

### <a id="rce-12"></a>RCE-1.2 -- The shared error: one repr(C) block, a fallible constructor handing the error back, and counting freed through the recorded allocator, tested with a recording allocator. *(completed 2026-10-23 19:03:04 -04:00)*

Per [RCE-D-4](DESIGN-NOTES.md#rce-d-4), [RCE-D-5](DESIGN-NOTES.md#rce-d-5) and [RCE-D-8](DESIGN-NOTES.md#rce-d-8). The item
as it stood at completion:

**The shared error**: the `repr(C)` block, the fallible constructor and its `Global` variant handing the error back on
failure, `Clone`, `Drop`, `Deref` to `dyn Error + Send + Sync`, `Display`, `Debug`, `Error`, identity equality, and `Send`
and `Sync`. Tested with a recording allocator that checks every free's pointer and layout, counts allocations against frees
and poisons a freed block: the last drop frees once and drops the error once, on another thread too; clones dropped
concurrently free once; a failing allocator returns the error undropped; a zero-sized and an over-aligned error;
downcasting; and a source delegated.

What was done. [shared.rs](src/shared.rs) holds `SharedError`, `try_new` and `try_new_in`, and `ptr_eq`; the one unsizing is
a `NonNull` coercion inside `try_new_in`. The tests' `Recording` allocator ([tests.rs](src/shared/tests.rs)) is leaked once
per test, so no two tests share its counts. Beyond the item's list, the tests cover the global variant over an
`io::Error`, a `SharedError` boxed as a source, a hundred errors of two layouts from one allocator, and, at compile time,
`Send`, `Sync` and the type being two words; the type's doc example is a doctest. Three sabotages, each made and reverted
alone, were caught: freeing when the count reached 2 (eight tests, one of which read the poison back as `probe 3722304989`,
`0xDDDDDDDD`); a clone that did not increment; and freeing with the error's layout instead of the block's, which the
recording allocator refused. The manifest entries are RCE-1.4's.

## Moved 2026-10-23 19:11:50 -04:00 -- RCE-1.3: the static error and the report

### <a id="rce-13"></a>RCE-1.3 -- The static error and the two-form report, comparing by identity, with README examples compiled as doctests. *(completed 2026-10-23 19:11:50 -04:00)*

Per [RCE-D-6](DESIGN-NOTES.md#rce-d-6) and [RCE-D-7](DESIGN-NOTES.md#rce-d-7). The item as it stood at completion:

**The static error and the report**: a `const`-constructible static error holding a message; the two-form report, `Clone`,
dereferencing to `dyn Error + Send + Sync`, comparing by identity and never equal across forms. The README gains examples of
both, compiled as doctests through the crate's `#![doc = include_str!("../README.md")]`.

What was done. [static_error.rs](src/static_error.rs) holds `StaticError` (`new` and `message`, both `const`), whose own
equality compares messages; [report.rs](src/report.rs) holds `ErrorReport`, with `from_static` (`const`) and `From` for
both forms. The README gains two examples, a static and a shared report side by side, and an allocator of the caller's own
through `try_new_in`. One defect was found by its test before commit: the report's `Deref` returned the `SharedError` coerced
to `dyn Error` -- it is an `Error` itself -- so downcasting saw the `SharedError` and not the error it holds. It now derefs
through it, and both `SharedError`'s and `ErrorReport`'s documentation say that each is an `Error` itself and is downcast
through `Deref`. The docs also say a zero-sized static's address may be shared with another's, which identity rests on.

## Moved 2026-10-23 19:34:03 -04:00 -- RCE-1.4: sabotage entries, and RCE-M1 complete

### <a id="rce-14"></a>RCE-1.4 -- A sabotage manifest: eight defects the tests catch, two orderings no test can see recorded as surviving, and one control. *(completed 2026-10-23 19:34:03 -04:00)*

Per [RCE-D-8](DESIGN-NOTES.md#rce-d-8). The item as it stood at completion:

**Sabotage entries** in a `sabotage.json` for the free condition, the count's increment and the layout the free uses, each
shown caught; and for the decrement's `Release` and the fence's `Acquire`, recorded as surviving.

What was done. [sabotage.json](sabotage.json) holds eleven entries, beyond the item's five: the error never dropped, the
three report defects RCE-1.3 could make -- the deref to the wrapper, which RCE-1.3 found before commit, and identity
weakened across forms and between statics -- the zero-sized block's alignment, and a control, a clone counting with
`SeqCst`, which is slower and not wrong. A full sweep behaved as declared in every entry. The wrong-layout entry ends the
suite with `0xC0000409` rather than a test failure: the recording allocator's assertion fires inside `Drop`, so the panic
aborts the process. It fires every time; it is an assertion, not a corruption that happened to crash, and a single test run
of the same mutation shows its message. The manifest is found by CI's sweep planner through its `crates/*/sabotage*.json`
glob, so nothing registers it.

<a id="rce-m1-complete"></a>**RCE-M1 is complete**, and with it [CHECKLIST.md](CHECKLIST.md): its stubs are deleted, the
plan is recorded in [COMPLETED-PLANS.md](COMPLETED-PLANS.md), and the next work is durable-ioring's `DI-23.4.2`.
