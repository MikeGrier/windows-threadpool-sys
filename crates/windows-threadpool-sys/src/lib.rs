// Copyright (c) 2026 Mike Grier
//! Memory-safe access to the Windows thread pool APIs.
//!
//! The Windows thread pool integrates work, timers, waits, and asynchronous I/O
//! with the operating system's own scheduling facilities. This crate will wrap
//! those facilities while making callback and resource lifetimes explicit in
//! Rust.
//!
//! The crate is currently in its initial development stage. It provides
//! [`callback_env`] (SDK-equivalent `TP_CALLBACK_ENVIRON_V3` helpers),
//! [`work`] (owned `TP_WORK` objects), and [`io`] (the `TP_IO` completion
//! backend built on the overlapped submission seam owned by
//! `windows-overlapped-io-sys`).

#![warn(missing_docs)]

pub mod callback_env;
pub mod io;
pub mod pool;
pub mod work;
