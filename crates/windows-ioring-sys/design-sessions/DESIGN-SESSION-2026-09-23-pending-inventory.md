# Design session -- the pending-token inventory (2026-09-23)

**Summary.** An exploration of `M23.3`, which asks whether this crate should offer a
pending-operations map and a slot arena over it. The session produced a working spike
(`src/pending.rs`), three measured findings that falsified earlier claims -- two of them
mine, stated confidently and wrongly -- and a design alternative I dismissed on a reason
that turned out not to hold. No decision is taken here; `M23.3` still owns that.

## What the checklist said, and what a census found

`M23.3` recorded that nine sites keep a map from `UserData` to an unclaimed `Token`. A
fresh census found otherwise:

- **There are about twelve**, not nine. The list missed `model_a_delivery.rs`,
  `model_b_multiplexed.rs` and `generated_sequences.rs`.
- **Only about a third keep the bare map described.** The rest carry per-operation
  sidecar data -- a slot index, an expected length, a phase, a sequence number, a round.
  `flush_barrier_stress.rs` keeps two sidecar maps beside its tokens.
- **Exactly one site needs synchronisation**: `model_a_delivery.rs` wraps its map in a
  `Mutex`, because it is the Model A path where completions land on pool threads.

The count was the item's main evidence, and it was taken before `M24` relocated eleven
tests. **The duplicated thing is not the map**; it is the claim discipline over a map
whose value type differs at nearly every site. That is why the spike is `Pending<T, X>`
and not `Pending<T>` -- the generic is a finding, not a convenience.

## The two types already existed, related by hand

`RingContract` is public, always on, and not feature-gated. Every consumer that uses it
drives it *in parallel with* its own map:

```text
self.contract.observe_push(token.id());
self.in_flight.insert(token.id(), InFlight { token, slot });
```

The same event, recorded twice, by hand, at every site -- a restatement in the
repository's own terms, and one that can drift in both directions. The oracle's value
depends on being driven correctly by the very code it exists to check.

So the pair the engineer was reaching for -- "if the rule is heavy, can we have two
related types?" -- already exists. What was missing is that the light one should
**drive** the heavy one, so a consumer updates one thing and both stay true.

## Synchronisation: no, and the reason is already paid for

`pop_within` takes `&mut self`, and `IoRing` is `Send` but **not `Sync`** -- there is only
`unsafe impl Send for IoRing`. Whoever pops a completion already holds exclusive access,
so a map reachable through that same `&mut self` needs no `Arc`, no `Mutex`, and no
interior mutability. The exclusivity exists already.

## Falsified: the type-erasure objection

**This was my claim and it was wrong.** I argued a map owned by the ring would have to
store heterogeneous `Token<T>`, forcing `Box<dyn Any>` and a downcast at the claim site,
and that this killed the idea. The engineer asked why, since for any particular ring the
type is fixed. Checking:

- **Per-ring monomorphisation holds for every real consumer.** The epoch-log's log ring
  carries `Token<RegisteredUse>` for appends, and its commits are *tokenless*.
  `checkpoint.rs` looked like a counterexample because it has two maps, but its local
  `Pending` is plain bookkeeping and only one map holds tokens.
- **Where it genuinely does not hold, the answer is a closed enum, not `dyn Any`** -- and
  this tree already has one. `generated_sequences.rs` puts eight token types on a single
  ring behind `enum Held`, with an exhaustive `match` in `claim` and no runtime type
  check at all.

So a generic `IoRing<T>` owning the map is viable, which matters because it is the shape
that would make the ring *notify* rather than be told -- and drift between the ring and
the inventory structurally impossible rather than merely discouraged. Its real costs are
different from the one I asserted: `IoRing` is not generic today, so this is a breaking
change to a published crate; a consumer mixing shapes writes a `Held`-style enum; and
tokenless pushes still need a story.

## Why `commit.rs` uses `flush_raw`, and what follows

Asked during the session, and the answer explains the tokenless commits above.

The safe `flush<F: FileTarget>(&F, ..) -> Token<F::Guard>` requires an **owned, guarded**
file -- `SharedFile` is `Arc<OwnedHandle>`, and the token holds a clone of that `Arc`,
which is what keeps the handle alive for the kernel. `Committer::commit` is handed a bare
`RawHandle` that the log's `File` owns, so it cannot build a `SharedFile` without taking
ownership and closing the log's handle out from under it. It therefore takes the `unsafe`
`flush_raw`, whose safety argument is hand-written: *"`file` is the log's own handle and
outlives every operation pushed here; the log drains to empty before it closes."*

**Why that makes the commit tokenless, precisely.** A write's token guards the *buffer*,
so `write_registered_raw` still returns one even with a raw file. A flush has no buffer --
its only possible guard is the file -- so with a raw handle there is nothing to hold, and
`flush_raw` returns a bare `usize`.

Three implications:

1. **The commit path cannot be in any token inventory as currently plumbed.** Not because
   flushes are special, but because this sample passes a borrowed handle.
2. **A compile-time guard was traded for a prose argument**, and the prose is load-bearing:
   nothing enforces "the log drains to empty before it closes".
3. **It is the same shape as `placement.rs`'s constraint**, found earlier the same day: a
   borrowed `RawHandle` and an owned handle make different APIs reachable. Two independent
   places in one sample reach for a lower-level call for the same plumbing reason, which
   suggests the plumbing rather than the calls is the thing to look at -- and `M25.3`
   already reopens how the log is opened.

## What the spike established, and what it cannot

**Established, by test and by sabotage:**

- One call site keeps the map and the oracle in step. Cutting the wiring is caught.
- An unclaimed token is loud at teardown rather than a silent deliberate leak. Removing
  the `Drop` guard is caught.
- `Pending<T, X>` fits a real consumer: converting `append.rs` removed its `InFlight`
  struct and its hand-driven `observe_completion`/`observe_claim` pair.
- The `M22.2` ordering defect is now caught by an assertion, via a test that drives a
  failed write through the injection seam.

**Not established, and two of these are corrections to things I asserted:**

- **The ring does not notify anyone.** `Pending` is consumer-driven. Nothing forces a
  minted token into the inventory, so a consumer can take a `Token` from `Batch` and never
  register it, and no type, test or oracle notices. The spike removed drift between the map
  and the oracle; it did not remove drift between the ring and the map.
- **One consumer is converted, not twelve.** This is a worked example, not a property of
  the crate.
- **`Pending::checked()` owning the oracle creates a decoy hazard.** A consumer that
  already had a `RingContract` keeps a field that is never written to again. The
  conversion did exactly that, and its teardown `assert_quiescent()` passed *vacuously* --
  compiled, ran, every test green. Sabotage confirms nothing catches it. Found by reading.
- **Ring teardown interaction is untested.** If a `Pending` and an `IoRing` both drop with
  operations outstanding, the ordering of their guards is unexamined -- and the session
  already found one drop-order surprise (see below).

## Two findings about the instruments themselves

**A feature-gated test is invisible to the sabotage harness.** The sweep first reported the
`M22.2` regression as *survived*. The new tests are behind `fault-injection`, off by
default, so the harness compiled them out and the sabotage landed in code nothing
exercised. Fixed with a manifest `testArgs` carrying `--all-features`. This is the trap the
repository already documents for cargo-mutants, arriving through a different tool: a
feature-gated guard and an absent guard are indistinguishable to a runner that does not
enable the feature.

**A failing test that leaves registered buffers outstanding aborts instead of reporting.**
`RegisteredBuffers::drop` refuses to free while operations are outstanding -- correct, from
`M5.3` -- via a bare `debug_assert!` that does not check `std::thread::panicking()`. So the
assertion fires and names the leak, then unwinding drops the arena, the `debug_assert`
panics during unwind, and the process aborts with `STATUS_STACK_BUFFER_OVERRUN`. Detection
is not weakened; the report is. Queued as `M23.4`.

## On requiring `finish`, and why it is not failable

Rust has no linear types, so nothing can force a method call on a value the caller owns.
Three rungs were considered:

- **`#[must_use]`** stops the return value being ignored, not the call being skipped.
- **The drop bomb** -- `Drop` panics when tokens are still held -- is the real enforcement,
  and is what the spike implements. It is suppressed while already panicking, because a
  second panic during unwind aborts and replaces the original failure.
- **A scoped constructor** (`Pending::scope(|p| ...)`) would genuinely force it, since the
  consumer never owns the value. Not built: it imposes a control-flow shape that suits the
  appender but not the tests that thread a map through several helpers.

**`finish` reports rather than fails**, and the engineer's observation is why: it *consumes*
the map, so an `Err` would leave nothing to retry with. A consuming method in a linear
discipline is normally total for exactly that reason. `-> Result<(), _>` would buy
`#[must_use]` from the language rather than from an attribute, at the cost of implying a
recoverable state that does not exist; the spike keeps `-> Vec<Violation>` with the
attribute.

The residual worry -- that a path is found so late that a drop bomb ships -- is real but
narrower than it looks. A failed completion is not unreachable, only untested, and
`Completion::with_injected_failure` reaches it on demand. What remains is the set of
conditions the seam cannot manufacture, which is worth naming rather than treating the
whole class as unreachable.

## Open, for `M23.3` to decide

1. Public type, documented pattern, or `test-util` module. Six of the twelve sites are
   tests, and test convenience is a weak reason to grow permanent surface.
2. Whether `checked()` survives in its current form, given the decoy hazard nothing catches.
3. Whether the stronger shape -- a generic `IoRing<T>` owning the map, so the ring notifies
   -- is worth a breaking change to a published crate.
4. What it refuses to decide. Batching, ordering and slot choice are caller questions; the
   sharper refusal is that the map does not decide whether you are checked.
