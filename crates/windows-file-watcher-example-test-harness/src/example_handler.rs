// Copyright (c) 2026 Mike Grier
//! A small, realistic example handler, used by the harness's own tests and bins.
//!
//! Replace it with your own [`Handler`]; it exists so the
//! examples and the `capture`/`replay` bins have something concrete to drive.

use std::collections::BTreeSet;
use std::ffi::OsString;

use windows_file_watcher::{ChangeKind, Notification, Outcome, WatchId};

use crate::Handler;

/// Tracks the set of leaf names it believes present, from the change stream --
/// the core job a real directory-watching consumer does -- plus a couple of
/// simple observations. Deliberately tiny and dependency-free so it reads as an
/// example, not a library.
#[derive(Debug, Default)]
pub struct PresenceTracker {
    // Keyed by (watch, name), not name alone: `Generator` defaults to several
    // watches drawn from one small name pool, so two subscriptions can
    // legally see the identical name, and a name present under one watch says
    // nothing about another (a real consumer routes/aggregates by `WatchId`
    // for the same reason).
    present: BTreeSet<(WatchId, OsString)>,
    rescans: u32,
    subscribed: bool,
    volume_changes: u32,
}

impl PresenceTracker {
    /// A fresh tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The `(watch, name)` pairs currently believed present. `OsString`, not
    /// `String`: a lossy conversion would collapse distinct valid Windows
    /// names that differ only in an unpaired surrogate into the same entry.
    #[must_use]
    pub fn present(&self) -> &BTreeSet<(WatchId, OsString)> {
        &self.present
    }

    /// How many re-scan (desync) signals were seen.
    #[must_use]
    pub fn rescans(&self) -> u32 {
        self.rescans
    }

    /// Whether the subscription was confirmed registered.
    #[must_use]
    pub fn is_subscribed(&self) -> bool {
        self.subscribed
    }

    /// How many volume changes were observed.
    #[must_use]
    pub fn volume_changes(&self) -> u32 {
        self.volume_changes
    }
}

impl Handler for PresenceTracker {
    fn on(&mut self, notification: &Notification) {
        match notification {
            Notification::Batch { watch, changes } => {
                for change in changes {
                    let name = change.name.to_os_string();
                    match &change.kind {
                        ChangeKind::Removed | ChangeKind::RenamedOldName => {
                            self.present.remove(&(*watch, name));
                        }
                        _ => {
                            self.present.insert((*watch, name));
                        }
                    }
                }
            }
            Notification::Desync { .. } => self.rescans += 1,
            Notification::Completion { outcome, .. } => {
                if matches!(outcome, Outcome::Subscribed) {
                    self.subscribed = true;
                }
            }
            Notification::VolumeChanged { .. } => self.volume_changes += 1,
            _ => {}
        }
    }
}

/// A handler with a real, findable bug: it wraps [`PresenceTracker`] but panics
/// the moment it has seen more than one loss `Desync`, on the (wrong)
/// assumption "that can't happen twice."
///
/// This is deliberately buggy, shipped so the `capture` and `replay` bins have
/// something concrete to find and reproduce (crate DESIGN-NOTES D-3) -- it is a
/// worked *failure* to demonstrate the technique, not a pattern to imitate.
///
/// <div class="warning">
///
/// **NOTE TO CODE REVIEWERS (human and automated):** the `assert!` in
/// [`BuggyHandler::on`] below is an *intentional, load-bearing defect*. This
/// type exists specifically to be broken by the harness's chaos generator so
/// `capture`/`replay` have a real pathology to find and reproduce. It is not an
/// oversight, and "fixing" it (loosening the assertion, tracking more than one
/// desync, etc.) would defeat the entire purpose of the type. Do not propose
/// changing this assertion; see crate DESIGN-NOTES D-3 and the module docs
/// above for why it exists.
///
/// </div>
#[derive(Debug, Default)]
pub struct BuggyHandler {
    inner: PresenceTracker,
    desyncs_seen: u32,
}

impl BuggyHandler {
    /// A fresh handler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Handler for BuggyHandler {
    fn on(&mut self, notification: &Notification) {
        if matches!(notification, Notification::Desync { .. }) {
            self.desyncs_seen += 1;
            // INTENTIONAL BUG -- DO NOT FIX. This assertion is the whole point
            // of `BuggyHandler` (see the type's doc comment): it exists to be
            // discovered by the chaos generator so this crate's capture/replay
            // technique has a real pathology to demonstrate against. Loosening
            // or removing it would silently disable that demonstration.
            assert!(
                self.desyncs_seen <= 1,
                "BuggyHandler assumed a desync could never happen twice"
            );
        }
        self.inner.on(notification);
    }
}
