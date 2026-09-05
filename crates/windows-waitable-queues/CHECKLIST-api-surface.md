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

- [ ] **API-1** -- Make `pop` distinguish an empty queue from a departed
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

- [ ] **API-2** -- Put `is_full` on the `Bounded` trait, so it is reachable
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

- [ ] **API-3** -- Add `try_iter` and `IntoIterator` on the consumers, keeping
  `drain` as the name that describes the semantics.

  `drain` is exactly `try_iter` -- take what is there, stop at the first empty
  -- but `drain` is the one-crate spelling (flume) while `try_iter` is the
  four-crate one, and `IntoIterator` on the receiving handle is four-crate as
  well. Neither costs anything to add.

  It is also trait-only today, so `rx.drain()` does not compile without
  `use windows_waitable_queues::Consumer`. Make the iterator reachable from the
  concrete types.
