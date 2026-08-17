// Copyright (c) 2026 Mike Grier
//! Memory-safe access to the Windows thread pool APIs.
//!
//! The Windows thread pool integrates work, timers, waits, and asynchronous I/O
//! with the operating system's own scheduling facilities. This crate will wrap
//! those facilities while making callback and resource lifetimes explicit in
//! Rust.
//!
//! The crate is currently in its initial development stage.

#![warn(missing_docs)]
