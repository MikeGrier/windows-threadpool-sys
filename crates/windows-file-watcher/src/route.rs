// Copyright (c) 2026 Mike Grier
//! Per-subscription routing within one coalesced directory watcher (D-6/D-7).
//!
//! A directory is watched once regardless of how many subscriptions target
//! entries within it (D-6). What a [`Route`] decides is which of a decoded
//! batch's records belong to *this* subscription. Two things vary:
//!
//! - **Reach**: a recursive directory subscription matches a record at any
//!   depth; a shallow one, or a file subscription, matches only a record with
//!   no path separator in its relative name -- a direct child of the directory
//!   actually opened.
//! - **Name**: a directory subscription matches every name within its reach; a
//!   file subscription (D-7) matches only its own leaf name, exactly. The
//!   comparison is on raw UTF-16 units, never a locale-aware case fold: this
//!   crate never re-interprets a name the kernel handed it, and a case-sensitive
//!   match is the honest default until a client asks for anything richer.
//!
//! A desync is never filtered by scope: it means "you may have missed something
//! in this directory," which is equally true for every subscription within it.

use wtf_string::Wtf16String;

use crate::notify::Change;
use crate::queue::{Sender, StandingSlot, WatchId};
use crate::watch::RetryMode;

/// The backslash that separates path components in a relative name.
const SEPARATOR: u16 = b'\\' as u16;

/// What part of a directory one subscription wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteScope {
    /// The directory itself. `subtree` reaches every depth; otherwise only
    /// direct children.
    Directory {
        /// Whether nested changes are reported as well.
        subtree: bool,
    },
    /// One file within the directory (D-7): always a direct child, matched by
    /// its exact leaf name. Never recursive -- a file has no subtree.
    File {
        /// The file's name within the directory that was actually opened.
        leaf: Wtf16String,
    },
}

impl RouteScope {
    /// Whether this scope needs the kernel's own `bWatchSubtree` reach.
    ///
    /// Only a recursive directory subscription does. A file subscription is
    /// always a direct child of the directory that is opened, regardless of
    /// whether some *other* subscription on the same directory needs subtree
    /// reach for its own purposes.
    pub(crate) fn needs_kernel_subtree(&self) -> bool {
        matches!(self, RouteScope::Directory { subtree: true })
    }

    /// Whether `name` -- raw UTF-16 units, exactly as the kernel reported them --
    /// belongs to this scope.
    fn matches(&self, name: &[u16]) -> bool {
        match self {
            RouteScope::Directory { subtree: true } => true,
            RouteScope::Directory { subtree: false } => is_direct_child(name),
            RouteScope::File { leaf } => is_direct_child(name) && name == leaf.as_units(),
        }
    }
}

/// Whether `name` names something directly within the watched directory, rather
/// than nested inside a subdirectory of it.
fn is_direct_child(name: &[u16]) -> bool {
    !name.contains(&SEPARATOR)
}

/// One subscription's place within a coalesced directory watcher: what it wants
/// and where its notifications go.
pub(crate) struct Route {
    /// The identifier every notification delivered through this route carries.
    pub(crate) watch: WatchId,
    /// What within the directory this subscription wants.
    pub(crate) scope: RouteScope,
    /// This subscription's session sink (D-11).
    pub(crate) sink: Sender,
    /// How this subscription wants faults recovered (D-27).
    pub(crate) retry: RetryMode,
    /// Whether this subscription wants `Suspended`/`Resumed`/`Established`
    /// (D-13).
    pub(crate) report_liveness: bool,
    /// The standing reservation for this subscription's fault question
    /// (D-27/D-28), present iff it can ever be asked one (`retry ==
    /// Interactive`). `report_liveness` alone never creates one:
    /// `Suspended`/`Resumed`/`Established` all ride the ordinary best-effort
    /// queue like any other observation (D-57).
    pub(crate) fault_slot: Option<StandingSlot>,
}

impl Route {
    /// The subset of `changes` this route should see, in the order they arrived.
    ///
    /// Cloned rather than referenced: more than one route can match an
    /// overlapping subset (a subtree route and a file route both watching the
    /// same directory, say), so no single route may claim exclusive ownership of
    /// the decoded batch.
    pub(crate) fn select(&self, changes: &[Change]) -> Vec<Change> {
        changes
            .iter()
            .filter(|change| self.scope.matches(change.name.as_wide()))
            .cloned()
            .collect()
    }
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Route")
            .field("watch", &self.watch)
            .field("scope", &self.scope)
            .field("retry", &self.retry)
            .field("report_liveness", &self.report_liveness)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
