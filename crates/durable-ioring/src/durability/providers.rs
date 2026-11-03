// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider, in the core (DI-D-27, DI-D-44): which domains it serves,
//! the parts of a seal it is asked about, its answers, and when a write is covered.
//!
//! A seal asks the built-in default for a flush per file it answers for, and the provider for each
//! domain it serves that the seal's writes touched; [`Durability::cover`] covers a write once every
//! part its file depends on has answered success. A failed or abandoned domain is observed as a
//! failure reaching the files whose declared domains contain it, and every file declared with none
//! -- `Reach::Domain` -- which is narrower than a failed flush's reach, the
//! intersection with the failed file's whole set (DI-D-21).

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{Due, Durability, Key, MarkingKind, Part, Reach, SealStage, Span, Stage, V, Write};
use crate::capacity::within;
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::diagnostics::FlushOutcome;
use crate::domains::DomainId;
use crate::ids::OpId;
use crate::types::{FailureCause, FlushDomain, IoTargetKey};

/// Note, in `touched`, each of the `domains` a write to a file declared with them names that the
/// provider serves: ascending, each once, within the room `touched` was made with, which is the
/// domains served, so a domain is handed back (`Err`) only by an inconsistency.
pub(super) fn touch(
    touched: &mut Vec<DomainId>,
    served: &[DomainId],
    domains: &[DomainId],
) -> Result<(), DomainId> {
    for &domain in domains.iter().filter(|domain| served.contains(domain)) {
        let Err(at) = touched.binary_search(&domain) else {
            continue;
        };
        if touched.len() == touched.capacity() {
            return Err(domain);
        }
        touched.insert(at, domain);
    }
    Ok(())
}

impl<E: EpochId + 'static, T: Clone, I: Instant, K: StampClock<Instant = I>>
    Durability<E, T, I, K>
{
    /// The domains the consumer's provider serves, sorted, as interned when the instance was built,
    /// and the consumer's name for each, in the same order, which a flush pair reports.
    pub(crate) fn serve(&mut self, domains: Arc<[DomainId]>, names: Vec<FlushDomain>) {
        self.served = domains;
        // The table was made for the domains the provider serves, so this never refuses.
        let named = self
            .served_names
            .try_extend(self.served.iter().copied().zip(names));
        if named.is_err() {
            self.inconsistent(
                "the provider's domains outnumber the table made for them",
                format_args!("{} domains", self.served.len()),
            );
        }
    }

    /// How many (seal, domain) entries the table of what the seals asked holds.
    #[cfg(test)]
    pub(crate) fn seal_domains_held(&self) -> usize {
        self.seal_domains.len()
    }

    /// The domains `lineage`'s seal `seal` asked the provider about, ascending (DI-D-115).
    pub(crate) fn asked(&self, lineage: Key, seal: u64) -> impl Iterator<Item = DomainId> + '_ {
        self.served
            .iter()
            .copied()
            .filter(move |&domain| self.seal_domains.get(&(lineage, seal, domain)).is_some())
    }

    /// The writes named to `lineage`'s seal `seal` for the provider, in push order: each with its
    /// file and the domains that file was declared with. The request the provider is made is read
    /// from these as it is drained, not kept from when the seal started (DI-D-115): a write stays
    /// named to its seal until the seal's parts have answered.
    pub(crate) fn named_for_provider(
        &self,
        lineage: Key,
        seal: u64,
    ) -> impl Iterator<Item = (OpId, IoTargetKey, &[DomainId])> + '_ {
        self.writes
            .values()
            .filter(move |write| {
                write.lineage == lineage
                    && write.stage == Stage::Named(seal)
                    && write.routing.provider
            })
            .map(|write| (write.op, write.file, self.lists.get(write.domains)))
    }

    /// The provider answered `domain` of `lineage`'s seal `seal`: `Ok` for success, or the cause of
    /// its failure -- `FailureCause::Provider`, or `FailureCause::ProviderAbandoned` for a completion dropped
    /// unanswered. Success covers what it completes; a failure is observed at once, and is the
    /// seal's answer. An answer for a lineage no longer live reports nothing, as a late flush does
    /// (DI-D-42); one no request asked for is an inconsistency (DI-D-44, DI-D-46).
    pub(crate) fn provided(
        &mut self,
        lineage: Key,
        seal: u64,
        domain: DomainId,
        outcome: Result<(), FailureCause<V<E, I>>>,
    ) -> Due<E, T, I> {
        let mut due = self.fresh();
        if !self
            .lineages
            .get(&lineage)
            .is_some_and(|state| !state.ended)
        {
            return due;
        }
        let seal_domains = &mut self.seal_domains;
        let answered = self
            .seals
            .get_mut(&(lineage, seal))
            .ok_or("a provider answered for a seal that does not exist")
            .and_then(|candidate| {
                let SealStage::Flushing { failed, .. } = &mut candidate.stage else {
                    return Err("a provider answered for a seal not flushing");
                };
                let part = seal_domains
                    .get_mut(&(lineage, seal, domain))
                    .ok_or("a provider answered a domain the seal did not ask about")?;
                if part.answered {
                    return Err("a provider answered a domain twice");
                }
                part.answered = true;
                part.ok = outcome.is_ok();
                if outcome.is_err() {
                    *failed = true;
                    candidate.answered = true;
                }
                Ok(())
            });
        if let Err(what) = answered {
            self.inconsistent(
                what,
                format_args!("lineage {lineage}, seal {seal}, domain {domain:?}, {outcome:?}"),
            );
            return due;
        }
        let ended = match &outcome {
            Ok(()) => FlushOutcome::Succeeded,
            Err(FailureCause::ProviderAbandoned { .. }) => FlushOutcome::Abandoned,
            Err(_) => FlushOutcome::Failed,
        };
        self.part_answered(lineage, seal, Part::Domain(domain), ended);
        match outcome {
            Ok(()) => self.cover(lineage, seal),
            Err(cause) => {
                self.observe(cause, &Reach::Domain(domain), None, &mut due);
            }
        }
        self.advance(due)
    }

    /// Cover every write named to `lineage`'s seal `seal` whose parts have all answered success:
    /// its file's flush, if the default answers for it, and each of its file's domains the
    /// provider serves. Each is told to the failure observer as covered, once for each failure
    /// suspecting it (DI-D-36, DI-D-113).
    pub(super) fn cover(&mut self, lineage: Key, seal: u64) {
        let Some(found) = self.seals.get(&(lineage, seal)) else {
            return;
        };
        let SealStage::Flushing { .. } = &found.stage else {
            return;
        };
        let (served, spans, lists, seal_domains) =
            (&self.served, &self.spans, &self.lists, &self.seal_domains);
        let covered = |write: &Write<E, T>| {
            write.lineage == lineage
                && write.stage == Stage::Named(seal)
                && (!write.routing.default
                    || spans
                        .get(&(lineage, write.file, Span::Seal(seal)))
                        .is_some_and(|entry| entry.ok))
                && lists
                    .get(write.domains)
                    .iter()
                    .filter(|domain| served.contains(domain))
                    .all(|&domain| {
                        seal_domains
                            .get(&(lineage, seal, domain))
                            .is_some_and(|part| part.ok)
                    })
        };
        // Each marking cites who answered for its write (DI-D-47): the default's flush, observed,
        // and the provider's warranty, exactly as the write's file is routed.
        // Made for as many writes as can be uncovered, so every write covered fits.
        let mut ops = core::mem::take(&mut self.scratch.covered);
        let mut unlisted = None;
        for write in self.writes.values().filter(|write| covered(write)) {
            let kind = MarkingKind::Covered {
                flushed: write.routing.default,
                provider: write.routing.provider,
            };
            if within(&mut ops, (write.op, kind)).is_err() {
                unlisted.get_or_insert(write.op);
            }
        }
        if let Some(op) = unlisted {
            self.inconsistent(
                "a seal covered more writes than the operations declared",
                format_args!("write {op:?}, lineage {lineage}, seal {seal}"),
            );
        }
        // `ops` is in push order, as the writes are kept.
        if !ops.is_empty() {
            self.let_go(|write| {
                ops.binary_search_by_key(&write.op.seq, |(op, _)| op.seq)
                    .is_ok()
            });
        }
        for (op, kind) in ops.drain(..) {
            self.mark(op, kind);
        }
        self.scratch.covered = ops;
    }
}
