// Copyright (c) Mike Grier.

//! Which capturable aspects to collect.
//!
//! A capture set names the aspects that can be **read** off the calling thread.
//! Declared aspects are not in this vocabulary at all: there is nothing to
//! collect for them, so they are stated by the caller instead -- see
//! [`crate::declared`].
//!
//! # There is deliberately no `Default` implementation
//!
//! The default set is a **named constant**, [`CaptureSet::DEFAULT`], which a
//! caller must name to get.
//!
//! That is not ceremony. The workspace decision that this composite is
//! exhaustively enumerated rests on its field list being contract surface: a
//! silently added field is a silent semantic change. An implicit default has the
//! same property in a worse form, because growing it changes behaviour for
//! callers who never named it and have no diff to review. Naming it makes
//! growth a visible change to a named thing, lets a caller who wants stability
//! list aspects explicitly, and gives a caller who takes the default somewhere
//! to go and read what it contains.
//!
//! # Example
//!
//! ```
//! use windows_thread_ambient_sys::capture_set::{CaptureSet, CapturableAspect};
//!
//! // The default is opted into by name, never inherited by accident.
//! let set = CaptureSet::DEFAULT;
//! assert!(set.contains(CaptureSet::IMPERSONATION));
//!
//! // TxF is excluded from the default; ask for it deliberately.
//! assert!(!set.contains(CaptureSet::TRANSACTION));
//! let with_txf = set.union(CaptureSet::TRANSACTION);
//! assert!(with_txf.contains(CaptureSet::TRANSACTION));
//!
//! // A caller that wants stability names its aspects rather than taking a set
//! // whose membership may grow.
//! let pinned = CaptureSet::IMPERSONATION.union(CaptureSet::ERROR_MODE);
//! assert_eq!(pinned.aspects().count(), 2);
//! assert!(pinned.aspects().any(|a| a == CapturableAspect::ErrorMode));
//! ```

use std::fmt;

/// The bit each aspect occupies in a [`CaptureSet`].
///
/// Internal to this crate and not a wire format, so the values carry no
/// compatibility obligation; they exist so no bare literal appears in the logic.
mod bit {
    pub(super) const IMPERSONATION: u8 = 1 << 0;
    pub(super) const ERROR_MODE: u8 = 1 << 1;
    pub(super) const TRANSACTION: u8 = 1 << 2;
}

/// One aspect that can be read off the calling thread.
///
/// Declared aspects are absent by construction: there is nothing to capture for
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CapturableAspect {
    /// The thread's impersonation context.
    Impersonation,
    /// The thread error mode.
    ErrorMode,
    /// The thread's current TxF transaction.
    Transaction,
}

impl CapturableAspect {
    /// Every capturable aspect.
    ///
    /// [`CaptureSet::ALL`] is derived from this list rather than restating it,
    /// so an aspect added here joins that set automatically instead of leaving
    /// it quietly stale.
    pub const EVERY: &'static [Self] = &[Self::Impersonation, Self::ErrorMode, Self::Transaction];

    const fn bit(self) -> u8 {
        match self {
            Self::Impersonation => bit::IMPERSONATION,
            Self::ErrorMode => bit::ERROR_MODE,
            Self::Transaction => bit::TRANSACTION,
        }
    }

    /// The singleton set containing just this aspect.
    #[must_use]
    pub const fn as_set(self) -> CaptureSet {
        CaptureSet { bits: self.bit() }
    }
}

impl fmt::Display for CapturableAspect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Impersonation => "impersonation",
            Self::ErrorMode => "error mode",
            Self::Transaction => "transaction",
        })
    }
}

/// Which capturable aspects a capture should collect.
///
/// There is no `Default` implementation; see the module documentation.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureSet {
    bits: u8,
}

const fn derive_all() -> CaptureSet {
    let mut bits = 0u8;
    let mut index = 0;
    while index < CapturableAspect::EVERY.len() {
        bits |= CapturableAspect::EVERY[index].bit();
        index += 1;
    }
    CaptureSet { bits }
}

impl CaptureSet {
    /// Collect nothing.
    ///
    /// Every aspect is then [`Captured::NotCaptured`](crate::Captured::NotCaptured),
    /// which leaves the target thread's own values alone.
    pub const NONE: Self = Self { bits: 0 };

    /// Just the impersonation context.
    pub const IMPERSONATION: Self = CapturableAspect::Impersonation.as_set();

    /// Just the thread error mode.
    pub const ERROR_MODE: Self = CapturableAspect::ErrorMode.as_set();

    /// Just the current TxF transaction.
    pub const TRANSACTION: Self = CapturableAspect::Transaction.as_set();

    /// The recommended starting point: impersonation and the thread error mode.
    ///
    /// **Adding an aspect to this set is a breaking change**, and that is the
    /// reason it exists as a name rather than as a `Default` implementation.
    ///
    /// TxF is deliberately excluded. It is deprecated by Microsoft, capturing it
    /// costs a lazy `ntdll` binding a caller may never need, and -- the reason
    /// that actually decides it -- a captured transaction enlists remoted work
    /// in a transaction the caller may commit or roll back while that work is
    /// still running. That is a hazard to opt into deliberately, not one to
    /// acquire by taking a default. Add [`TRANSACTION`](Self::TRANSACTION) when
    /// you mean it.
    pub const DEFAULT: Self = Self {
        bits: bit::IMPERSONATION | bit::ERROR_MODE,
    };

    /// Every capturable aspect this version knows.
    ///
    /// **This set grows.** Membership is its meaning, so a later version adding
    /// an aspect will capture it here without further notice. A caller that
    /// needs a fixed set should name its aspects instead.
    pub const ALL: Self = derive_all();

    /// Both sets' aspects.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    /// This set without `other`'s aspects.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self {
            bits: self.bits & !other.bits,
        }
    }

    /// Whether every aspect of `other` is present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    /// Whether nothing would be collected.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// The aspects in this set, in a stable order.
    pub fn aspects(self) -> impl Iterator<Item = CapturableAspect> {
        CapturableAspect::EVERY
            .iter()
            .copied()
            .filter(move |aspect| self.bits & aspect.bit() != 0)
    }
}

impl fmt::Debug for CaptureSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("CaptureSet(none)");
        }
        f.write_str("CaptureSet(")?;
        for (index, aspect) in self.aspects().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{aspect}")?;
        }
        f.write_str(")")
    }
}

impl From<CapturableAspect> for CaptureSet {
    fn from(aspect: CapturableAspect) -> Self {
        aspect.as_set()
    }
}

#[cfg(test)]
mod tests;
