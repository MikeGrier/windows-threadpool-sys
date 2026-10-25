// Copyright (c) 2026 Mike Grier
// Split from durability.rs at 1deffac3.
//! A failure's resolution and the inventory (DI-D-12, DI-D-34, DI-D-41): importing a failure,
//! resolving failures, handing out a token, and listing the unresolved ones.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::{Due, Durability, Event, Key, Reach, Release, Standing, V, suspects};
use crate::alloc_tags::{tag, tagged};
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::ids::{FailureId, FailureToken};
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
                    let failure = self.failures.remove(index);
                    self.free_tokens.push(failure.token);
                    let markings = tagged(tag::DURABILITY_RESOLVE_MARKINGS, || {
                        self.markings(&failure).collect()
                    });
                    let suspect = self.suspect_set(&failure, tag::DURABILITY_RESOLVE_SUSPECT);
                    let (lists, writes, kept) = (&self.lists, &self.writes, &self.kept);
                    let lineages = &mut self.lineages;
                    for write in suspects(lists, writes, kept, &failure) {
                        if let Some(state) = lineages.get_mut(&write.lineage) {
                            tagged(tag::DURABILITY_RESOLVE_ABANDONED, || {
                                state.abandoned.insert(write.epoch);
                            });
                        }
                    }
                    tagged(tag::DURABILITY_RESOLVE_ABANDONED_EVENT, || {
                        due.events.push(Event::Abandoned {
                            failure: failure.id,
                            suspect,
                            markings,
                        });
                    });
                    self.abandon_held(&failure, &mut due);
                    self.release_suspects(&failure);
                    self.release_reach(failure.reach);
                }
                Resolution::Heal => {
                    let lineages = &self.lineages;
                    let failure = &self.failures[index];
                    let pending = tagged(tag::DURABILITY_RESOLVE_HEAL_PENDING, || {
                        let mut pending = BTreeMap::new();
                        for write in suspects(&self.lists, &self.writes, &self.kept, failure) {
                            if pending.contains_key(&write.lineage) {
                                continue;
                            }
                            if let Some(state) =
                                lineages.get(&write.lineage).filter(|state| !state.ended)
                            {
                                let release = Release {
                                    after: state.next_seal,
                                    through: state.pushed_through,
                                };
                                pending.insert(write.lineage, release);
                            }
                        }
                        pending
                    });
                    self.failures[index].standing = Standing::Healing { pending };
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
            Standing::Healing { .. } => None,
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
            let cause = tagged(tag::DURABILITY_FAILURES_CAUSE, || failure.cause.clone());
            let markings = tagged(tag::DURABILITY_FAILURES_MARKINGS, || {
                self.markings(failure).collect()
            });
            let info = FailureInfo {
                id: failure.id,
                cause,
                suspect: self.suspect_set(failure, tag::DURABILITY_FAILURES_SUSPECT),
                observed: failure.observed,
                markings,
                token_live: match &failure.standing {
                    Standing::Open => self.tokens.is_live(failure.token, failure.id),
                    Standing::Healing { .. } => true,
                },
            };
            visit(info);
        }
    }

    /// How many unresolved failures were imported: each counts from its import until it is
    /// resolved, a heal waiting for its seal included (DI-D-107).
    pub(crate) fn imports(&self) -> usize {
        self.failures
            .iter()
            .filter(|failure| matches!(failure.cause, FailureCause::Imported { .. }))
            .count()
    }
}
