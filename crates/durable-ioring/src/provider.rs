// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider (DI-2.12, DI-D-27).

use std::fmt::{self, Debug};
use std::io;
use std::sync::{Arc, Weak};

use crate::contract::Identities;
use crate::ids::Lineage;
use crate::types::{FailureCause, FlushDomain, IoTargetKey};

/// A consumer-supplied durability provider. It makes writes the instance has seen complete
/// durable, by any means, and answers per flush domain. An instance holds at most one; several
/// are composed by a router that is itself a provider. Every other domain, and every file declared
/// with none, is the built-in default's.
pub trait DurabilityProvider<V: Identities>: Send + Debug {
    /// The flush domains this provider serves, read once when the instance is built.
    fn domains(&self) -> Vec<FlushDomain>;

    /// Make the named writes durable. Returns promptly; the work may finish later, on any thread,
    /// through each domain's completion -- inside this call included. Holds no reference to the
    /// instance.
    ///
    /// dioring calls it from the consumer's own calls on the instance -- normally the `pop` that
    /// follows the completion letting a seal start -- and never with its lock held, so an answer
    /// given inside it is recorded at once (DI-D-44). A panic propagates to that call, and every
    /// completion it drops unanswered is a failure of its domain.
    fn make_durable(&mut self, request: FlushRequest<V>);
}

/// One seal's work for the consumer's provider.
#[derive(Debug)]
pub struct FlushRequest<V: Identities> {
    /// One part per flush domain the provider serves that the seal touched.
    pub domains: Vec<DomainRequest<V>>,
}

/// One flush domain's part of a request, and the handle that answers it.
#[derive(Debug)]
pub struct DomainRequest<V: Identities> {
    /// The domain.
    pub domain: FlushDomain,
    /// Each file in the domain with writes named for it.
    pub files: Vec<IoTargetWrites<V>>,
    /// Answers this domain, once.
    pub completion: DomainCompletion,
}

/// A target in one domain's part, and the writes to it named for that domain: identities, never
/// extents. Every one has completed. The provider reaches what backs the target by its own means;
/// the request lends it nothing (DI-D-50 P6).
#[derive(Debug)]
pub struct IoTargetWrites<V: Identities> {
    /// The consumer's key for the target.
    pub key: IoTargetKey,
    /// The writes named.
    pub writes: Vec<V::OpId>,
}

/// One seal's request for the consumer's provider, as [`DurableIoCore`](crate::DurableIoCore)
/// makes it: the driver hands it to the provider and returns each domain's answer to the core
/// with [`DurableIoCore::provided`](crate::DurableIoCore::provided), naming this lineage and seal.
#[derive(Debug)]
pub struct ProviderRequest<V: Identities> {
    /// The seal's lineage.
    pub lineage: V::Lineage,
    /// The seal's sequence number within its lineage, which each answer names.
    pub seal: u64,
    /// One part per flush domain the provider serves that the seal touched, in the order the
    /// domains were interned.
    pub domains: Vec<ProviderPart<V>>,
}

/// One flush domain's part of a [`ProviderRequest`].
#[derive(Debug)]
pub struct ProviderPart<V: Identities> {
    /// The domain.
    pub domain: FlushDomain,
    /// Each target in the domain with writes named for it, in the order first written.
    pub targets: Vec<IoTargetWrites<V>>,
}

/// Answers one domain of one request, once, from any thread at any time -- inside `make_durable`
/// included. Success warrants that every write named for the domain is durable (the contract).
/// Dropped unanswered, it is a failure of the domain, `FailureCause::ProviderAbandoned`. Answering records
/// the outcome, and sets the readiness signal if that makes an entry poppable; it never runs
/// delivery. Once the instance has ended, an answer does nothing; ending waits for every completion
/// to be answered or dropped.
pub struct DomainCompletion {
    home: Weak<dyn Answer>,
    call: DomainCall,
    answered: bool,
}

impl DomainCompletion {
    /// A completion answering `call` through `home`, which counts it as unanswered from now.
    pub fn new(home: Weak<dyn Answer>, call: DomainCall) -> Self {
        if let Some(home) = home.upgrade() {
            home.issued();
        }
        Self {
            home,
            call,
            answered: false,
        }
    }

    /// Every write named for the domain is durable.
    pub fn succeed(mut self) {
        self.answer(DomainOutcome::Succeeded);
    }

    /// The domain failed: `FailureCause::Provider`, with `error`.
    pub fn fail(mut self, error: io::Error) {
        self.answer(DomainOutcome::Failed(error));
    }

    fn answer(&mut self, outcome: DomainOutcome) {
        self.answered = true;
        if let Some(home) = self.home.upgrade() {
            home.answer(&self.call, outcome);
        }
    }
}

impl Drop for DomainCompletion {
    fn drop(&mut self) {
        if !self.answered {
            self.answer(DomainOutcome::Abandoned);
        }
    }
}

impl Debug for DomainCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainCompletion")
            .field("domain", &self.call.name)
            .finish_non_exhaustive()
    }
}

/// Which part of which seal a completion answers, as the core's `provided` names it.
#[derive(Clone, Debug)]
pub struct DomainCall {
    /// The seal's lineage.
    pub lineage: Lineage,
    /// The seal's sequence number within its lineage.
    pub seal: u64,
    /// The domain.
    pub name: FlushDomain,
}

/// How a provider answered one domain of one request.
#[derive(Debug)]
pub enum DomainOutcome {
    /// Every write named for the domain is durable.
    Succeeded,
    /// The domain failed: `FailureCause::Provider`, with the error.
    Failed(io::Error),
    /// The provider gave no answer and never will: `FailureCause::ProviderAbandoned`.
    Abandoned,
}

impl DomainOutcome {
    /// The outcome as the durability core records it: success, or the cause of `domain`'s failure.
    pub fn into_result<V: Identities>(self, domain: &FlushDomain) -> Result<(), FailureCause<V>> {
        match self {
            DomainOutcome::Succeeded => Ok(()),
            DomainOutcome::Failed(error) => Err(FailureCause::Provider {
                domain: domain.clone(),
                error: Arc::new(error),
            }),
            DomainOutcome::Abandoned => Err(FailureCause::ProviderAbandoned {
                domain: domain.clone(),
            }),
        }
    }
}

/// Where a completion's answer goes: the instance that made the request.
pub trait Answer: Send + Sync {
    /// A completion was made: it is unanswered until [`answer`](Self::answer) is called for it,
    /// which its drop does if nothing else has.
    fn issued(&self);

    /// Record `call`'s answer.
    fn answer(&self, call: &DomainCall, outcome: DomainOutcome);
}
