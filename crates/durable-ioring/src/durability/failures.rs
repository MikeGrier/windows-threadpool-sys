// Copyright (c) 2026 Mike Grier
// Split from durability.rs at 1deffac3.
//! A failure's resolution and the inventory (DI-D-12, DI-D-34, DI-D-41): importing a failure,
//! resolving failures, handing out a token, and listing the unresolved ones.

use alloc::vec::Vec;

use super::{Due, Durability, Event, Key, Reach, Standing, V, suspects};
use crate::alloc_tags::{tag, tagged};
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::ids::{FailureId, FailureToken};
use crate::types::{
    FailureCause, FailureInfo, Marking, Resolution, ResolveError, ResolveRefusal, SuspectWrite,
};

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
                    let (lists, writes, kept) = (&self.lists, &self.writes, &self.kept);
                    let lineages = &mut self.lineages;
                    for write in suspects(lists, writes, kept, &failure) {
                        if let Some(state) = lineages.get_mut(&write.lineage) {
                            tagged(tag::DURABILITY_RESOLVE_ABANDONED, || {
                                state.abandoned.insert(write.epoch);
                            });
                        }
                    }
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
                    failure.standing = Standing::Resolved {
                        marks_before: self.kept.order(),
                    };
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
            Standing::Healing { .. } | Standing::Resolved { .. } => None,
        }
    }

    /// The unresolved failures, in observation order.
    pub(crate) fn failures(&self) -> Vec<FailureInfo<V<E, I>>> {
        let mut infos = tagged(tag::DURABILITY_FAILURES_INFOS, || {
            Vec::with_capacity(self.failures.len())
        });
        self.failure_infos(|info| tagged(tag::DURABILITY_FAILURES_INFO, || infos.push(info)));
        infos
    }

    /// Each unresolved failure, in observation order.
    pub(crate) fn failure_infos(&self, mut visit: impl FnMut(FailureInfo<V<E, I>>)) {
        for failure in &self.failures {
            let token_live = match &failure.standing {
                Standing::Open => self.tokens.is_live(failure.token, failure.id),
                Standing::Healing { .. } => true,
                Standing::Resolved { .. } => continue,
            };
            let cause = tagged(tag::DURABILITY_FAILURES_CAUSE, || failure.cause.clone());
            visit(FailureInfo {
                id: failure.id,
                cause,
                observed: failure.observed,
                token_live,
            });
        }
    }

    /// Give `visit` each write failure `id` suspects, in push order (DI-D-108): `false` if its
    /// records are not held -- it is not this instance's, or its `Abandoned` or `Healed` was popped.
    pub(crate) fn failure_suspects(
        &self,
        id: FailureId,
        visit: impl FnMut(SuspectWrite<V<E, I>>),
    ) -> bool {
        let Some(failure) = self.failures.iter().find(|failure| failure.id == id) else {
            return false;
        };
        self.visit_suspects(failure, visit);
        true
    }

    /// Give `visit` each marking failure `id` has gained, in the order observed, those of a
    /// resolved failure up to its resolution (DI-D-108): `false` if its records are not held.
    pub(crate) fn failure_markings(
        &self,
        id: FailureId,
        visit: impl FnMut(Marking<V<E, I>>),
    ) -> bool {
        let Some(failure) = self.failures.iter().find(|failure| failure.id == id) else {
            return false;
        };
        self.markings(failure).for_each(visit);
        true
    }

    /// The consumer popped failure `id`'s `Abandoned` or `Healed` entry: its records go, and with
    /// them the places, the token slot and the import's list it held (DI-D-108).
    pub(crate) fn forget(&mut self, id: FailureId) {
        let Some(index) = self.failures.iter().position(|failure| {
            failure.id == id && matches!(failure.standing, Standing::Resolved { .. })
        }) else {
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
        super::exchange::Snapshot {
            suspect,
            markings: self.markings(failure).collect(),
        }
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
