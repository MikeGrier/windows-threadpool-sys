// Copyright (c) 2026 Mike Grier
//! Memory-safe access to the Windows thread pool APIs.
//!
//! The Windows thread pool integrates work, timers, waits, and asynchronous I/O
//! with the operating system's own scheduling facilities. Its distinguishing
//! property is that an idle workload costs no threads at all: the pool and the
//! kernel cooperate so a process waiting on timers, events, or I/O holds no
//! dedicated thread stacks. This crate wraps those facilities while making
//! callback and resource lifetimes explicit in Rust.
//!
//! # The object types
//!
//! Each thread-pool object is an owned Rust type whose `Drop` performs the
//! documented teardown for that object, so callbacks can never outlive the state
//! they capture:
//!
//! | Type | Wraps | Runs the callback when |
//! |---|---|---|
//! | [`work::ThreadpoolWork`] | `TP_WORK` | you submit it |
//! | [`timer::ThreadpoolTimer`] | `TP_TIMER` | a due time arrives, once per arming |
//! | [`timer::ThreadpoolPeriodicTimer`] | `TP_TIMER` | every period, until stopped |
//! | [`wait::ThreadpoolWait`] | `TP_WAIT` | a handle signals or a wait times out |
//! | [`io::ThreadpoolIo`] | `TP_IO` | an overlapped operation completes |
//!
//! One-shot and periodic timers are separate types on purpose. The platform
//! models both with one object and a `period` argument, which hides the property
//! that matters most when writing the callback: a [`timer::ThreadpoolPeriodicTimer`] may
//! queue its next tick while the previous one is still running, so its callback
//! must tolerate overlapping with itself, whereas a [`timer::ThreadpoolTimer`]
//! re-armed from *inside* its callback never does -- that request is applied
//! only once the callback returns. Arming a one-shot from outside while its
//! callback runs can still overlap it; see the [`timer`] module for both the
//! choice and that distinction.
//!
//! Three supporting types shape where those callbacks run and how they are torn
//! down: [`pool::ThreadpoolPool`] is an owned private pool,
//! [`callback_env::CallbackEnviron`] is the environment that selects a pool and
//! a callback priority when an object is created, and
//! [`cleanup_group::CleanupGroup`] releases many objects in one step instead of
//! dropping each individually.
//!
//! # Submitting work
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use windows_threadpool_sys::work::ThreadpoolWork;
//!
//! let count = Arc::new(AtomicUsize::new(0));
//! let counter = Arc::clone(&count);
//!
//! let work = ThreadpoolWork::new(move || {
//!     counter.fetch_add(1, Ordering::SeqCst);
//! }, None)?;
//!
//! for _ in 0..4 {
//!     work.submit();
//! }
//! work.wait();
//!
//! assert_eq!(count.load(Ordering::SeqCst), 4);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Running callbacks on a private pool
//!
//! A [`pool::ThreadpoolPool`] bounds the threads a subsystem may consume.
//! Declare the pool before the objects that use it, so it is dropped last.
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use windows_threadpool_sys::callback_env::CallbackEnviron;
//! use windows_threadpool_sys::pool::ThreadpoolPool;
//! use windows_threadpool_sys::work::ThreadpoolWork;
//!
//! let pool = ThreadpoolPool::new()?;
//! pool.set_max_threads(2)?;
//!
//! let mut env = CallbackEnviron::new();
//! env.set_pool(&pool);
//!
//! let count = Arc::new(AtomicUsize::new(0));
//! let counter = Arc::clone(&count);
//! let work = ThreadpoolWork::new(move || {
//!     counter.fetch_add(1, Ordering::SeqCst);
//! }, Some(&mut env))?;
//!
//! work.submit();
//! work.wait();
//! assert_eq!(count.load(Ordering::SeqCst), 1);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Callback rules
//!
//! Callbacks run on shared, process-managed threads, so every object type here
//! holds its callback to the same contract:
//!
//! - It must restore any thread-local or thread state it changes before
//!   returning, and must not terminate its thread.
//! - It must not block waiting on its own object's rundown, which would wait on
//!   itself.
//! - It must not panic. A panic unwinds to the `extern "system"` trampoline,
//!   where an escaping unwind aborts the process; nothing contains it. The panic
//!   hook still runs first, so the message and location reach stderr by default
//!   -- what is given up is the process, not the diagnostic. A callback that can
//!   fail must handle its own errors rather than panicking.
//!
//! # Relationship to `windows-overlapped-io-sys`
//!
//! Thread-pool I/O is one of three completion backends for the overlapped model
//! defined by [`windows-overlapped-io-sys`]. This crate implements the `TP_IO`
//! backend over that crate's endpoint ownership and pinned operation storage,
//! adding the balanced `StartThreadpoolIo` accounting that only the thread pool
//! requires. The pool's internal completion port is never exposed.
//!
//! [`windows-overlapped-io-sys`]: https://docs.rs/windows-overlapped-io-sys
//!
//! # Status
//!
//! The crate is in active development. Work, timers, waits, private pools,
//! cleanup groups, and thread-pool I/O are implemented and tested.
//!
//! Thread-pool I/O is deliberately not a cleanup-group member: a `TP_IO` object
//! must not be closed while an overlapped operation is outstanding, and a bulk
//! release cannot satisfy that. See [`cleanup_group`] for the reasoning.

#![warn(missing_docs)]

// Every module wraps a Win32 thread-pool object, so the whole public surface is
// gated on Windows and the crate resolves to an empty one elsewhere. This
// matches the sibling `windows-overlapped-io-sys`, and it is what lets a
// cross-platform dependency tree name this crate unconditionally instead of
// failing to compile on other targets.
#[cfg(windows)]
pub mod callback_env;
#[cfg(windows)]
pub mod cleanup_group;
#[cfg(windows)]
pub mod io;
#[cfg(windows)]
pub mod pool;
#[cfg(windows)]
pub mod timer;
#[cfg(windows)]
pub mod wait;
#[cfg(windows)]
pub mod work;

// The crate's markdown documentation is compiled as doctests, so an example that
// a contract change invalidates breaks the build instead of quietly teaching the
// old answer. `cfg(doctest)` means these items exist only while rustdoc collects
// tests, so they cost an ordinary build nothing.
#[cfg(all(doctest, windows))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
