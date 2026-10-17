# Completed checklists: windows-waitable-queues

Append-only. Newest groups at the bottom.

## Moved 2026-09-05 -- public API surface, closed against eight published queue crates

# Checklist: public API surface

Closes the gaps found by comparing this crate's public surface against the eight
most-depended-on Rust queue crates: `crossbeam-channel`, `crossbeam-queue`,
`flume`, `thingbuf`, `ringbuf`, `rtrb`, `concurrent-queue`, and
`std::sync::mpsc`.

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## What the comparison established

Of seventeen capabilities present in three or more of those eight, this crate
already has nine, under the majority spelling in every case (`len`, `capacity`,
`is_empty`, `remaining`). The items below are the gaps that a reader coming from
any of those crates would notice. Deliberately **not** queued, because the
comparison showed them to be one- or two-crate features rather than
expectations: an explicit `close()` operation (1/8), `peek` (2/8, and no MPMC
crate offers it), bulk and slice transfers (2/8, both single-producer designs),
async (2/8), `force_push` (2/8), `sender_count` (1/8), and weak handles (1/8).

**These are breaking changes, and that is why they are queued now.** The crate is
unpublished at `0.0.1`, so the cost of making them is zero and it will not be
zero again.

## M1: the surface

- [x] **API-1** -- Make `pop` distinguish an empty queue from a departed
  producer, by returning `Result<T, TryRecvError>` with `Empty` and
  `Disconnected` variants rather than `Option<T>`.

  **The strongest signal is internal, not comparative.** `push` already
  distinguishes `PushError::Full` from `PushError::Disconnected`, and `recv`
  already returns `RecvError::Disconnected`; only `pop` collapses the two. The
  crate has the vocabulary and one method declines to use it.

  Five of the eight crates distinguish these. The three that do not --
  `crossbeam-queue`, `rtrb`, `ringbuf` -- have **no handles and no
  disconnection concept at all**, so they have nothing to distinguish. This
  crate is channel-shaped: handles, `Drop`-based disconnection, and
  `is_disconnected` on both sides. The shape implies the expectation.

  It also removes a protocol the caller is currently asked to remember. The
  `Consumer` trait documents: *"Ask only after `pop` has returned `None`.
  Draining to empty and then finding the producers gone is the only order that
  cannot lose an item."* That is a `TryRecvError` written as prose, and prose
  cannot be enforced.

- [x] **API-2** -- Put `is_full` on the `Bounded` trait, so it is reachable
  generically and from a consumer.

  Every concrete `Producer` answers `is_full` as an inherent method, but the
  trait does not declare it, so generic code over `Bounded` cannot ask and no
  `Consumer` offers it. The two surfaces disagree about which questions exist:
  `remaining` is on the trait and therefore works on every consumer, yet is
  spelled out inherently only on `reserving_mpsc`'s.

  Six of the eight crates offer `is_full`. Give it a default implementation in
  terms of `remaining`, so no shape has to restate it, and add the inherent
  method to each `Consumer` for the same reason the other accessors are
  inherent: a caller should not need an import to ask.

- [x] **API-3** -- Add `try_iter` and `IntoIterator` on the consumers, keeping
  `drain` as the name that describes the semantics.

  `drain` is exactly `try_iter` -- take what is there, stop at the first empty
  -- but `drain` is the one-crate spelling (flume) while `try_iter` is the
  four-crate one, and `IntoIterator` on the receiving handle is four-crate as
  well. Neither costs anything to add.

  It is also trait-only today, so `rx.drain()` does not compile without
  `use windows_waitable_queues::Consumer`. Make the iterator reachable from the
  concrete types.

## Moved 2026-10-17 19:52:24 -04:00 -- WQ-1: a capacity the shapes accept no longer aborts the process when memory cannot hold it

### <a id="wq-1"></a>WQ-1 -- Every bounded shape allocates its ring fallibly, and a refusal is a `CapacityError` that says it was the memory. *(completed 2026-10-17 19:52:24 -04:00)*

Found by a consumer: `win-multi-file-tail-watcher` accepted a `client_sq_capacity` of `1 << 40`, which is a
valid `slotwise_mpsc` capacity, and the process aborted inside `Vec::with_capacity`. Decision:
[D-42](DESIGN-NOTES.md#d-42).

`capacity::allocate_slots` is the one place a ring is allocated, through `try_reserve_exact`, and `spsc`,
`slotwise_mpsc`, `permit_mpsc` and `reserving_mpsc` (all three layouts) call it. A refusal is a new kind of
`CapacityError` read through `is_allocation_failure`; `previous_valid` and `next_valid` are `None` for it.
The type is still `Copy` and its fields are still private, so the addition is not a breaking change. The `# Errors`
sections of the four `bounded` functions and `CapacityError`'s own docs state it.

Tests: the helper makes slots in order and makes none when the memory is refused; each shape's maximum capacity is
refused (a size past `isize::MAX`) and so is 2^31 slots of 1 MiB (a size the allocator is asked for and declines,
64-bit only); the refusals that are a mistake in the request do not report themselves as allocation failures.

Sabotage: entries in `sabotage.json` and a second manifest, `sabotage-experimental.json`, for `permit_mpsc`,
which the default manifest cannot reach because it runs without `experimental-permit-claim` -- a first attempt put
the permit entry in the default manifest and it survived, which is how that was found. One entry swaps the fallible
call for `reserve_exact` and is caught by the process aborting; that exit is deterministic here (the allocator's
refusal is the same every run), unlike the heap-corruption crashes the sweep's notes warn about. A control that swaps
`try_reserve_exact` for `try_reserve` survives in both manifests, as it should: the two refuse the same requests.

Not done here, and queued: the consumer's side, which maps the new error and tests it
([TW-9.6](../win-multi-file-tail-watcher/CHECKLIST.md)), waits on a release of this crate.