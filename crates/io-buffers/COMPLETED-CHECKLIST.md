# Completed checklist: io-buffers

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-12 20:46:32 -04:00 -- IB-1.1: the contract, moved from windows-ioring-sys

### <a id="ib-11"></a>IB-1.1 -- The buffer contract in a portable crate: `IoBuf`, `IoBufMut`, `RegisteredSpan` and the standard-type implementations, copied from `windows-ioring-sys` with their tests. *(completed 2026-10-12 20:46:32 -04:00)*

Per [IB-D-2](DESIGN-NOTES.md#ib-d-2). Built by copying the ring crate's `buf.rs` and its tests, and
`RegisteredSpan` from its `batch.rs`, so each file's header names its source and the commit it was
copied at. **The obligations are unchanged; the words that named the holder are not.** The ring
crate's text said the promise binds "for as long as this crate holds the value", and linked its own
`IoRing::try_pop` and milestones; in a crate no ring owns, the holder is "the I/O layer the value was
handed to", and the module documentation, the two `# Safety` sections and the one test comment that
named the ring crate say so. The three bullets of `IoBuf`'s obligation and `IoBufMut`'s additions are
word for word, except that "this crate does not track an initialized prefix" now says "the I/O layer". `RegisteredSpan`'s field documentation no longer cites the ring's
`RegisteredBuffers::get`, and its derives are the ring's: `Clone`, `Copy`, `Debug`.

What the item said that was not so: it expected sabotage entries that pinned the originals to move
with them, and the ring crate has none for `buf.rs`; the ring crate's own record is that mutation
testing found the null-address case there. So this crate has a manifest of its own,
[sabotage.json](sabotage.json), pinning each implementation's address and length separately, and its
control survives as it should. A run of `cargo-mutants` was started and abandoned: it copies the
whole repository, `.scratch/` included, which holds tens of gigabytes of earlier sweep working
copies, and would not have finished usefully.

The README gained a doctest showing a caller-defined buffer. The ring crate is untouched, as the
item required; `IB-1.3` replaces its pair. The commit carries `Release-As: 0.1.0`.

The item as it stood at completion:

- [x] **IB-1.1** -- **The contract, moved from `windows-ioring-sys`**
  ([IB-D-2](DESIGN-NOTES.md#ib-d-2)): `IoBuf`, `IoBufMut` and `RegisteredSpan`, with the
  implementations for `Vec<u8>`, `Box<[u8]>`, `Arc<[u8]>` and the static slices, and their
  documentation and tests, moved by copy so the obligations keep their text. The ring crate is
  untouched by this item: its own pair stays until `IB-1.3` replaces it, so every commit builds.
  Tests are the ring crate's, moved, plus any the move shows to be missing, and run in the
  `portable` CI job beside `wtf-string`. The sabotage manifest entries that pinned the originals
  move with them. The commit carries `Release-As: 0.1.0`, since release-please attributes a commit
  by the paths it touches and `Cargo.toml` says 0.1.0.

## Moved 2026-10-12 21:06:14 -04:00 -- IB-1.2: NumaBuffer's buffer impls, in win-numa-sys

### <a id="ib-12"></a>IB-1.2 -- `NumaBuffer` implements `io-buffers`' `IoBuf` and `IoBufMut`, in `win-numa-sys`, with the ring crate's tests and safety argument. *(completed 2026-10-12 21:06:14 -04:00)*

Per [IB-D-4](DESIGN-NOTES.md#ib-d-4) and `win-numa-sys`'s
[N-D-2](../win-numa-sys/DESIGN-NOTES.md#n-d-2). Built by copying the ring crate's
`numa_buffer_io.rs` and its tests into `win-numa-sys` as `io_buf.rs`, with the two implementations'
safety comments unchanged and the module documentation rewritten to say why they are here: a trait
may be implemented only by the crate that owns it or the crate that owns the type, `io-buffers` is
portable and cannot depend on a Windows allocator, so the type's crate does. The ring crate is
untouched and still implements its own trait for `NumaBuffer`, so every commit builds. `win-numa-sys`
gained a dependency on `io-buffers`, written as `path` with `version = "0.1.0"`, which `IB-1.1`'s
`Release-As` makes a fact and not a guess. The crate's README, its type documentation and its crate
documentation, which said it implements no buffer trait, now say what it does. One test was added to
the moved four, showing a `NumaBuffer` satisfies the traits as a generic bound, which is how a ring
takes it.

**The item's plan changed in one respect:** it said `IB-1.3` would use a path-only dependency
"deliberately", as `durable-ioring` does. That reasoning does not carry over: `durable-ioring` has no
fixed first version to write, and this crate's is fixed. `IB-1.3`'s text now says so.

`win-numa-sys` had no sabotage manifest; it has one now, covering only these implementations. Three
entries are caught and a control survives. Two are scored as a process abort, since the test that
reads through the pointer builds a slice from a null one and Rust's precondition check aborts, which
hides the deterministic catch: the correspondence test fails an assertion by itself. The manifest says
so at each.

The item as it stood at completion:

- [x] **IB-1.2** -- **`NumaBuffer`'s buffer impls move to `win-numa-sys`**
  ([IB-D-4](DESIGN-NOTES.md#ib-d-4)): `win-numa-sys` depends on `io-buffers` and implements
  `io_buffers::IoBuf` and `IoBufMut` for `NumaBuffer`, with the safety argument the ring crate's
  `numa_buffer_io.rs` carries. Additive, so it builds beside the ring crate's own implementation of
  its own trait. The allocator crate's README, which says it defines no buffer trait, says what it
  now does instead.

## Moved 2026-10-12 21:15:48 -04:00 -- IB-1.3: windows-ioring-sys takes the contract

### <a id="ib-13"></a>IB-1.3 -- `windows-ioring-sys` takes the buffer contract from `io-buffers` and re-exports it at the same paths. *(completed 2026-10-12 21:15:48 -04:00)*

Per the ring crate's [D-89](../windows-ioring-sys/DESIGN-NOTES.md#d-89). The ring crate depends on
`io-buffers` (`path` with `version = "0.1.0"`, which `IB-1.1`'s `Release-As` makes a fact), its `buf`
module, its `RegisteredSpan` definition and its `numa_buffer_io` module are deleted with their tests,
and `lib.rs` re-exports `IoBuf`, `IoBufMut` and `RegisteredSpan` from `io-buffers`, so every path a
consumer bound to still resolves. `batch.rs` imports them from the new crate. The ring crate's unit
and integration tests, its doctests and `durable-ioring`'s all pass, with all features; clippy and
rustdoc `-D warnings` are clean. Its sabotage manifest named nothing in the deleted code, so nothing
needed re-anchoring. The ring crate's decisions `D-1` and `D-11`, which describe the duplication and
the registration extension, carry a marker pointing at where the contract lives now, and `D-89`
records the move.

**Not a breaking change**, which the work was checked against rather than assumed: the types are
reached by the same paths, an `unsafe impl` of the trait for a consumer's own type compiles as it did
(the tests of `durable-ioring`, which have several, are the check), and the one real consequence is a
new dependency. The commit is therefore `refactor`, which release-please does not count as a feature.

The ring crate's `MUTATION-SURVIVORS.md` still names `buf.rs` lines, which is an old report of a
sweep that no longer applies to this file; it is left as the record it is.

The item as it stood at completion:

- [x] **IB-1.3** -- **`windows-ioring-sys` takes the contract from `io-buffers`**: its `buf` module and
  `RegisteredSpan` definition are deleted, `IoBuf`, `IoBufMut` and `RegisteredSpan` are re-exported
  at the same paths so no consumer's import changes, and `numa_buffer_io.rs` is deleted, its
  implementations having moved in `IB-1.2`. The ring crate's tests and sabotages stay green, with
  those that pinned the deleted code re-anchored or retired. **The dependency is `path` with
  `version = "0.1.0"`**, which is not a guess: `IB-1.1`'s commit fixes this crate's first release
  to 0.1.0 with `Release-As`. This crate must be published before the ring crate's next release,
  which the publish workflow's wait for workspace siblings exists for. (`durable-ioring`'s
  path-only dependencies are different: it has no `Release-As` to rely on, and `DI-12.4` is where
  it pins them.)

## Moved 2026-10-12 21:26:37 -04:00 -- IB-1.4: an owned, aligned buffer

### <a id="ib-14"></a>IB-1.4 -- An owned, aligned buffer, `AlignedBuf`: fallible to build, zeroed, fixed in length, implementing `IoBuf` and `IoBufMut`. *(completed 2026-10-12 21:26:37 -04:00)*

Per [IB-D-3](DESIGN-NOTES.md#ib-d-3) and [IB-D-6](DESIGN-NOTES.md#ib-d-6), which records the interface
the item left to be settled: construction returns `Result<_, AlignedBufError>`, with three reasons --
a bad alignment, a length no allocation can have, and an allocator that cannot provide it -- where
`windows-namespace-request-sys`' `AlignedBuffer` panics and aborts; there is `try_clone` and no
`Clone`; the length is fixed; and the bytes are zeroed or copied, never uninitialised. It derefs to
`[u8]`, its `Debug` shows the length and alignment and never the contents, and equality is by contents
and alignment. A length of zero allocates nothing and still has an aligned, non-null address. The
README and the type's documentation each carry a doctest.

**Tests** cover every alignment from one to 64 KiB against nine lengths, including zero, one, awkward
and not a multiple of the alignment; zeroing against an allocator reusing a block just filled with
`0xAB`; the copy, the clone and equality; each error with its numbers, and the allocator-failure arm
reached by asking for the largest length that fits an `isize`; the traits as a bound; the address
across a move and across a collection growing; and use from another thread and from several. **The
test binary installs a counting allocator**, per thread, because a missed free is invisible to every
other assertion: three tests read it, for a drop, for a clone, and for the empty and the failed
cases. The sabotages are all caught, one of them the leak, and the controls survive; two of the caught
are scored as a process abort, which the manifest says is a weaker catch.

The item's open points, answered: a bad alignment, an overflowing length and an allocation failure
are reported as the three errors; it cannot grow; `Debug` shows the shape, not the contents; and
there is no `Clone`. The assistant's choices are queued in `DI-9.7`. The crate's two parked decisions,
`IB-2+.1` and `IB-2+.2`, remain.

The item as it stood at completion:

- [x] **IB-1.4** -- **An owned, aligned buffer** ([IB-D-3](DESIGN-NOTES.md#ib-d-3)): an allocation
  whose alignment the caller chooses at construction, zeroed or copied from bytes, implementing
  `IoBuf` and `IoBufMut`, `Send` and `Sync`, with no `unsafe` needed to construct or use it. The
  interface is this item's to settle: how a bad alignment, an overflowing length and an allocation
  failure are reported (`windows-namespace-request-sys`' `AlignedBuffer` panics on a bad alignment,
  which a buffer built by a consumer from a target's declared geometry would rather receive as an
  error), whether it can grow, and what its `Debug` and `Clone` say. Tests include every alignment
  from one to a page, lengths of zero and not a multiple of the alignment, the allocation's address
  checked, and the drop path. Gated on `IB-1.1`; independent of `IB-1.2` and `IB-1.3`.

## <a id="ib-m1-complete"></a>IB-M1 is complete

`IB-1.1` to `IB-1.4` are archived above under their own anchors. The milestone heading and its stubs
left [CHECKLIST.md](CHECKLIST.md) with this item, which finished it. The cross-component handoffs the
stubs carried -- to `windows-ioring-sys`' `IB-1.3` after `IB-1.2`, and to `durable-ioring`'s `DI-8.2`
after `IB-1.3` -- are discharged by the items they named, and `DI-8.2`'s own prerequisite note stays in
[its checklist](../durable-ioring/CHECKLIST.md).

## Moved 2026-10-12 21:31:29 -04:00 -- IB-2+.1: windows-overlapped-io-sys adopts the shared traits

### <a id="ib-2+1"></a>IB-2+.1 -- `windows-overlapped-io-sys` adopts the shared traits and re-exports them at the same paths. *(completed 2026-10-12 21:31:29 -04:00)*

Per [IB-D-5](DESIGN-NOTES.md#ib-d-5). The item was parked as a decision for the engineer; "implement
ib-2+.1" is taken as that decision to adopt. The overlapped crate depends on `io-buffers` (`path` with
`version = "0.1.0"`), its `buf` module -- the two traits, the standard-type implementations and their
tests -- is deleted, and `lib.rs` re-exports `IoBuf` and `IoBufMut` from `io-buffers`, so every
`windows_overlapped_io_sys::IoBuf` a consumer bound to still resolves. What was in `buf.rs` besides the
traits was `OversizedBuffer`, a test buffer reporting a length no `u32` can express; it kept its text in
a test-only `oversized_buffer` module, and the two tests that use it name the new path. The overlapped
crate's own tests of the standard-type implementations are `io-buffers`' now, which carries the same
set copied from the ring crate.

**The cost the item named was checked, not assumed:** a type implementing both crates' traits would
have two implementations of one trait. Every dependent was compiled with all features and all targets
-- the ring crate, `windows-file-watcher`, `windows-threadpool-sys` -- and none did. It is still a
break for a downstream crate that does, which is why the commit is marked breaking.

**The check that the two crates now name one trait** is
`windows-ioring-sys/tests/shared_buffer_contract.rs`: a type implementing only `io-buffers`' traits
satisfies both crates' bounds, and so do the shared aligned buffer and `Vec<u8>`. It is a claim about
a trait being the same trait, so it is made for the compiler -- if either crate defined its own again
the test would stop compiling, which is a stronger rung than an assertion. It sits in the ring crate
because that crate already has the overlapped crate as a dev-dependency. The assistant did not run the
test against the pre-change tree to see it fail; that it would is by construction, since a type
implementing only the shared traits could not satisfy a crate-local bound.

The overlapped crate's all-features unit, integration and doc tests pass, its clippy and rustdoc
`-D warnings` are clean, and the ring crate's whole suite passes. Its `DESIGN-NOTES.md` says the traits
moved, the ring crate's `D-1` and `D-89` say the duplication is resolved, and `IB-D-5` is rewritten
from a deferral to a record of the decision.

The item as it stood at completion:

- [x] **IB-2+.1** -- **Whether `windows-overlapped-io-sys` adopts the shared traits**
  ([IB-D-5](DESIGN-NOTES.md#ib-d-5)): re-export them at the same paths, as the ring crate does in
  `IB-1.3`. A type that today implements both crates' traits would then have two implementations of
  one trait, so this is a decision on that cost and not a mechanical step.

## Moved 2026-10-12 21:55:30 -04:00 -- IB-2+.2: PageBuffers on AlignedBuf

### <a id="ib-2+2"></a>IB-2+.2 -- `windows-overlapped-io-sys`' `PageBuffers` holds an `AlignedBuf`. *(completed 2026-10-12 21:55:30 -04:00)*

`PageBuffers` is now a page count over an `AlignedBuf` of `pages * PAGE_SIZE` bytes at `PAGE_SIZE`
alignment. Its own `alloc_zeroed` and `dealloc`, raw pointer, and manual `Send`, `Sync` and `Drop`
are gone, and its `IoBuf` and `IoBufMut` impls delegate to the buffer. The public API is unchanged,
so the commit is a `refactor`, not a breaking change. One behaviour moved: an allocation failure
that aborted through `handle_alloc_error` is now a panic, which `new` already was for zero pages or
an overflow.

The type had almost no direct tests of its own (its only coverage was through the scatter and gather
I/O tests), so the item added tests for the page arithmetic, alignment and zeroing across page
counts, the segment array and its terminator, shared views, move stability, the two panics, and
`Send`, `Sync` and `Debug`, with a sabotage manifest (`crates/windows-overlapped-io-sys/sabotage.json`)
whose four sabotages are caught and whose control survives.

The item as it stood at completion:

- [x] **IB-2+.2** -- **`windows-overlapped-io-sys`' `PageBuffers` holds an `AlignedBuf`.** An internal
  swap: it has its own `alloc_zeroed` and `dealloc` at `PAGE_SIZE`, its own `Send`/`Sync` and both
  trait impls, and exposes only `pages`, `len`, `as_bytes`, `as_bytes_mut` and the segment array, none
  of which changes. Not a breaking change: `new` still panics on zero pages or an overflow, and an
  allocation failure that aborted through `handle_alloc_error` becomes a panic through the buffer's
  error. The page-aligned buffer is what `ReadFileScatter` and `WriteFileGather` need, and its tests
  are real unbuffered I/O.

## Moved 2026-10-12 22:00:30 -04:00 -- IB-2+.3: the ring crate's test buffers on AlignedBuf

### <a id="ib-2+3"></a>IB-2+.3 -- The ring crate's three test-local `Aligned` buffers become `AlignedBuf`. *(completed 2026-10-12 22:00:30 -04:00)*

`handover.rs`, `flush_barrier.rs` and `flush_barrier_stress.rs` in `windows-ioring-sys`' `tests/`
now build their unbuffered-I/O buffers with a small `aligned` helper over `AlignedBuf` at the
4096-byte sector alignment, and the three hand-aligned structs and their `unsafe impl`s are gone.
The two files that fill the buffer use `AlignedBuf`'s `DerefMut`. The stress file, which is
`#[ignore]`d, was compiled by the all-targets gate and two of its instruments were run explicitly
with `IORING_STRESS_TRIALS=5`; both passed. The spike under `design-sessions/spikes/` keeps its own
copy, as a frozen record.

A check made along the way: with the helpers' alignment lowered to one byte, the unbuffered tests
still passed on this host, so on this machine they do not discriminate alignment. That was equally
true of the structs they replaced, and is recorded here only so the tests are not read as pinning
the alignment; `AlignedBuf`'s own tests do.

The item as it stood at completion:

- [x] **IB-2+.3** -- **The ring crate's three test-local `Aligned` buffers become `AlignedBuf`.**
  `handover.rs`, `flush_barrier.rs` and `flush_barrier_stress.rs` each over-allocate a `Vec<u8>` and
  offset into it, with an `unsafe impl` of the traits and the reason in a comment: a hand-aligned
  allocation cannot be freed under the right layout, which `AlignedBuf` does by allocating with the
  layout. Test-only, and removes three `unsafe impl`s. Two need a fill byte, which `AlignedBuf`'s
  `DerefMut` gives. `handover.rs` and `flush_barrier.rs` run by default; the stress file is
  `#[ignore]`d and has to be run explicitly to be checked.
