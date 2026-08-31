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
//! generic over exactly what it needs and nothing more.
//!
//! # Choosing between `mpsc` and `reserving_mpsc`
//!
//! They are **two different claim protocols, not one queue with a switch**.
//! [`mpsc`] is Vyukov's bounded array queue, where a producer asks a slot's own
//! sequence number whether it is free. [`reserving_mpsc`] counts free slots
//! against the consumer's position, which is the only way a reservation can be
//! answered at all. Both are well-studied designs in production use elsewhere,
//! which is why this crate ships both instead of picking one for you.
//!
//! - Need [`Reserving`]? Only [`reserving_mpsc`] has it; [`mpsc`] structurally
//!   cannot.
//! - Otherwise **start with [`reserving_mpsc`]**: it was the faster of the two
//!   at every producer count above one that we measured.
//! - One producer *and* one consumer? Use [`spsc`], which beats both.
//!
//! Measured ns per push, isolated regime, median of three. An AMD EPYC 7763
//! slice (8 cores, 16 threads) and a Snapdragon X2 Elite (12 cores, no SMT):
//!
//! | producers | `mpsc` x64 | `reserving` x64 | `mpsc` ARM64 | `reserving` ARM64 |
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
//! inverted once: the split was designed on the assumption that `mpsc` would be
//! the cheaper shape, and measurement disagreed on both machines. Producer
//! count, how hard the consumer drains, and where the threads are scheduled all
//! move the answer -- placement alone moved an SPSC handoff by 5.6x on one of
//! these hosts.
//!
//! Two things that look like reasons to choose and are not. **Capacity**:
//! `mpsc` reaches 2^63 slots and `reserving_mpsc` 2^31, but that counts slots
//! allocated up front rather than items ever pushed, and 2^31 slots is tens of
//! gigabytes before the ring holds anything useful. **`mpsc` winning at one
//! producer**: true in one regime, and at one producer you want [`spsc`].
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
//! [`spsc`] and [`mpsc`] are implemented, both with their doorbell: either can
//! be polled with no kernel object at all, blocked on directly, or waited on
//! alongside other handles. The remaining shapes land in the milestones tracked
//! by `CHECKLIST-io-domains.md` at the workspace root; the decisions they are
//! built against are recorded in `DESIGN-NOTES.md` beside this file.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

mod blocking;
mod capacity;
pub mod disposal;
mod doorbell;
mod error;
mod metrics;
pub mod mpsc;
mod options;
#[cfg(test)]
mod race_hooks;
pub mod reserving_mpsc;
pub mod spsc;
pub mod traits;

pub use disposal::Disposal;
pub use error::{CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError};
pub use options::Options;
pub use traits::{Bounded, Consumer, Drain, Observable, Producer, Reserving, Waitable};

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
#[repr(align(128))]
struct CacheAligned<T>(T);
