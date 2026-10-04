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
//! # Teardown drains, and why that is not a style preference
//!
//! Every teardown in this crate **drains**: it lets a callback that is already
//! queued run, rather than asking the kernel to discard it. `Drop` blocks until
//! that is finished, and [`wait::ThreadpoolWait::stop_and_drain`] is the same
//! work at a point the caller chooses.
//!
//! That is partly a correctness argument -- a queued callback is work the caller
//! asked for, and discarding it is not "finalised". It is also the only
//! available defence against a measured fault in the Windows thread pool, which
//! is worth stating plainly because the fault is silent, process-wide, and easy
//! to attribute to anything else.
//!
//! ## The fault
//!
//! Tearing down a wait asks the kernel to cancel its completion packet
//! (`IopCancelWaitCompletionPacket`). When the packet has **already been
//! delivered** to the pool's completion port, that cancel removes it. If the
//! removal lands a few microseconds after the packet was queued, on a pool that
//! has **no threads yet**, the port's notification to its worker factory can be
//! lost.
//!
//! Afterwards that pool dispatches **nothing**: not the waits already armed, not
//! a freshly armed wait, not a timer, not a completed overlapped read. The
//! factory meanwhile reads as perfectly healthy -- not paused, not shut down,
//! permitted to create a worker, no failed creation, zero workers. Work queues
//! up on the port and is never looked at.
//!
//! ## What it costs, and what recovers it
//!
//! The blast radius is one pool, and all of it. A second pool in the same
//! process is unaffected. But the pool normally affected is the **default** one,
//! so in practice that is every component in the process that did not create its
//! own -- including code with no connection to whoever tore the wait down.
//!
//! Submitting a work item recovers it immediately and completely, because that
//! reaches the factory by a route the lost notification is not on. Nothing else
//! observed does. So a program that submits work items near its waits sees a
//! **latency spike** easy to mistake for scheduler jitter; a program using only
//! waits, timers and I/O has no stimulus that will ever help it and **hangs**.
//!
//! ## Why draining is the fix rather than a delay
//!
//! A gap between the disarm and the close also prevents it, and is **not** what
//! this crate does. A gap makes the race improbable; draining makes it
//! impossible, because `WaitForThreadpoolWaitCallbacks` with
//! `fCancelPendingCallbacks` false cannot reach the removal primitive on any
//! path. The queued callback runs, dispatch clears the association, and the
//! close that follows finds nothing to take.
//!
//! Note the shape of that: **the close performs the same removal**, so no choice
//! of entry point avoids the dangerous call. Draining does not dodge it -- it
//! empties it. Avoiding
//! [`wait::ThreadpoolWait::try_cancel_pending_no_heal_tracking`] buys nothing on
//! its own, which is why the default paths drain rather than the hazardous call
//! being hidden.
//!
//! ## If you are worried about code you do not control
//!
//! The fault needs a pool with no threads, which bounds the exposure to process
//! start and to the moments after a pool's last worker retires.
//! [`pool::prewarm_default_pool`] removes that precondition for as long as the
//! pool stays warm. It is not a fix, and this crate's own teardowns do not need
//! it.
//!
//! The worker-factory counters the diagnosis turned on are **not** readable from
//! this crate: reading them needs an undocumented entry point and an unpublished
//! structure layout, which this crate does not ship. The captures that were taken
//! with them are under `measurements/`.
//!
//! Holding the pool warm permanently is not available: `SetThreadpoolThreadMinimum`
//! does not accept the default pool, and calling it that way terminates the
//! process rather than failing.
//!
//! ## Being told when a teardown had to drain: the `fail-fast` feature
//!
//! By default a teardown that finds a drain still owed records it and carries
//! on. The `fail-fast` feature, off by default, turns that into a panic, so a
//! caller who left the blocking drain to `Drop` finds out at the point it
//! happened rather than from a trace afterwards.
//!
//! **Cargo unifies features across a build, so any crate that enables this
//! turns it on for every crate in that build.** A library cannot decline
//! another dependency's choice, and `default-features = false` does not help:
//! the feature is off by default, so declining the defaults declines nothing.
//! What changes is teardown behaviour process-wide, which is a reasonable thing
//! for an application to ask about its own code and an unreasonable one to
//! impose on an unrelated component sharing the build. Enable it from a binary,
//! a test, or a development profile rather than from a published library's
//! default feature set.
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
pub(crate) mod heal;
#[cfg(windows)]
pub mod io;
#[cfg(windows)]
pub(crate) mod obligation;
#[cfg(windows)]
pub mod pool;
#[cfg(windows)]
pub(crate) mod rearm;
#[cfg(windows)]
pub mod timer;

/// A trace for defects that only appear under concurrency, compiled out
/// unless the `trace` feature is on and narrowed by environment variable when
/// it is. See the module documentation for why an `eprintln!` is the wrong
/// instrument for that class of problem.
///
/// Gated on Windows like every other Win32-backed module here: the traced
/// build calls `GetCurrentThreadId`, so leaving it ungated would let
/// `--features trace` break this crate's empty-on-other-targets behaviour.
#[cfg(windows)]
pub mod trace;
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
