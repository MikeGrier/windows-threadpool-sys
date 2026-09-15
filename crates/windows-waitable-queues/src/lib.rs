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
//! # How long `reserving_mpsc` runs before its claim position recurs
//!
//! **[`reserving_mpsc`] can lose an item after 2^32 pushes under its default
//! layout, on every target -- not only 32-bit ones.** That layout gives the
//! claim position a 32-bit half of a packed word, so this reaches x86-64 and
//! ARM64 exactly as it reaches i686. Read that sentence before the paragraph
//! below, because the phrase "32-bit position" invites the opposite reading and
//! this project has already had to correct that misreading once.
//!
//! **This is a property of the default layout, not of the shape**, and that is
//! a change: it was previously a defect a caller had to live with. The claim
//! word packs an outstanding-reservation count beside the position, and how its
//! bits are divided is now a caller's choice. Reservations are bounded by how
//! many producers are mid-send -- hundreds at most -- so giving up a ceiling
//! nobody reaches buys positions:
//!
//! | Layout | Outstanding reservations | Pushes to recurrence | At sustained maximum rate |
//! |---|---|---|---|
//! | `Balanced` (default) | 4,294,967,295 | 2^32 | about 37 seconds |
//! | `Enduring` | 65,535 | 2^48 | about 28 days |
//! | `Perpetual` | 255 | 2^56 | about 20 years |
//! | `Wide` (needs `dwcas`) | 4,294,967,295 | 2^64 | unreachable |
//!
//! ```
//! use windows_waitable_queues::reserving_mpsc::{self, Perpetual};
//!
//! // The same queue, with a claim position that outlives the process.
//! let (tx, rx) = reserving_mpsc::bounded_as::<u32, Perpetual>(64)?;
//! # let _ = (tx, rx);
//! # Ok::<(), windows_waitable_queues::CapacityError>(())
//! ```
//!
//! **A deeper position is the same exchange on the same word.** `Balanced`,
//! `Enduring`, and `Perpetual` all issue the same exchange on the same 64-bit
//! word and differ only in shift and mask constants, so there is no structural
//! reason for one to be slower -- but **what that costs in throughput is not
//! established**: a probe comparing them found them indistinguishable at low
//! producer counts, and at high counts a difference that did not clearly exceed
//! the run-to-run variation of the same code measured twice. `Wide` is a separate
//! matter: it needs a 128-bit exchange, and the whole push path was measured as
//! slower under it as producer count rises -- near parity at one or two,
//! several times by thirty-two, in the isolated regime -- and it is the only
//! thing in
//! this crate
//! that costs a third-party dependency. What it provides that `Perpetual` does
//! not is a 64-bit position: the recurrence moves to 2^64 pushes, which no
//! deployment reaches, rather than to a horizon measured in years.
//!
//! The default remains `Balanced` so that no existing caller's behaviour
//! changed when the choice was introduced. Under it, a queue driven past 2^32
//! pushes by two or more producers can **silently lose an item** -- the defect
//! described above. `Enduring` and `Perpetual` move that point out, and `Wide`
//! removes it.
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
//! **The exposure, measured rather than estimated.** Under `Balanced`, 2^32
//! pushes is 37 seconds to roughly four minutes of *sustained* pushing at this
//! crate's own measured rates -- about two minutes at two producers, which is
//! the smallest count that can trigger it at all. That is sustained throughput,
//! not a total accumulated over an uptime. Reaching the wrap is necessary but
//! not sufficient: a producer must also be stalled inside a window a few
//! instructions wide. Rare, but a preemption is enough, and "rare" over
//! billions of pushes is not "never".
//!
//! The figures in the table above scale that same measurement by the position
//! width, so they are a floor on time rather than a forecast: a queue that must
//! drain cannot sustain the fastest rate measured, and a slower producer takes
//! proportionally longer to reach its wrap.
//!
//! **What bears on it.**
//!
//! - **Naming a layout moves it.** `Perpetual` puts the recurrence about twenty
//!   years out. **What it costs in throughput is not established** -- it issues
//!   the same atomic compare-exchange on the same `u64` as the default, and was
//!   measured as indistinguishable from it at low producer counts; at high
//!   counts the difference did not clearly exceed the run-to-run variation of
//!   the same code measured twice.
//! - **[`slotwise_mpsc`] does not have this hazard** under any layout. Its
//!   positions are 64 bits on every target, so the equivalent wrap needs 2^64
//!   claims. It does not offer [`Reserving`].
//! - **[`spsc`] never had it**, having no contended claim to race.
//! - **The default layout is sound below its wrap.** A queue that will not push
//!   4.3 billion items in one run, or that is not driven at sustained maximum
//!   rate by two or more producers, is not exposed even on `Balanced`.
//!
//! This is disclosed on the same principle as the ordering gap below: an
//! adopter gets the information we have rather than an assurance we cannot
//! support. The difference between the two is worth stating plainly -- an
//! unverified ordering is a *risk* of a bug, while this is a known one with a
//! computed exposure. What has changed is that the exposure is now a number the
//! caller sets rather than one the crate imposes.
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
//!   producers?** [`reserving_mpsc`] under its default layout has a known
//!   item-loss defect past that volume, on every target; [`slotwise_mpsc`]'s
//!   positions are 64 bits under every configuration, and naming a deeper layout
//!   on [`reserving_mpsc`] moves the recurrence out. The mechanism is in the
//!   section above.
//! - **Of the two MPSC shapes, only [`reserving_mpsc`] implements
//!   [`Reserving`]**; [`slotwise_mpsc`] structurally cannot. ([`spsc`]
//!   implements it too, and the experimental `permit_mpsc` exposes its own
//!   `reserve`.) Wanting it no longer means accepting the default layout's
//!   recurrence, but the trade is not gone -- it changes axis: a deeper position
//!   is paid for with a lower ceiling on outstanding reservations, 65,535 under
//!   `Enduring` and 255 under `Perpetual` against `u32::MAX` under the default.
//! - **[`spsc`] requires exactly one producer and one consumer**, and does less
//!   work than either MPSC shape because of it.
//!
//! The measurements below are one host's observation, recorded with the
//! parameters that produced them. They are not a ranking.
//!
//! Isolated regime (producers only, capacity large enough that nothing is
//! refused), ns per push, median of three runs:
//!
//! | producers | `slotwise_mpsc` | `reserving_mpsc` | `permit_mpsc` | `baseline_fetch_add` |
//! |---|---|---|---|---|
//! | 1 | 6.3 | 5.4 | 8.0 | 2.3 |
//! | 2 | 54.0 | 34.9 | 41.5 | 11.7 |
//! | 4 | 89.3 | 37.1 | 32.1 | 15.1 |
//! | 8 | 143.8 | 38.1 | 26.4 | 15.2 |
//! | 16 | 246.9 | 51.1 | 21.4 | 15.3 |
//! | 32 | 235.7 | 53.0 | 21.2 | 15.1 |
//!
//! Attribution, because a figure without it is not reusable data:
//!
//! | | |
//! |---|---|
//! | Host | `x86_64 16p/8c smt+ L2[2,2,2,2,2,2,2,2] ec[0:16] numa[16]` |
//! | Profile | release |
//! | Sampling | 50,000 pushes per producer, median of 5 repetitions, one untimed warmup pass |
//! | Runs | 3 whole-probe invocations, median of the three |
//! | Instrument | `probe-queue-contention`, at commit `a99108f` |
//! | Taken | 2026-09-15 |
//!
//! The banner's `numa[16]` is a single NUMA node holding all sixteen processors,
//! so nothing here says anything about cross-domain behaviour. `permit_mpsc` is
//! behind `experimental-permit-claim` and is not covered by the semver promise.
//! `baseline_fetch_add` is N threads incrementing one `AtomicU64`, included so
//! the queue figures can be read against what this processor does to a contended
//! line at all.
//!
//! **Read these as one machine's numbers.** Producer counts above 8 oversubscribe
//! this host's 8 physical cores, and the spread across the three runs is not
//! small: `slotwise_mpsc` at sixteen producers gave 257.3, 215.1 and 246.9.
//!
//! A previous version of this table compared an AMD EPYC 7763 slice against a
//! Snapdragon X2 Elite. It was removed rather than carried forward: its figures
//! predate a correction to the probe's timing window, and neither machine is
//! available here to retake them. One finding from it was structural rather than
//! numeric and is worth keeping -- the split was designed on the assumption that
//! `slotwise_mpsc` would be the cheaper shape, and measurement disagreed on both
//! machines.
//!
//! **What moves these numbers.** Producer count, how hard the consumer drains,
//! and where the threads are scheduled -- placement alone moved an SPSC handoff
//! by 5.6x on an earlier host this workspace measured.
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
//! alongside other handles. Shapes with many consumers, and shapes that signal
//! when space becomes available so a producer can wait for room, are under
//! consideration for a future revision. The decisions this crate is built
//! against are recorded in `DESIGN-NOTES.md` beside this file.

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
pub use error::{
    CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError, TryRecvError,
};
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
