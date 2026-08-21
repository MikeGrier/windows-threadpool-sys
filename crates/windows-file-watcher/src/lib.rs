// Copyright (c) 2026 Mike Grier
//! Memory-safe Windows path-change watching with full `ReadDirectoryChangesW`
//! fidelity and a `FindFirstChangeNotification` coarse fallback.
//!
//! This crate is Windows-only: every item is gated behind `cfg(windows)`, so it
//! resolves to an empty crate on other targets. Platform-independent watching is
//! meant to be built at a higher layer -- this crate is about excellent Windows
//! behaviour (path-name and notification-limitation fidelity) with memory safety.
//!
//! # The model
//!
//! A [`Monitor`] owns the watching. It hands out [`Session`]s, each of which
//! bundles a way to make requests with the destination every subscription made
//! through it delivers to; [`Monitor::session`] returns one together with the
//! [`Receiver`] its notifications arrive on. [`Session::subscribe`] registers a
//! path and returns an affine [`Watch`] that cancels when dropped.
//!
//! ```no_run
//! use windows_file_watcher::{Monitor, Notification, WatchOptions};
//!
//! let monitor = Monitor::new()?;
//! let (session, receiver) = monitor.session();
//! let watch = session.subscribe(r"C:\some\directory", WatchOptions::new())?;
//!
//! while let Some(notification) = receiver.recv() {
//!     match notification {
//!         Notification::Batch { changes, .. } => println!("{} change(s)", changes.len()),
//!         Notification::Desync { cause, .. } => println!("re-scan: {cause:?}"),
//!         Notification::Completion { outcome, .. } => println!("request: {outcome:?}"),
//!     }
//! }
//! # drop(watch);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Everything is queued, in both directions
//!
//! The crate never calls into client code. A request is something the client
//! enqueues; a notification is something the crate enqueues and the client
//! collects. So nothing a client does -- blocking, panicking, being slow -- can
//! stall or unwind the crate's own cadence, and that holds by construction rather
//! than by asking a callback to behave.
//!
//! Which thread a client drains on is entirely its own business. A client that
//! does not want to dedicate one to [`Receiver::recv`] can take
//! [`Receiver::doorbell`] and wait on it from its own thread pool.
//!
//! # Losses are reported, never silent
//!
//! `ReadDirectoryChangesW` can lose changes -- its buffer overflows under a burst
//! -- and so can a client that stops draining. Both, and every other hole, are
//! reported as one cause-tagged [`Notification::Desync`] meaning *re-scan*.
//! Honest reporting of that limitation is a core requirement of this crate rather
//! than an afterthought.

#![warn(missing_docs)]

#[cfg(windows)]
mod directory;

#[cfg(windows)]
mod monitor;

#[cfg(windows)]
mod notify;

#[cfg(windows)]
mod queue;

#[cfg(windows)]
mod servicing;

#[cfg(windows)]
mod session;

#[cfg(all(windows, test))]
mod testing;

#[cfg(windows)]
mod watch;

#[cfg(windows)]
mod watcher;

#[cfg(windows)]
pub use directory::OpenFailure;
#[cfg(windows)]
pub use monitor::Monitor;
#[cfg(windows)]
pub use notify::{Change, ChangeKind, DecodedBatch, DesyncCause, RelativeName, decode_batch};
#[cfg(windows)]
pub use queue::{DEFAULT_BOUND, Notification, Outcome, Receiver, WatchId};
#[cfg(windows)]
pub use session::Session;
#[cfg(windows)]
pub use watch::{RetryMode, Watch, WatchOptions};
