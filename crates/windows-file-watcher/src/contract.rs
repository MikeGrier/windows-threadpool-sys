// Copyright (c) 2026 Mike Grier
//! Validating a notification stream against this crate's delivery contract.
//!
//! [`ContractChecker`] is the executable form of the *sequencing* rules --
//! ordering, tier-conditioned legality, cross-message continuity, terminality --
//! that a per-value type cannot carry. It exists so those rules have **one**
//! definition that everything binds to: this crate's own integration tests, the
//! example test harness's generator, and a consumer validating its own test
//! doubles or a captured production stream.
//!
//! That single definition is the point. A hand-written second copy of a
//! sequencing rule is not a check of the contract, it is a check of the copy --
//! see [restatement drift](../../../DESIGN-NOTES.md) in the workspace design
//! notes, where five of six findings across three review rounds were corrections
//! that had failed to reach every place stating them.
//!
//! # What it checks, and what it deliberately does not
//!
//! Only rules the contract actually establishes are asserted. Over-constraining
//! is the same defect as under-specifying, and this crate has produced it: the
//! harness generator once excluded a `Desync { QueueFull }` that a coarse watch
//! can legally report. Each rule below cites the decision it comes from.
//!
//! **Checked:**
//!
//! - **Terminality.** Nothing follows `Completion { Cancelled }`,
//!   `Completion { Failed }`, or `Desync { Stopped }` for that watch (D-30/D-46,
//!   and D-22 for the last).
//! - **Tier-conditioned emission.** Once a tier has been reported, a Coarse watch
//!   emits no `Batch` and no `Desync { Overflow }`, and a Detailed watch emits no
//!   `Desync { Coarse }` (D-17). Delegated to
//!   [`DesyncCause::is_reachable_in`](crate::DesyncCause::is_reachable_in) rather
//!   than restated here.
//! - **Volume-change continuity.** A watch's next `VolumeChanged.previous` equals
//!   its own prior `.current`, because `install` stores the confirmed identity
//!   (D-78).
//! - **Volume-change distinctness.** `previous` and `current` never compare equal,
//!   since the notification is only emitted when identity actually differs (D-78)
//!   and identity compares by serial alone (D-50).
//!
//! **Deliberately not checked**, because the contract permits them and asserting
//! otherwise would encode a rule the watcher does not keep:
//!
//! - A `Resumed` **without** a preceding `Suspended`. A route coalescing onto an
//!   already-faulted watcher joins after `enter_fault` sent its `Suspended`s, so
//!   it observes a bracket it never saw open (M14.2).
//! - A `Suspended` closed by `Desync { Stopped }` rather than `Resumed` (M14.2).
//! - `Established` arriving other than first, for that same mid-fault join
//!   (M14.2).
//! - A tier that **changes** between establishments: `reopen` re-resolves it every
//!   call (D-61), so Detailed then Coarse, or the reverse, is legal.
//! - A `Batch` **inside a fault bracket**. `on_completion` re-arms before it
//!   decodes, so a read that completed and then failed to re-arm calls
//!   `enter_fault` first and publishes the already-completed batch afterwards.
//!   Those changes were in hand and dropping them would be silent loss, so one
//!   batch may legally follow a bracket's opening notifications.
//!
//! **Not checkable from the stream alone**, and so not attempted: at most one
//! question is outstanding per subscription (D-28's standing-slot invariant). The
//! answer travels the *request* queue, which never appears here, so a second
//! `RetryQuestion` after the client answered the first is indistinguishable from
//! two outstanding at once.
//!
//! # Scope
//!
//! The checker is per-`WatchId` and order-sensitive, matching the contract:
//! ordering is defined *within* a subscription (D-12/D-26), not across
//! subscriptions, so interleaving between watches is never a violation.

use std::collections::HashMap;

use crate::directory::VolumeIdentity;
use crate::notify::DesyncCause;
use crate::queue::{Notification, Outcome, WatchId};
use crate::retry::WatchMode;

/// Why a notification stream violated the delivery contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractViolation {
    /// A notification arrived for a watch that had already ended.
    AfterTerminal {
        /// The watch that had already ended.
        watch: WatchId,
        /// How it had ended.
        terminator: Terminator,
    },
    /// A `Batch` arrived for a watch whose reported tier cannot produce one.
    BatchInCoarseTier {
        /// The watch affected.
        watch: WatchId,
    },
    /// A `Desync` cause arrived that the watch's reported tier cannot produce.
    CauseUnreachableInTier {
        /// The watch affected.
        watch: WatchId,
        /// The cause reported.
        cause: DesyncCause,
        /// The tier last reported for this watch.
        tier: WatchMode,
    },
    /// A `VolumeChanged.previous` did not continue from this watch's own prior
    /// `.current`.
    VolumeDiscontinuity {
        /// The watch affected.
        watch: WatchId,
    },
    /// A `VolumeChanged` carried a `previous` and `current` that compare equal,
    /// which describes a volume changing to itself.
    VolumeUnchanged {
        /// The watch affected.
        watch: WatchId,
    },
}

/// How a watch ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminator {
    /// `Completion { Cancelled }`.
    Cancelled,
    /// `Completion { Failed }`.
    Failed,
    /// `Desync { Stopped }`.
    Stopped,
}

/// One subscription's position in the contract.
#[derive(Debug, Default)]
struct WatchState {
    /// The tier last reported, if liveness reporting ever revealed one. `None`
    /// leaves every tier-conditioned rule unchecked rather than guessed: a
    /// subscription that did not opt into `report_liveness` never learns its
    /// tier, and assuming Detailed would invent a rule.
    tier: Option<WatchMode>,
    /// The identity this watch was last confirmed on, for D-78's continuity.
    volume: Option<VolumeIdentity>,
    /// Set once the watch has ended; any later notification is a violation.
    ended: Option<Terminator>,
}

/// Validates a notification stream against the delivery contract.
///
/// Feed every notification, in arrival order, to [`ContractChecker::observe`].
/// Notifications for different watches may interleave freely.
///
/// ```
/// use windows_file_watcher::{
///     ContractChecker, DesyncCause, Notification, Outcome, WatchId,
/// };
///
/// let mut checker = ContractChecker::new();
/// let watch = WatchId::from_raw(1);
///
/// checker
///     .observe(&Notification::Completion { watch, outcome: Outcome::Subscribed })
///     .expect("a registration outcome is legal");
/// checker
///     .observe(&Notification::Desync { watch, cause: DesyncCause::Stopped })
///     .expect("a terminal desync is legal");
///
/// // Nothing may follow a terminator for that watch.
/// let after = checker.observe(&Notification::Desync {
///     watch,
///     cause: DesyncCause::Overflow,
/// });
/// assert!(after.is_err());
/// ```
#[derive(Debug, Default)]
pub struct ContractChecker {
    watches: HashMap<WatchId, WatchState>,
}

impl ContractChecker {
    /// A checker with no watches seen yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate one notification and fold it into the per-watch state.
    ///
    /// # Errors
    ///
    /// Returns the first rule `notification` violates for its own watch. The
    /// checker's state is still advanced, so a caller that logs and continues
    /// sees subsequent violations rather than a cascade from one.
    pub fn observe(&mut self, notification: &Notification) -> Result<(), ContractViolation> {
        let watch = notification.watch();
        let state = self.watches.entry(watch).or_default();

        if let Some(terminator) = state.ended {
            return Err(ContractViolation::AfterTerminal { watch, terminator });
        }

        match notification {
            Notification::Batch { .. } => {
                if state.tier == Some(WatchMode::Coarse) {
                    return Err(ContractViolation::BatchInCoarseTier { watch });
                }
            }
            Notification::Desync { cause, .. } => {
                if cause.is_terminal() {
                    state.ended = Some(Terminator::Stopped);
                }
                if let Some(tier) = state.tier
                    && !cause.is_reachable_in(tier)
                {
                    return Err(ContractViolation::CauseUnreachableInTier {
                        watch,
                        cause: *cause,
                        tier,
                    });
                }
            }
            Notification::Completion { outcome, .. } => match outcome {
                Outcome::Cancelled => state.ended = Some(Terminator::Cancelled),
                Outcome::Failed { .. } => state.ended = Some(Terminator::Failed),
                Outcome::Subscribed | Outcome::Establishing => {}
            },
            Notification::Established { mode, .. } => {
                // Recorded, never compared against the previous tier: D-61
                // re-resolves the tier on every reopen, so a change is legal.
                state.tier = Some(*mode);
            }
            Notification::VolumeChanged {
                previous, current, ..
            } => {
                if previous == current {
                    return Err(ContractViolation::VolumeUnchanged { watch });
                }
                if let Some(known) = &state.volume
                    && known != previous
                {
                    return Err(ContractViolation::VolumeDiscontinuity { watch });
                }
                state.volume = Some(current.clone());
            }
            Notification::Suspended { .. }
            | Notification::Resumed { .. }
            | Notification::RetryQuestion { .. } => {}
        }

        Ok(())
    }

    /// Validate a whole stream, returning the first violation.
    ///
    /// # Errors
    ///
    /// Returns the first violation found, or `Ok(())` if the stream is legal.
    pub fn observe_all<'a>(
        &mut self,
        notifications: impl IntoIterator<Item = &'a Notification>,
    ) -> Result<(), ContractViolation> {
        for notification in notifications {
            self.observe(notification)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
