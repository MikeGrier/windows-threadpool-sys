// Copyright (c) 2026 Mike Grier
//! The retry protocol (D-27): fixed per-operation defaults, a floor, and the
//! earliest-answer reduction over a fault's asked subscriptions.
//!
//! `WatchOptions` carries only [`crate::watch::RetryMode`]'s two variants --
//! `Defaults` or `Interactive` -- with no numeric override. So unlike the
//! aspirational "soonest-recovering reduction" language in the crate's fault
//! model notes (growth multiplier, cap, jitter, per-error-kind override), there
//! is no field anywhere a caller could use to state one of those, and this
//! module does not invent behaviour nobody can configure. What is implemented is
//! exactly D-27's literal text: a fixed delay per failing operation, asked of
//! every interactive subscription, resolved to the earliest answer (a decliner
//! counted at the default), and clamped to a floor.

use std::time::Duration;

/// The delay used when nothing overrides it, and nobody was asked. Also every
/// interactive decliner's answer.
///
/// One value for both operations, per `Azure/m`'s shipped code (D-27); kept as
/// two constants because D-15 nonetheless keeps the operations texturally
/// distinct and a future divergence should not have to touch every call site.
pub(crate) const OPEN_DEFAULT_DELAY: Duration = Duration::from_millis(500);
pub(crate) const ARM_DEFAULT_DELAY: Duration = Duration::from_millis(500);

/// No answer, from anyone, is ever honoured below this. Protects against a hot
/// retry loop driven by a misbehaving or miscoded interactive answer.
pub(crate) const FLOOR: Duration = Duration::from_millis(50);

/// Which failing operation a fault arose from, matching D-15's reopen-retry /
/// rearm-retry split. Each has its own default delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaultOperation {
    /// The directory (or a file target's parent) could not be opened.
    Open,
    /// A `ReadDirectoryChangesW` completion reported a failure, or resubmitting
    /// one did.
    Arm,
}

impl FaultOperation {
    /// The delay used when nobody interactive is asked, or everyone declines.
    #[must_use]
    pub(crate) fn default_delay(self) -> Duration {
        match self {
            FaultOperation::Open => OPEN_DEFAULT_DELAY,
            FaultOperation::Arm => ARM_DEFAULT_DELAY,
        }
    }
}

/// The mode a watch is established in (D-13/D-17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WatchMode {
    /// `ReadDirectoryChangesW` on a `ThreadpoolIo`.
    Detailed,
    /// `FindFirstChangeNotification` on a `ThreadpoolWait` (M6): the universal
    /// floor, reached when the volume does not support the detailed API.
    Coarse,
}

/// Clamp a resolved delay to the floor.
#[must_use]
pub(crate) fn clamp(delay: Duration) -> Duration {
    delay.max(FLOOR)
}

#[cfg(test)]
mod tests;
