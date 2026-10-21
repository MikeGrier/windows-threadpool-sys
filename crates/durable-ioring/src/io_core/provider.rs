// Copyright (c) 2026 Mike Grier
//! The consumer's provider, as the core sees it (DI-D-27, DI-D-44, DI-D-50 P6): the domains it
//! serves, the requests a seal makes of it, and its answers.
//!
//! The core holds no provider. It routes each target's writes by the domains the provider serves,
//! emits a [`ProviderRequest`] per seal that touched one, and takes each domain's answer through
//! [`DurableIoCore::provided`]. Calling the provider, and doing so with no lock held so that it may
//! answer inside its call, is the driver's.

use std::fmt;
use std::sync::Arc;

use super::{Actions, DurableIoCore, V};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::diagnostics::DiagnosticSink;
use crate::domains::{first_repeat, intern_all};
use crate::durability::{Due, ProviderCall};
use crate::ids::Lineage;
use crate::provider::{DomainOutcome, IoTargetWrites, ProviderPart, ProviderRequest};
use crate::types::FlushDomain;

/// A provider named a domain twice: the domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateDomain(pub FlushDomain);

impl fmt::Display for DuplicateDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the provider named the domain {:?} twice", self.0)
    }
}

impl std::error::Error for DuplicateDomain {}

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// A core whose consumer's provider serves `domains`, read once, as [`new`](Self::new) builds
    /// one otherwise. A target declaring a domain it serves has that domain's part of each seal
    /// answered by the provider; every other domain, and every target declared with none, is the
    /// built-in default's commit.
    ///
    /// # Errors
    ///
    /// [`DuplicateDomain`] for a domain named twice.
    pub fn with_provider(
        clock: K,
        diagnostics: Arc<dyn DiagnosticSink>,
        domains: Vec<FlushDomain>,
    ) -> Result<Self, DuplicateDomain> {
        if let Some(repeat) = first_repeat(domains.iter().cloned()) {
            return Err(DuplicateDomain(repeat));
        }
        let mut core = Self::new(clock, diagnostics);
        let served = intern_all(&mut core.domains, domains.iter().cloned());
        let mut named: Vec<_> = domains
            .into_iter()
            .map(|domain| (core.domains[&domain], domain))
            .collect();
        named.sort_unstable_by_key(|(id, _)| *id);
        core.served_names = named.into_iter().map(|(_, domain)| domain).collect();
        core.durability
            .serve(Arc::clone(&served), core.served_names.clone());
        core.served = served;
        Ok(core)
    }

    /// The provider answered `domain` of `lineage`'s seal `seal`, as a [`ProviderRequest`] named
    /// them. An answer for a lineage since ended reports nothing, as a late commit does (DI-D-42).
    ///
    /// An answer naming another core's lineage, a domain the provider does not serve, a seal or
    /// domain no request asked about, or a domain answered twice is the driver or the provider
    /// breaking the protocol, and is panicked on (DI-D-44, DI-D-46).
    pub fn provided(
        &mut self,
        lineage: Lineage,
        seal: u64,
        domain: &FlushDomain,
        outcome: DomainOutcome,
    ) -> Actions<E, B, K> {
        let was_empty = self.queue.is_empty();
        let id = self
            .domains
            .get(domain)
            .copied()
            .filter(|id| self.served.binary_search(id).is_ok());
        let (Some(id), true) = (id, lineage.instance == self.instance) else {
            self.durability.inconsistent(
                "a provider answered for a request this core never made",
                format_args!("lineage {lineage:?}, seal {seal}, domain {domain:?}"),
            );
            return self.act(Due::default(), was_empty);
        };
        let due = self
            .durability
            .provided(lineage.seq, seal, id, outcome.into_result(domain));
        self.act(due, was_empty)
    }

    /// A request in the core's terms, as the provider is handed it.
    pub(super) fn request(&self, call: ProviderCall) -> ProviderRequest<V<E, K>> {
        let domains = call
            .domains
            .into_iter()
            .map(|part| {
                let at = self
                    .served
                    .binary_search(&part.domain)
                    .expect("a request names only domains the provider serves");
                ProviderPart {
                    domain: self.served_names[at].clone(),
                    targets: part
                        .files
                        .into_iter()
                        .map(|(key, writes)| IoTargetWrites { key, writes })
                        .collect(),
                }
            })
            .collect();
        ProviderRequest {
            lineage: self.name(call.lineage),
            seal: call.seal,
            domains,
        }
    }
}
