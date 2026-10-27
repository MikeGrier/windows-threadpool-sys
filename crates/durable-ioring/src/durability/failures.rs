// Copyright (c) 2026 Mike Grier
// Split from durability.rs at 1deffac3.
//! A failure's resolution and the inventory (DI-D-12, DI-D-34, DI-D-41): importing a failure,
//! resolving failures, handing out a token, and listing the unresolved ones.

use alloc::vec::Vec;

use super::{Due, Durability, Event, Key, Reach, Standing, V};
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::ids::{FailureId, FailureToken};
#[cfg(test)]
use crate::types::SuspectWrite;
use crate::types::{FailureCause, FailureInfo, Resolution, ResolveError, ResolveRefusal};

impl<E: EpochId + 'static, T: Clone, I: Instant, K: StampClock<Instant = I>>
    Durability<E, T, I, K>
{
    /// A failure the consumer learned of outside the instance (DI-D-12 (h)), reaching the writes
    /// `reach` covers -- of lineage `only`, if one is named.
    pub(crate) fn import(
        &mut self,
        cause: FailureCause<V<E, I>>,
        reach: &Reach,
        only: Option<Key>,
    ) -> (FailureId, Due<E, T, I>) {
        let mut due = self.fresh();
        let id = self.observe(cause, reach, only, &mut due);
        (id, self.advance(due))
    }

    /// Resolve failures, validated whole and applied together: the tokens of a refused call come
    /// back, and nothing changes.
    pub(crate) fn resolve(
        &mut self,
        items: Vec<(FailureToken, Resolution)>,
    ) -> Result<Due<E, T, I>, ResolveError<V<E, I>>> {
        if let Some(foreign) = items
            .iter()
            .map(|(token, _)| token.id)
            .find(|id| id.instance != self.instance)
        {
            return Err(ResolveError {
                reason: ResolveRefusal::Foreign(foreign),
                returned: items,
            });
        }
        let mut due = self.fresh();
        for (token, resolution) in items {
            // A live token's failure is unresolved: resolving it consumed its only other token.
            let Some(index) = self.failures.iter().position(|f| f.id == token.id) else {
                self.inconsistent(
                    "a live token for a failure not in the inventory",
                    format_args!("failure {:?}, {resolution:?}", token.id),
                );
                continue;
            };
            match resolution {
                Resolution::Abandon => {
                    // Out of the table while it is read and its holds are acted on, and back in its
                    // place once resolved: it holds what it held until its entry is popped.
                    let mut failure = self.failures.remove(index);
                    self.abandon_epochs(&failure);
                    #[cfg(test)]
                    let snapshot = self.snapshot(&failure);
                    self.emit(
                        &mut due,
                        Event::Abandoned {
                            failure: failure.id,
                            #[cfg(test)]
                            snapshot,
                        },
                    );
                    self.abandon_held(&failure, &mut due);
                    failure.standing = Standing::Resolved;
                    self.failures.insert(index, failure);
                }
                Resolution::Heal => {
                    // What it waits for in each lineage is read when a seal finishes (DI-D-114).
                    self.failures[index].standing = Standing::Healing {
                        at: self.heal_now(),
                    };
                }
            }
        }
        self.release_healed(&mut due);
        Ok(self.advance(due))
    }

    /// A token for `id`, if it is an unresolved failure whose token is not live.
    pub(crate) fn take_token(&mut self, id: FailureId) -> Option<FailureToken> {
        let failure = self.failures.iter().find(|f| f.id == id)?;
        match failure.standing {
            Standing::Open => FailureToken::mint(id, &self.tokens, failure.token, false),
            Standing::Healing { .. } | Standing::Resolved => None,
        }
    }

    /// The unresolved failures, in observation order, each copied out for a test to keep.
    #[cfg(test)]
    pub(crate) fn failures_listed(&self) -> Vec<crate::types::FailureListed<V<E, I>>> {
        let mut listed = Vec::new();
        self.failure_infos(|info| listed.push(info.into()));
        listed
    }

    /// Give `visit` each unresolved failure, in observation order, its cause lent (DI-D-103).
    pub(crate) fn failure_infos(&self, mut visit: impl FnMut(FailureInfo<'_, V<E, I>>)) {
        for failure in &self.failures {
            let token_live = match &failure.standing {
                Standing::Open => self.tokens.is_live(failure.token, failure.id),
                Standing::Healing { .. } => true,
                Standing::Resolved => continue,
            };
            visit(FailureInfo {
                id: failure.id,
                cause: &failure.cause,
                observed: failure.observed,
                token_live,
            });
        }
    }

    /// Each write failure `id` suspects, in push order, for a test to compare with what the
    /// observer was told; `None` if its records are not held -- it is not this instance's, or its
    /// `Abandoned` or `Healed` was popped.
    #[cfg(test)]
    pub(crate) fn suspects_listed(&self, id: FailureId) -> Option<Vec<SuspectWrite<V<E, I>>>> {
        let failure = self.failures.iter().find(|failure| failure.id == id)?;
        let mut listed = Vec::new();
        self.visit_suspects(failure, |write| listed.push(write));
        Some(listed)
    }

    /// The consumer popped failure `id`'s `Abandoned` or `Healed` entry: its records go, and with
    /// them the places, the token slot and the import's list it held (DI-D-108).
    pub(crate) fn forget(&mut self, id: FailureId) {
        let Some(index) = self
            .failures
            .iter()
            .position(|failure| failure.id == id && matches!(failure.standing, Standing::Resolved))
        else {
            self.inconsistent(
                "a resolution entry was popped for a failure not resolved",
                format_args!("failure {id:?}"),
            );
            return;
        };
        let failure = self.failures.remove(index);
        self.free_tokens.push(failure.token);
        self.release_suspects(&failure);
        self.release_reach(failure.reach);
    }

    /// `failure`'s records as they stand, for a test to compare with an event about it.
    #[cfg(test)]
    pub(super) fn snapshot(
        &self,
        failure: &super::Failure<E, I>,
    ) -> super::exchange::Snapshot<E, I> {
        let mut suspect = Vec::new();
        self.visit_suspects(failure, |write| suspect.push(write));
        super::exchange::Snapshot { suspect }
    }

    /// How many imported failures the table holds: each counts from its import until its
    /// resolution entry is popped, a heal waiting for its seal included (DI-D-107, DI-D-108).
    pub(crate) fn imports(&self) -> usize {
        self.failures
            .iter()
            .filter(|failure| matches!(failure.cause, FailureCause::Imported { .. }))
            .count()
    }
}
