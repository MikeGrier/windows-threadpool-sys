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
//!         // Suspended/Resumed/Established/RetryQuestion are opt-in (D-13/D-27)
//!         // and never arrive unless requested through `WatchOptions`.
//!         _ => {}
//!     }
//! }
//! # drop(watch);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Everything is queued, in both directions
//!
//! The crate never calls into client code. A request is something the client
//! enqueues (the request queue); a notification is something the crate enqueues
//! and the client collects (the notification queue). So nothing a client does --
//! blocking, panicking, being slow -- can stall or unwind the crate's own
//! cadence, and that holds by construction rather than by asking a callback to
//! behave.
//!
//! Which thread a client drains the notification queue on is entirely its own
//! business. A client that does not want to dedicate one to [`Receiver::recv`]
//! can take [`Receiver::doorbell`] -- a manual-reset event, created lazily -- and
//! wait on it from its own thread pool, including from a `ThreadpoolWait`
//! callback of its own: ringing a doorbell is crate-owned queue signaling, not a
//! callback carrying client data, so this is not an exception to "the crate
//! never calls into client code" -- there is no exception.
//!
//! # Losses are reported, never silent
//!
//! `ReadDirectoryChangesW` can lose changes -- its buffer overflows under a burst
//! -- and so can a client that stops draining. Both, and every other hole, are
//! reported as one cause-tagged [`Notification::Desync`] meaning *re-scan*.
//! Honest reporting of that limitation is a core requirement of this crate rather
//! than an afterthought.
//!
//! # Faults recover on their own, or on your terms
//!
//! An open failure or a live-watch fault is never terminal (D-14): the monitor
//! retries indefinitely, and only a target that can genuinely never become
//! watchable is reported permanently rather than retried forever. Recovery
//! timing is chosen per subscription, through [`WatchOptions::retry`]:
//!
//! - [`RetryMode::Defaults`] (the default) retries autonomously at a fixed
//!   500ms delay.
//! - [`RetryMode::Interactive`] asks: a [`Notification::RetryQuestion`] names
//!   the failing operation, and [`Session::answer`] supplies the next delay
//!   (clamped to a 50ms floor); an explicit `answer(watch, None)` declines,
//!   which is what counts at the default -- never answering at all leaves the
//!   question outstanding indefinitely. Several subscriptions sharing a
//!   coalesced directory watcher take the earliest answer.
//!
//! Opting a subscription into [`WatchOptions::report_liveness`] additionally
//! delivers `Suspended`/`Resumed` brackets around an outage and an
//! `Established { mode }` report naming which tier -- detailed
//! (`ReadDirectoryChangesW`) or the coarse `FindFirstChangeNotification`
//! fallback for volumes that do not support the detailed API -- is actually
//! watching.

#![warn(missing_docs)]

#[cfg(windows)]
mod directory;

#[cfg(windows)]
mod coarse;

#[cfg(windows)]
mod monitor;

#[cfg(windows)]
mod notify;

#[cfg(windows)]
mod queue;

#[cfg(windows)]
mod route;

#[cfg(windows)]
mod retry;

#[cfg(windows)]
mod servicing;

#[cfg(windows)]
mod session;

// M9.5: the data-driven scenario stress model/harness, shared by the
// `run-scenario` binary and the `scenario_stress` integration test. `pub`
// because a `[[bin]]` target can only reach it through the library's public
// surface (D-72) -- see the module's own docs for why its JSON schema is
// nonetheless not part of this crate's semver contract.
#[cfg(all(windows, feature = "scenario-tool"))]
pub mod scenario;

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
pub use retry::{FaultOperation, WatchMode};
#[cfg(windows)]
pub use session::Session;
#[cfg(windows)]
pub use watch::{RetryMode, Watch, WatchOptions};
