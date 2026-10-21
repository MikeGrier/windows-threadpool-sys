// Copyright (c) 2026 Mike Grier
//! The routing provider (DI-2.12 point 1, DI-D-81): one provider composed of others, so an
//! instance, which holds at most one consumer provider, can have several.
//!
//! Each provider registered with the router serves exactly the flush domains its own
//! [`domains`](DurabilityProvider::domains) returns, read once when the router is built. A domain
//! claimed twice -- by two registrations, or twice by one -- refuses the build.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

use crate::contract::Identities;
use crate::provider::DurabilityProvider;
use crate::types::FlushDomain;

/// Builds a [`RoutingProvider`] from providers registered with it in turn. Registrations are
/// numbered from 0 in the order made, and [`RoutedTwice`] names them by that number.
pub struct RouterBuilder<V: Identities> {
    providers: Vec<Box<dyn DurabilityProvider<V>>>,
}

impl<V: Identities> RouterBuilder<V> {
    /// A builder with no provider registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register `provider`, to serve the domains it declares. Its domains are read by
    /// [`build`](Self::build), not here.
    #[must_use]
    pub fn register(mut self, provider: impl DurabilityProvider<V> + 'static) -> Self {
        self.providers.push(Box::new(provider));
        self
    }

    /// The router: each registered provider's domains read once, in registration order.
    ///
    /// # Errors
    ///
    /// [`RoutedTwice`] for the first domain met a second time, reading the registrations in order
    /// and each one's domains in the order it gave them. The providers already read are dropped
    /// with the builder, and those after it are not asked for their domains.
    pub fn build(self) -> Result<RoutingProvider<V>, RoutedTwice> {
        let mut owners: HashMap<FlushDomain, usize> = HashMap::new();
        let mut routes = Vec::with_capacity(self.providers.len());
        for (registration, provider) in self.providers.into_iter().enumerate() {
            let domains = provider.domains();
            for domain in &domains {
                match owners.entry(domain.clone()) {
                    Entry::Occupied(owner) => {
                        return Err(RoutedTwice {
                            domain: domain.clone(),
                            first: *owner.get(),
                            second: registration,
                        });
                    }
                    Entry::Vacant(vacant) => {
                        vacant.insert(registration);
                    }
                }
            }
            routes.push(Route { provider, domains });
        }
        Ok(RoutingProvider { routes })
    }
}

impl<V: Identities> Default for RouterBuilder<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Identities> fmt::Debug for RouterBuilder<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouterBuilder")
            .field("providers", &self.providers)
            .finish()
    }
}

/// A provider that serves the domains of the providers registered with it, each its own, and
/// itself no other. Built by [`RouterBuilder`].
pub struct RoutingProvider<V: Identities> {
    routes: Vec<Route<V>>,
}

/// One registration: the provider and the domains it declared when the router was built.
struct Route<V: Identities> {
    provider: Box<dyn DurabilityProvider<V>>,
    domains: Vec<FlushDomain>,
}

impl<V: Identities> RoutingProvider<V> {
    /// A builder with no provider registered.
    #[must_use]
    pub fn builder() -> RouterBuilder<V> {
        RouterBuilder::new()
    }

    /// Every domain a registered provider serves: the registrations in order, and each one's
    /// domains in the order it gave them when the router was built. No domain appears twice, which
    /// the build refused.
    #[must_use]
    pub fn domains(&self) -> Vec<FlushDomain> {
        self.routes
            .iter()
            .flat_map(|route| route.domains.iter().cloned())
            .collect()
    }
}

impl<V: Identities> fmt::Debug for RoutingProvider<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoutingProvider")
            .field("routes", &self.routes)
            .finish()
    }
}

impl<V: Identities> fmt::Debug for Route<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route")
            .field("provider", &self.provider)
            .field("domains", &self.domains)
            .finish()
    }
}

/// A router's build met a flush domain claimed twice: the domain, and the registrations that
/// claimed it, numbered from 0 in registration order. `first` and `second` are equal when one
/// provider declared the domain twice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedTwice {
    /// The domain.
    pub domain: FlushDomain,
    /// The registration that claimed it first.
    pub first: usize,
    /// The registration that claimed it again.
    pub second: usize,
}

impl fmt::Display for RoutedTwice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.first == self.second {
            write!(
                f,
                "registration {} declared the flush domain {:?} twice",
                self.first, self.domain
            )
        } else {
            write!(
                f,
                "the flush domain {:?} is claimed by registrations {} and {}",
                self.domain, self.first, self.second
            )
        }
    }
}

impl std::error::Error for RoutedTwice {}

#[cfg(test)]
mod tests;
