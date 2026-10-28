// Copyright (c) 2026 Mike Grier
//! The consumer's provider, as the core sees it (DI-D-27, DI-D-44, DI-D-50 P6): the domains it
//! serves, the requests a seal makes of it, and its answers.
//!
//! The core holds no provider. It routes each target's writes by the domains the provider serves,
//! queues a request per seal that touched one, drained into a [`RequestBuf`], and takes each domain's answer through
//! [`DurableIoCore::provided`]. Calling the provider, and doing so with no lock held so that it may
//! answer inside its call, is the driver's.

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{CoreActions, DurableIoCore, V};
use crate::alloc_tags::{tag, tagged};
use crate::capacity::{Capacity, ConstructionError};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::diagnostics::DiagnosticSink;
use crate::domains::{first_repeat, intern_all};
use crate::durability::Due;
use crate::ids::Lineage;
use crate::provider::DomainOutcome;
use crate::request::RequestBuf;
use crate::types::FlushDomain;

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// A core whose consumer's provider serves `domains`, read once, as [`new`](Self::new) builds
    /// one otherwise. A target declaring a domain it serves has that domain's part of each seal
    /// answered by the provider; every other domain, and every target declared with none, is the
    /// built-in default's commit.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::NoLineages`] if `capacity` declares no lineage;
    /// [`ConstructionError::DuplicateDomain`] for a domain named twice; and
    /// [`ConstructionError::TooManyDomains`] if the provider serves more domains than `capacity`
    /// declares (DI-D-107).
    pub fn with_provider(
        clock: K,
        diagnostics: Arc<dyn DiagnosticSink>,
        capacity: Capacity,
        domains: Vec<FlushDomain>,
    ) -> Result<Self, ConstructionError> {
        if capacity.lineages == 0 {
            return Err(ConstructionError::NoLineages);
        }
        if let Some(repeat) = first_repeat(domains.iter().cloned()) {
            return Err(ConstructionError::DuplicateDomain(repeat));
        }
        if domains.len() > capacity.domains {
            return Err(ConstructionError::TooManyDomains {
                served: domains.len(),
                declared: capacity.domains,
            });
        }
        let mut core = Self::build(clock, diagnostics, capacity, domains.len())?;
        // Checked above to fit the table made for them.
        let served = intern_all(&mut core.domains, domains.iter().cloned()).map_err(|_| {
            ConstructionError::TooManyDomains {
                served: domains.len(),
                declared: capacity.domains,
            }
        })?;
        let mut named: Vec<_> = tagged(tag::IO_CORE_WITH_PROVIDER_NAMED, || {
            domains
                .into_iter()
                .map(|domain| (core.domains[&domain], domain))
                .collect()
        });
        named.sort_unstable_by_key(|(id, _)| *id);
        core.served_names = tagged(tag::IO_CORE_WITH_PROVIDER_SERVED_NAMES, || {
            named.into_iter().map(|(_, domain)| domain).collect()
        });
        let names = tagged(tag::IO_CORE_WITH_PROVIDER_NAMES, || {
            core.served_names.clone()
        });
        core.durability.serve(Arc::clone(&served), names);
        core.served = served;
        Ok(core)
    }

    /// The provider answered `domain` of `lineage`'s seal `seal`, as the request named them. An answer for a lineage since ended reports nothing, as a late commit does (DI-D-42).
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
    ) -> CoreActions {
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

    /// A buffer to drain the provider's requests into, with room for every one this core can have
    /// made at once ([`Capacity`]'s `operations` and the domains the provider serves): made by the
    /// driver when it builds the instance, since it is storage and storage is made then
    /// (DI-D-115). One for a core with no provider is empty.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::Table`] if a size is more than a `usize`, or cannot be allocated.
    pub fn request_buf(&self) -> Result<RequestBuf<V<E, K>>, ConstructionError> {
        RequestBuf::new(&self.capacity, self.served.len())
    }

    /// Drain every request the provider has to be made, in the order the seals started, into
    /// `buf`, emptying it first and the core's queue as it goes (DI-D-115). A request is read from
    /// the core as it stands now: the domains its seal asked about, and the writes named to it,
    /// which a lineage that has ended since may have let go.
    pub fn drain_requests(&mut self, buf: &mut RequestBuf<V<E, K>>) {
        buf.clear();
        while let Some(call) = self.requests.pop_front() {
            let lineage = self.name(call.lineage);
            let (durability, served, names) = (&self.durability, &self.served, &self.served_names);
            let asked = durability
                .asked(call.lineage, call.seal)
                .filter_map(|domain| {
                    let at = served.binary_search(&domain).ok()?;
                    Some((domain, names.get(at)?.clone()))
                });
            let named = || durability.named_for_provider(call.lineage, call.seal);
            if buf.fill(lineage, call.seal, asked, named).is_err() {
                self.durability.inconsistent(
                    "the provider's requests did not fit the buffer made for them",
                    format_args!("lineage {lineage:?}, seal {}", call.seal),
                );
            }
        }
    }
}
