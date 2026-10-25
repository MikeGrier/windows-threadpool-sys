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

use super::{
    DomainPart, Due, Durability, Key, MarkingKind, Part, Reach, SealStage, Span, Stage, V, Write,
};
use crate::alloc_tags::{tag, tagged};
use crate::capacity::within;
use crate::clock::{Instant, StampClock};
use crate::contract::EpochId;
use crate::diagnostics::FlushOutcome;
use crate::domains::DomainId;
use crate::ids::OpId;
use crate::types::{FailureCause, FlushDomain, IoTargetKey};

/// Name `op`, a write to `file` declared with `domains`, in the request for each of those domains
/// the provider serves.
pub(super) fn name(
    parts: &mut Vec<DomainPart>,
    served: &[DomainId],
    file: IoTargetKey,
    domains: &[DomainId],
    op: OpId,
) {
    for &domain in domains.iter().filter(|domain| served.contains(domain)) {
        let at = match parts.iter().position(|part| part.domain == domain) {
            Some(at) => at,
            None => {
                tagged(tag::PROVIDERS_NAME_PART, || {
                    parts.push(DomainPart {
                        domain,
                        files: Vec::new(),
                    });
                });
                parts.len() - 1
            }
        };
        let files = &mut parts[at].files;
        match files.iter_mut().find(|(key, _)| *key == file) {
            Some((_, writes)) => tagged(tag::PROVIDERS_NAME_WRITE, || writes.push(op)),
            None => {
                let writes = tagged(tag::PROVIDERS_NAME_WRITES, || vec![op]);
                tagged(tag::PROVIDERS_NAME_FILE, || files.push((file, writes)));
            }
        }
    }
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
        let answered = self
            .seals
            .get_mut(&(lineage, seal))
            .ok_or("a provider answered for a seal that does not exist")
            .and_then(|candidate| {
                let SealStage::Flushing {
                    domains, failed, ..
                } = &mut candidate.stage
                else {
                    return Err("a provider answered for a seal not flushing");
                };
                let part = domains
                    .iter_mut()
                    .find(|part| part.domain == domain)
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
            Ok(()) => self.cover(lineage, seal, &mut due),
            Err(cause) => {
                self.observe(cause, &Reach::Domain(domain), None, &mut due);
            }
        }
        self.advance(due)
    }

    /// Cover every write named to `lineage`'s seal `seal` whose parts have all answered success:
    /// its file's flush, if the default answers for it, and each of its file's domains the
    /// provider serves. Each is marked covered in every failure suspecting it (DI-D-36).
    pub(super) fn cover(&mut self, lineage: Key, seal: u64, due: &mut Due<E, T, I>) {
        let Some(found) = self.seals.get(&(lineage, seal)) else {
            return;
        };
        let SealStage::Flushing { domains, .. } = &found.stage else {
            return;
        };
        let (served, spans, lists) = (&self.served, &self.spans, &self.lists);
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
                    .all(|domain| domains.iter().any(|part| part.domain == *domain && part.ok))
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
            self.mark(op, kind, due);
        }
        self.scratch.covered = ops;
    }
}
