// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider, in the core (DI-D-27, DI-D-44): which domains it serves,
//! the parts of a seal it is asked about, its answers, and when a write is covered.
//!
//! A seal asks the built-in default for a flush per file it answers for, and the provider for each
//! domain it serves that the seal's writes touched; [`Durability::cover`] covers a write once every
//! part its file depends on has answered success. A failed or abandoned domain is observed as a
//! failure reaching the files whose declared domains contain it, and every file declared with none
//! -- `Reach::Domains` of that one domain -- which is narrower than a failed flush's reach, the
//! intersection with the failed file's whole set (DI-D-21).

use std::sync::Arc;

use super::{DomainPart, Due, Durability, Key, MarkingKind, Reach, SealStage, Stage, V, Write};
use crate::contract::EpochId;
use crate::dioring::{DomainId, TimeBase};
use crate::ids::OpId;
use crate::types::{Cause, FileKey};

/// Name `op`, a write to `file` declared with `domains`, in the request for each of those domains
/// the provider serves.
pub(super) fn name(
    parts: &mut Vec<DomainPart>,
    served: &[DomainId],
    file: FileKey,
    domains: &[DomainId],
    op: OpId,
) {
    for &domain in domains.iter().filter(|domain| served.contains(domain)) {
        let at = match parts.iter().position(|part| part.domain == domain) {
            Some(at) => at,
            None => {
                parts.push(DomainPart {
                    domain,
                    files: Vec::new(),
                });
                parts.len() - 1
            }
        };
        let files = &mut parts[at].files;
        match files.iter_mut().find(|(key, _)| *key == file) {
            Some((_, writes)) => writes.push(op),
            None => files.push((file, vec![op])),
        }
    }
}

impl<E: EpochId + 'static, T: Clone, K: TimeBase> Durability<E, T, K> {
    /// The domains the consumer's provider serves, sorted, as interned when the instance was built.
    pub(crate) fn serve(&mut self, domains: Arc<[DomainId]>) {
        self.served = domains;
    }

    /// The provider answered `domain` of `lineage`'s seal `seal`: `Ok` for success, or the cause of
    /// its failure -- `Cause::Provider`, or `Cause::ProviderAbandoned` for a completion dropped
    /// unanswered. Success covers what it completes; a failure is observed at once, and is the
    /// seal's answer. An answer for a lineage no longer live reports nothing, as a late flush does
    /// (DI-D-42); one no request asked for is an inconsistency (DI-D-44, DI-D-46).
    pub(crate) fn provided(
        &mut self,
        lineage: Key,
        seal: u64,
        domain: DomainId,
        outcome: Result<(), Cause<V<E>>>,
    ) -> Due<E, T> {
        let mut due = Due::default();
        let Some(state) = self.lineages.get_mut(&lineage).filter(|state| !state.ended) else {
            return due;
        };
        let answered = state
            .seals
            .iter_mut()
            .find(|candidate| candidate.seq == seal)
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
        match outcome {
            Ok(()) => self.cover(lineage, seal, &mut due),
            Err(cause) => {
                let reach = Reach::Domains(Arc::from([domain]));
                self.observe(cause, &reach, None, &mut due);
            }
        }
        self.advance(due)
    }

    /// Cover every write named to `lineage`'s seal `seal` whose parts have all answered success:
    /// its file's flush, if the default answers for it, and each of its file's domains the
    /// provider serves. Each is marked covered in every failure suspecting it (DI-D-36).
    pub(super) fn cover(&mut self, lineage: Key, seal: u64, due: &mut Due<E, T>) {
        let Some(found) = self
            .lineages
            .get(&lineage)
            .and_then(|state| state.seals.iter().find(|candidate| candidate.seq == seal))
        else {
            return;
        };
        let SealStage::Flushing { files, domains, .. } = &found.stage else {
            return;
        };
        let served = &self.served;
        let covered = |write: &Write<E, T>| {
            write.lineage == lineage
                && write.stage == Stage::Named(seal)
                && (!write.routing.default
                    || files.iter().any(|file| file.file == write.file && file.ok))
                && write
                    .domains
                    .iter()
                    .filter(|domain| served.contains(domain))
                    .all(|domain| domains.iter().any(|part| part.domain == *domain && part.ok))
        };
        // Each marking cites who answered for its write (DI-D-47): the default's flush, observed,
        // and the provider's warranty, exactly as the write's file is routed.
        let ops: Vec<(OpId, MarkingKind)> = self
            .writes
            .values()
            .filter(|write| covered(write))
            .map(|write| {
                let kind = MarkingKind::Covered {
                    flushed: write.routing.default,
                    provider: write.routing.provider,
                };
                (write.op, kind)
            })
            .collect();
        if ops.is_empty() {
            return;
        }
        self.writes.retain(|_, write| !covered(write));
        for (op, kind) in ops {
            self.mark(op, kind, due);
        }
    }
}
