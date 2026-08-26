// Copyright (c) 2026 Mike Grier
//! A small, realistic example handler, used by the harness's own tests and bins.
//!
//! Replace it with your own [`Handler`]; it exists so the
//! examples and the `capture`/`replay` bins have something concrete to drive.

use std::collections::BTreeSet;

use windows_file_watcher::{ChangeKind, Notification, Outcome};

use crate::Handler;

/// Tracks the set of leaf names it believes present, from the change stream --
/// the core job a real directory-watching consumer does -- plus a couple of
/// simple observations. Deliberately tiny and dependency-free so it reads as an
/// example, not a library.
#[derive(Debug, Default)]
pub struct PresenceTracker {
    present: BTreeSet<String>,
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

    /// The names currently believed present.
    #[must_use]
    pub fn present(&self) -> &BTreeSet<String> {
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
            Notification::Batch { changes, .. } => {
                for change in changes {
                    let name = change.name.to_os_string().to_string_lossy().into_owned();
                    match &change.kind {
                        ChangeKind::Removed | ChangeKind::RenamedOldName => {
                            self.present.remove(&name);
                        }
                        _ => {
                            self.present.insert(name);
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
