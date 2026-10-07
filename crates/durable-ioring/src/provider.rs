// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider (DI-2.12, DI-D-27).

use std::fmt::Debug;

use win_shared_os_owned_handle::SharedHandle;

use crate::contract::Identities;
use crate::types::{FileKey, FlushDomain};

/// A consumer-supplied durability provider. It makes writes the instance has seen complete
/// durable, by any means, and answers per flush domain. An instance holds at most one; several
/// are composed by a router that is itself a provider. Every other domain, and every file declared
/// with none, is the built-in default's.
pub trait DurabilityProvider<V: Identities>: Send + Debug {
    /// The flush domains this provider serves, read once when the instance is built.
    fn domains(&self) -> Vec<FlushDomain>;

    /// Make the named writes durable. Returns promptly; the work may finish later, on any thread,
    /// through each domain's completion. Holds no reference to the instance.
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

/// Answers one domain of one request, once, from any thread at any time. Success warrants that
/// every write named for the domain is durable (the contract). Dropped unanswered, it is a
/// failure of the domain, `Cause::ProviderAbandoned`.
#[derive(Debug)]
pub struct DomainCompletion {
    _private: (),
}
