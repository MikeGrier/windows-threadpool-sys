// Copyright (c) 2026 Mike Grier
//! Memory-safe Rust over the Windows `IoRing` submission/completion ring.
//!
//! Windows 11 and Server 2022 added `IoRing`, a submission/completion ring for
//! file I/O closer in shape to `io_uring` than to anything else Windows offers.
//! This crate raises those primitives into safe Rust with the minimum additional
//! CPU and memory cost: a completion hands the caller's buffer back without the
//! crate having allocated anything to track it.
//!
//! # Scope: a file data plane, not a general completion backend
//!
//! The kernel's operation table is fixed at seven entries -- no-op, read, write,
//! flush, register-files, register-buffers, and cancel. There is no ioctl
//! operation, no socket operation, and no directory-change operation, and unlike
//! Linux's `io_uring` -- which grew to roughly fifty opcodes including full
//! socket support -- Windows `IoRing` has not grown beyond file I/O.
//!
//! So this crate is not a replacement for
//! [`windows-overlapped-io-sys`](https://docs.rs/windows-overlapped-io-sys),
//! which remains the crate for arbitrary I/O on arbitrary handles. Use this one
//! for high-volume file reads and writes; use that one for everything else.
//! Neither can subsume the other.
//!
//! # Availability is a runtime question
//!
//! `IoRing` is present from Windows 11 and Server 2022, but three separate facts
//! about it are decided at runtime rather than at compile time: which ring
//! version the system supports, whether the ring is a real kernel ring or a
//! user-mode emulation with no kernel benefit, and whether the completion-event
//! feature the thread-pool delivery path depends on is available at all. All
//! three are answerable without creating a ring.
//!
//! # Status
//!
//! Under construction. The design, including the delivery-architecture guidance
//! this crate exists to make usable, is recorded in `DESIGN-NOTES.md` beside the
//! source; the build-out is tracked in `CHECKLIST.md`.

#![warn(missing_docs)]

#[cfg(windows)]
mod capability;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod ring;

#[cfg(windows)]
pub use capability::{Capabilities, RingVersion, capabilities};
#[cfg(windows)]
pub use error::IoRingError;
#[cfg(windows)]
pub use ring::{IoRing, Op, RingInfo};
