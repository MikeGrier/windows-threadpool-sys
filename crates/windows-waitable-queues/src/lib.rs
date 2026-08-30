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
//! # Status
//!
//! Skeleton. The shapes land in the milestones tracked by
//! `CHECKLIST-io-domains.md` at the workspace root; the decisions they will be
//! built against are recorded in `DESIGN-NOTES.md` beside this file.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
