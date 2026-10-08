// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider (DI-2.12, DI-D-27).

use std::fmt::{self, Debug};
use std::io;
use std::sync::Weak;

use win_shared_os_owned_handle::SharedHandle;

use crate::contract::Identities;
use crate::dioring::DomainId;
use crate::types::{FileKey, FlushDomain};

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
    pub files: Vec<FileWrites<V>>,
    /// Answers this domain, once.
    pub completion: DomainCompletion,
}

/// A file in one domain's part, and the writes to it named for that domain: identities, never
/// extents. Every one has completed.
#[derive(Debug)]
pub struct FileWrites<V: Identities> {
    /// The consumer's key for the file.
    pub key: FileKey,
    /// The file, lent so the provider can flush it out of band.
    pub file: SharedHandle,
    /// The writes named.
    pub writes: Vec<V::OpId>,
}

/// Answers one domain of one request, once, from any thread at any time -- inside `make_durable`
/// included. Success warrants that every write named for the domain is durable (the contract).
/// Dropped unanswered, it is a failure of the domain, `Cause::ProviderAbandoned`. Answering records
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
    pub(crate) fn new(home: Weak<dyn Answer>, call: DomainCall) -> Self {
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

    /// The domain failed: `Cause::Provider`, with `error`.
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

/// Which part of which seal a completion answers.
#[derive(Clone, Debug)]
pub(crate) struct DomainCall {
    /// The seal's lineage, by its key within the instance.
    pub(crate) lineage: u64,
    /// The seal's sequence number.
    pub(crate) seal: u64,
    /// The domain, as the instance interned it.
    pub(crate) domain: DomainId,
    /// The domain's bytes, for the cause of its failure.
    pub(crate) name: FlushDomain,
}

/// How a domain was answered.
pub(crate) enum DomainOutcome {
    Succeeded,
    Failed(io::Error),
    /// Dropped unanswered.
    Abandoned,
}

/// Where a completion's answer goes: the instance that made the request.
pub(crate) trait Answer: Send + Sync {
    /// A completion was made: it is unanswered until [`answer`](Self::answer) is called for it,
    /// which its drop does if nothing else has.
    fn issued(&self);

    /// Record `call`'s answer.
    fn answer(&self, call: &DomainCall, outcome: DomainOutcome);
}
