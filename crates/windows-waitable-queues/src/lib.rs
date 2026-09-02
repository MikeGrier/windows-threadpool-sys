// Copyright (c) Mike Grier.

//! Bounded producer/consumer queues whose readiness is a waitable Windows
//! `HANDLE`.
//!
//! **Windows only.** Every public item is behind `cfg(windows)`; the crate
//! builds to an empty shell on other platforms.
//!
//! # Why this exists
//!
//! There are good concurrent queues for Rust already. What none of them offers
//! on Windows is the one property this crate is named for: **you cannot wait on
//! them alongside a kernel object.**
//!
//! `crossbeam-channel` blocks in `recv`, but parks on its own internal
//! primitive and exposes no `HANDLE`; its `Select` is built purely from channel
//! operations, with no way to register a foreign OS object.
//! `crossbeam-queue` does not block at all. So a thread that must wake on
//! "a message arrived **or** my I/O completed **or** shutdown was signalled"
//! cannot express that wait, and must poll one source while blocking on
//! another -- which either burns a core or adds latency.
//!
//! On Windows a `HANDLE` is the universal waitable currency:
//! `WaitForSingleObject`, `WaitForMultipleObjects`, `MsgWaitForMultipleObjects`,
//! a thread-pool wait, and alertable waits all take one. So a queue whose
//! readiness *is* a `HANDLE` composes with everything the platform can wait on,
//! and one that hides its readiness behind a private primitive composes with
//! nothing.
//!
//! # What is here
//!
//! A family of queue shapes rather than one queue, which is why the crate is
//! named in the plural. They differ in producer and consumer cardinality, in
//! how they store their items, and in what they do when full. No shape is the
//! canonical one, so there is deliberately no type named `Queue`: a consumer
//! names the shape it wants.
//!
//! Each shape is split into a **producer handle** and a **consumer handle**,
//! and cardinality is carried by whether those handles are [`Clone`]. A
//! single-producer queue hands out a producer that cannot be cloned, so
//! "single producer" is a fact the compiler enforces rather than a sentence in
//! a doc comment.
//!
//! What the shapes have in common is described by the [capability
//! traits](traits) -- [`Producer`], [`Consumer`], [`Bounded`], [`Waitable`],
//! [`Reserving`] -- each naming one thing a queue can do, so a caller can be
//! generic over exactly what it needs and nothing more. [`Claim`] is the one
//! that is not a queue capability: it describes the *reservation* a
//! [`Reserving`] queue hands out, and is what lets a generic caller redeem one
//! rather than only drop it. Bring it into scope to call `send` on a claim
//! whose concrete type you have not named.
//!
//! # Waiting is one-directional: a producer cannot park on a full queue
//!
//! [`Waitable`] is a *consumer's* capability. A consumer can park until there
//! is something to take; there is **no equivalent for a producer waiting until
//! there is somewhere to put**. [`Producer::push`] refuses immediately with
//! [`PushError::Full`], [`Reserving::reserve`] returns `None`, and neither
//! offers a handle to wait on.
//!
//! **Stated here because the obvious comparison misleads.** `crossbeam-channel`
//! blocks in `send` on a full bounded channel, so a reader arriving from it
//! will expect the same and get a refusal. A producer with nowhere to go has to
//! decide for itself -- shed the item, retry on its own schedule, or buffer --
//! rather than being parked by the queue. The refusal *is* the backpressure
//! (D-6), and it is typed so the item comes back rather than being swallowed.
//!
//! The absence is deliberate and not permanent. Whether a producer can wait,
//! and **what it would wait on**, is open: a blocking send that parks on
//! something `WaitForMultipleObjects` cannot see would reintroduce the very
//! composition problem that ruled out the existing channel crates, which is the
//! reason this one exists. Two properties already shape the answer -- a bounded
//! queue can offer such a wait and an unbounded one never can, so it belongs in
//! its own capability trait rather than in [`Waitable`]; and while every shape
//! here has a single consumer, two of them have many *producers*, so a
//! "there is room" signal has N waiters and is not the doorbell mirrored.
//!
//! # A known defect in `reserving_mpsc`, disclosed rather than fixed
//!
//! **[`reserving_mpsc`] can lose an item after 2^32 pushes, on every target --
//! not only 32-bit ones.** Its claim position is a 32-bit half of a packed word
//! by construction, so this reaches x86-64 and ARM64 exactly as it reaches
//! i686. Read that sentence before the paragraph below, because the phrase
//! "32-bit position" invites the opposite reading and this project has already
//! had to correct that misreading once.
//!
//! **What happens.** A producer checks that there is room, is descheduled, and
//! resumes after other producers have driven the position field through a
//! complete wrap. Its claim then succeeds against a value that is numerically
//! identical but a whole generation later, and it writes into a slot whose
//! emptiness was decided long ago. If that slot now holds an item the consumer
//! has not taken, the item is overwritten.
//!
//! **The failure is silent.** No error, no panic, no counter moves. The
//! consumer receives a different item than the one that was sent, and nothing
//! observable says so -- which is why this is documented here rather than left
//! to a caller to discover, and why it cannot be mitigated after the fact.
//!
//! **The exposure, measured rather than estimated.** 2^32 pushes is 37 seconds
//! to roughly four minutes of *sustained* pushing at this crate's own measured
//! rates -- about two minutes at two producers, which is the smallest count
//! that can trigger it at all. That is sustained throughput, not a total
//! accumulated over an uptime. Reaching the wrap is necessary but not
//! sufficient: a producer must also be stalled inside a window a few
//! instructions wide. Rare, but a preemption is enough, and "rare" over
//! billions of pushes is not "never".
//!
//! **What to do about it.** The choice is a real one, which is why the crate
//! states the facts instead of quietly picking:
//!
//! - **[`slotwise_mpsc`] does not have this hazard.** Its positions are 64 bits
//!   on every target, so the equivalent wrap needs 2^64 claims and cannot be
//!   reached. Prefer it unless you need [`Reserving`].
//! - **[`spsc`] never had it**, having no contended claim to race.
//! - **[`reserving_mpsc`] is sound below the wrap.** A queue that will not push
//!   4.3 billion items in one run, or that is not driven at sustained maximum
//!   rate by two or more producers, is not exposed.
//! - If you need reservations *and* those volumes, say so -- the fix is
//!   prototyped and measured, and it is the shipping decision that is open, not
//!   the engineering.
//!
//! This is disclosed on the same principle as the ordering gap below: an
//! adopter gets the information we have rather than an assurance we cannot
//! support. The two are not equally forgiving, though, and the difference is
//! worth stating plainly -- an unverified ordering is a *risk* of a bug, while
//! this is a known one with a computed exposure.
//!
//! # How far the memory orderings are verified, and how far they are not
//!
//! Stated plainly because a lock-free queue that is vague about this is asking
//! to be trusted rather than evaluated.
//!
//! **What is verified.** Every ordering was reasoned about when written and the
//! reasoning is recorded in `DESIGN-NOTES.md` beside the code it justifies. The
//! shapes are covered by an extensive unit suite and by a sabotage suite that
//! injects deliberate defects and requires each to be caught -- which is how the
//! one real ordering bug this crate has had was found: a lost wakeup where the
//! doorbell cleared its mirror flag before resetting the event.
//!
//! **What is not.** Stress testing cannot catch a *weakened memory ordering*
//! here, and that is measured rather than assumed: changing the producer's
//! `Acquire` load of the consumer's position to `Relaxed` left the entire suite
//! green, while every logic defect injected beside it was caught. A test can
//! only observe the interleavings the hardware and scheduler happen to produce,
//! and neither x86-64 nor ARM64 obliged.
//!
//! **So the orderings are not machine-checked.** Verification with a model
//! checker is planned before 1.0. Until then the `0.x` version is meant
//! literally, and an adopter for whom that matters now has the same information
//! we have rather than an assurance we cannot support.
//!
//! One limit worth knowing even after that work lands: a model checker covers
//! the queue shapes' positions and sequence numbers, and **cannot** cover the
//! doorbell, whose correctness is the interleaving of an atomic flag with real
//! `SetEvent` and `ResetEvent` calls. Modelling those would verify a model of
//! them rather than the calls themselves.
//!
//! # Where these algorithms come from
//!
//! **None of the queue algorithms here are novel, and that is deliberate.** A
//! concurrent queue is a bad place to be original: the failure mode is a
//! reordering that appears on one machine, under load, months later. Each shape
//! implements a published design, and the value this crate adds is the waiting,
//! not the queueing.
//!
//! - [`spsc`] is the classic single-producer single-consumer ring buffer, with
//!   the producer's and consumer's positions on separate cache lines so the two
//!   ends stop invalidating each other's line. The structure is old -- Lamport
//!   gave the concurrent-reader/writer treatment in 1983 -- and the padding is
//!   standard modern practice.
//! - [`slotwise_mpsc`] implements Dmitry Vyukov's bounded MPMC array queue,
//!   specialised to one consumer. Each slot carries its own sequence number, so
//!   a producer claims a position and then asks *that slot* whether it is ready,
//!   which keeps producers off any single shared line. It is among the most
//!   widely reimplemented concurrent queues in existence.
//! - [`reserving_mpsc`] uses the other classic approach: a producer counts free
//!   slots against the consumer's position, so space can be *claimed in advance*.
//!   Credit- or ticket-based admission of this kind is long-established in flow
//!   control, and it is the only way to answer "will there be room later?".
//!
//! Where this crate departs from a reference implementation it says so, and why,
//! in `DESIGN-NOTES.md`. The measured behaviour of both MPSC shapes is below,
//! including one case where the published intuition turned out to be wrong on
//! our hardware.
//!
//! # Why not an existing queue crate
//!
//! Rust has excellent channel crates, and for most programs one of them is the
//! right answer. **They are not usable here for one structural reason: on
//! Windows, waiting is a kernel-object operation, and a queue whose readiness is
//! not a `HANDLE` cannot take part in one.**
//!
//! A thread that must wait for "an item arrived **or** an I/O completed **or**
//! this process exited **or** cancellation was requested" waits on all of them
//! at once, in a single `WaitForMultipleObjects`. Every participant in that wait
//! has to be a kernel object. A channel that signals readiness through an
//! internal condition variable, a futex, or a parked-thread list cannot be one
//! of them, however good its own blocking receive is -- and however rich its own
//! select mechanism, because that mechanism can only select over its own
//! channels.
//!
//! The alternatives to a waitable queue are all worse in the same way:
//!
//! - **Poll the queue on a timer.** Trades latency against wakeups, and the
//!   thread is awake to discover nothing happened.
//! - **Dedicate a thread to blocking on the channel, which signals an event.**
//!   Correct, and costs a thread and a hop per item to convert a condition
//!   variable back into the kernel object you needed from the start.
//! - **Move everything to async.** A real answer for a program that is already
//!   async; not one for a thread whose other obligations are `HANDLE`s.
//!
//! So the queue owns a manual-reset event and keeps it consistent with the
//! queue's state -- which is the hard part, and what this crate is actually
//! for. The event is created lazily, so a consumer that only ever polls never
//! allocates a kernel object at all.
//!
//! # Choosing between `slotwise_mpsc` and `reserving_mpsc`
//!
//! They are **two different claim protocols, not one queue with a switch**.
//! [`slotwise_mpsc`] is Vyukov's bounded array queue, where a producer asks a slot's own
//! sequence number whether it is free. [`reserving_mpsc`] counts free slots
//! against the consumer's position, which is the only way a reservation can be
//! answered at all. Both are well-studied designs in production use elsewhere,
//! which is why this crate ships both instead of picking one for you.
//!
//! - **Pushing more than ~4 billion items in one run, from two or more
//!   producers?** Use [`slotwise_mpsc`]. [`reserving_mpsc`] has a known
//!   item-loss defect past that volume, on every target -- see the section
//!   above, which you should read before choosing.
//! - Need [`Reserving`]? Only [`reserving_mpsc`] has it; [`slotwise_mpsc`] structurally
//!   cannot. Weigh that against the defect above rather than treating the
//!   capability as settling the choice.
//! - Otherwise **start with [`reserving_mpsc`]**: it was the faster of the two
//!   at every producer count above one that we measured.
//! - One producer *and* one consumer? Use [`spsc`], which beats both.
//!
//! Measured ns per push, isolated regime, median of three. An AMD EPYC 7763
//! slice (8 cores, 16 threads) and a Snapdragon X2 Elite (12 cores, no SMT):
//!
//! | producers | `slotwise_mpsc` x64 | `reserving` x64 | `slotwise_mpsc` ARM64 | `reserving` ARM64 |
//! |---|---|---|---|---|
//! | 1 | 9.0 | 8.6 | 6.5 | 6.1 |
//! | 2 | 49.0 | 28.0 | 29.8 | 9.4 |
//! | 4 | 84.4 | 33.3 | 60.6 | 12.9 |
//! | 8 | 140.8 | 38.5 | 167.4 | 29.8 |
//! | 16 | 193.5 | 52.2 | 194.9 | 30.6 |
//! | 32 | 239.7 | 56.9 | 195.0 | 30.6 |
//!
//! **Read these as two data points, not as a law**, and measure your own
//! workload before treating them as settled. This comparison has already
//! inverted once: the split was designed on the assumption that `slotwise_mpsc` would be
//! the cheaper shape, and measurement disagreed on both machines. Producer
//! count, how hard the consumer drains, and where the threads are scheduled all
//! move the answer -- placement alone moved an SPSC handoff by 5.6x on one of
//! these hosts.
//!
//! Two things that look like reasons to choose and are not. **Capacity**: on a
//! 64-bit target `slotwise_mpsc` reaches 2^62 slots and `reserving_mpsc` 2^31.
//! On a 32-bit one the crate-wide ceiling is 2^30 and **both** shapes land
//! there -- `reserving_mpsc`'s packed 2^31 is clamped down to it too -- so the
//! difference disappears entirely and the comparison means nothing at all.
//! Either way it counts slots allocated up front rather than items ever pushed,
//! and 2^31 slots is tens of gigabytes before the ring holds anything useful.
//! **`slotwise_mpsc` winning at one producer**: true in one regime, and at one
//! producer you want [`spsc`].
//!
//! # Shutting down
//!
//! A consumer learns that every producer is gone from
//! `is_disconnected`, and a producer learns the consumer is gone from a typed
//! [`PushError::Disconnected`] that hands the item back. The orderly shutdown
//! is therefore: drain to empty, then check.
//!
//! For everything that does not go to plan there is [`disposal`]. A queue torn
//! down with items still in it must do *something* with them, and by default it
//! destroys them inside the last handle's drop -- on whichever thread happened
//! to release it. When an item owns a handle that is a hazard rather than a
//! detail, because closing a handle can block and the dropping thread may be a
//! pool callback that must not. Building the queue with a [`Disposal`] sink
//! hands those items back instead.
//!
//! # Status
//!
//! [`spsc`], [`slotwise_mpsc`] and [`reserving_mpsc`] are implemented, each with its
//! doorbell: any of them can
//! be polled with no kernel object at all, blocked on directly, or waited on
//! alongside other handles. The remaining shapes land in the milestones tracked
//! by `CHECKLIST-io-domains.md` at the workspace root; the decisions they are
//! built against are recorded in `DESIGN-NOTES.md` beside this file.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

// Every item is gated, so the crate builds to an empty shell off Windows rather
// than failing: the implementation rests on `std::os::windows::io` and
// `windows-sys` throughout, and the whole premise -- readiness that *is* a
// waitable `HANDLE` -- has no meaning on another platform. This mirrors the
// sibling Windows-only crates here, and the crate documentation above states
// the same contract, so the two cannot drift apart.

#[cfg(windows)]
mod blocking;
#[cfg(windows)]
mod capacity;
#[cfg(windows)]
pub mod disposal;
#[cfg(windows)]
mod doorbell;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod metrics;
#[cfg(windows)]
mod options;
/// **Experimental, and not covered by this crate's semver promise.**
///
/// A duplicate of [`reserving_mpsc`] differing only in its claim protocol,
/// built to be measured against it so that the ABA hole recorded as `SH-14.1`
/// can be closed on evidence rather than on judgement. It will either be merged
/// into `reserving_mpsc` or deleted.
#[cfg(all(windows, feature = "experimental-permit-claim"))]
pub mod permit_mpsc;
#[cfg(all(windows, test))]
mod race_hooks;
#[cfg(windows)]
pub mod reserving_mpsc;
#[cfg(windows)]
pub mod slotwise_mpsc;
#[cfg(windows)]
pub mod spsc;
#[cfg(windows)]
pub mod traits;

#[cfg(windows)]
pub use disposal::Disposal;
#[cfg(windows)]
pub use error::{CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError};
#[cfg(windows)]
pub use options::Options;
#[cfg(windows)]
pub use traits::{Bounded, Claim, Consumer, Drain, Observable, Producer, Reserving, Waitable};

/// Pads and aligns a value onto its own cache line.
///
/// The producer's position and the consumer's position are written by different
/// threads on every operation. Left adjacent they would share a cache line, and
/// each write would invalidate the other thread's copy of a value it only ever
/// reads -- false sharing, which converts an uncontended queue into a
/// contended one while every load and store remains individually correct.
///
/// 128 rather than 64: that is the cache line on aarch64, and on x86-64 the
/// adjacent-line prefetcher pulls pairs of 64-byte lines, so 64 does not
/// reliably separate them.
#[cfg(windows)]
#[repr(align(128))]
struct CacheAligned<T>(T);

// The README states this crate's wait protocol, and a review round found that
// statement had drifted from what `blocking::recv` actually does -- it named
// three steps where the code has four, and a caller following it would have
// waited forever at the end of the stream. That particular drift is fixed and
// pinned by a test, but the general risk is not: prose nothing executes can
// only rot.
//
// The README carries no code today, so this compiles nothing. It is here so
// that the first example somebody adds is compiled rather than trusted, which
// is the cheapest moment to close the gap. `cfg(doctest)` means the item exists
// only while rustdoc collects tests, so an ordinary build pays nothing.
#[cfg(all(doctest, windows))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
