// Copyright (c) 2026 Mike Grier
//! A published **example** test harness for file-change-notification handlers,
//! built on [`windows-file-watcher`]'s `test-util` seam.
//!
//! Read it, cut-and-paste from it, adapt it. It is an exemplar of technique, not
//! a supported framework -- see the crate's `DESIGN-NOTES.md` for why a
//! composable framework is the wrong goal. It drives *your* notification handler
//! with synthetic, deterministic schedules -- no filesystem, no thread pool.
//!
//! # The pieces
//!
//! - [`Handler`] -- the one plug point; implement it for your handler.
//! - [`Schedule`] / [`NotificationSpec`] -- a harness-owned, serde-serializable
//!   description of a delivery schedule, converted to real `Notification`s at
//!   drive time.
//! - [`drive`] -- feed a real file-watcher `Receiver` with a schedule and
//!   dispatch each notification to your handler.
//! - [`generator`] -- a seeded generator of contract-legal schedules (chaos that
//!   stays inside file-watcher's documented contract).
//! - [`run`] / [`oracle`] -- drive a handler and detect a pathology (a panic, an
//!   invariant violation, or -- via [`run_with_deadline`] -- a wedge).
//! - [`Recording`] / [`recording`] -- bundle a pathology's schedule and outcome
//!   as a JSON artifact you can save, load, and replay as a regression.
//! - [`example_handler`] -- a small, realistic handler used by the tests (and,
//!   in later milestones, the `capture`/`replay` bins).
//!
//! Later milestones add `capture`/`replay` bin tools.
//!
//! # Fidelity limit
//!
//! This tests your handler's *reactions*, not whether file-watcher would ever
//! emit a given sequence. Schedules stay inside file-watcher's documented
//! contract, so a pathology found here is real; a bug that depends on your
//! handler's own internal nondeterminism replays as a lead, not a guaranteed
//! repro.
//!
//! [`windows-file-watcher`]: https://crates.io/crates/windows-file-watcher

#[cfg(windows)]
mod driver;
#[cfg(windows)]
pub mod generator;
#[cfg(windows)]
mod handler;
#[cfg(windows)]
pub mod oracle;
#[cfg(windows)]
pub mod recording;
#[cfg(windows)]
pub mod schedule;

#[cfg(windows)]
pub mod example_handler;

#[cfg(windows)]
pub use driver::drive;
#[cfg(windows)]
pub use generator::{Generator, GeneratorConfig, Rng};
#[cfg(windows)]
pub use handler::Handler;
#[cfg(windows)]
pub use oracle::{Outcome, PathologyKind, run, run_with_deadline};
#[cfg(windows)]
pub use recording::Recording;
#[cfg(windows)]
pub use schedule::{
    ChangeKindSpec, ChangeSpec, DesyncCauseSpec, FailureCodeSpec, FaultDetailSpec,
    FaultOperationSpec, NotificationSpec, OpenFailureSpec, OutcomeSpec, Schedule, VolumeSpec,
    WatchModeSpec,
};
// Re-exported so a handler author can name the type its `on` receives without a
// separate `windows-file-watcher` import in their tests.
#[cfg(windows)]
pub use windows_file_watcher::Notification;
