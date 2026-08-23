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
//!   file subscription (D-7) matches only its own leaf name. The comparison
//!   is on raw UTF-16 units, case-insensitively via `CompareStringOrdinal`'s
//!   ordinal case folding: the default Windows filesystem is case-insensitive
//!   but case-preserving, so `CreateFileW` accepts a target whose casing
//!   differs from the stored name, while a decoded notification always
//!   carries the name as actually stored -- an exact match would silently
//!   drop every one of those events, and an ASCII-only fold would still drop
//!   one whenever the differing case falls outside ASCII (PR #20 review
//!   response).
//!
//! A desync is never filtered by scope: it means "you may have missed something
//! in this directory," which is equally true for every subscription within it.

use wtf_string::Wtf16String;

use crate::notify::Change;
use crate::queue::{Sender, StandingSlot, WatchId};
use crate::watch::{RetryMode, VolumeChangePolicy};

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
            RouteScope::File { leaf } => {
                is_direct_child(name) && names_match_case_insensitively(name, leaf.as_units())
            }
        }
    }
}

/// Whether `name` names something directly within the watched directory, rather
/// than nested inside a subdirectory of it.
fn is_direct_child(name: &[u16]) -> bool {
    !name.contains(&SEPARATOR)
}

/// Whether `a` and `b` are the same name under Windows' default
/// case-insensitive (but case-preserving) filesystem semantics
/// (`is_direct_child` above establishes reach; this decides identity within
/// it, for [`RouteScope::File`]).
///
/// Uses `CompareStringOrdinal` with `bIgnoreCase = TRUE` -- the OS's own
/// ordinal (per-UTF-16-unit) case folding -- rather than an ASCII-only fold:
/// an ASCII fold silently drops a match whenever a subscription's leaf name
/// and the kernel's stored spelling differ only in a non-ASCII letter's case
/// (e.g. a stored `E9.txt` opened through a subscription spelling `C9.txt`),
/// which is exactly the kind of event this crate's completeness contract
/// (D-77) promises never to drop.
fn names_match_case_insensitively(a: &[u16], b: &[u16]) -> bool {
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let a_len = i32::try_from(a.len()).unwrap_or(i32::MAX);
    let b_len = i32::try_from(b.len()).unwrap_or(i32::MAX);
    // SAFETY: `a`/`b` are valid `u16` slices with lengths that fit the `i32`
    // counts just computed; `CompareStringOrdinal` only reads the first
    // `a_len`/`b_len` units of each and returns a plain `i32` result code.
    let result = unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, TRUE) };
    // A truncated length (from a name longer than `i32::MAX` units,
    // unreachable in practice) would only ever make an equal pair look
    // unequal, never the reverse, so the fallback direction is safe.
    result == CSTR_EQUAL
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
    /// Whether this subscription wants to confirm a volume change on reopen
    /// (D-78/M12).
    pub(crate) on_volume_change: VolumeChangePolicy,
    /// The standing reservation for this subscription's fault question or
    /// volume-change question (D-27/D-28/M12), present iff it can ever need
    /// one (`retry == Interactive` or `on_volume_change == Confirm`).
    /// `report_liveness` alone never creates one: `Suspended`/`Resumed`/
    /// `Established` all ride the ordinary best-effort queue like any other
    /// observation (D-57).
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
