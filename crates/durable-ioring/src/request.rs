// Copyright (c) 2026 Mike Grier
//! A seal's request for the consumer's provider, in storage made at construction, and the request
//! the provider is lent (DI-D-115).
//!
//! The core does not keep a request from when its seal starts. It keeps which seal asked, and the
//! driver drains each one into a [`RequestBuf`], which it made when it built the instance and which
//! the core fills from its own tables: the domains the seal asked about, and the writes named to it.
//! The provider is then lent the request for the length of its call, as a [`FlushRequest`] whose
//! slices point into the buffer, and takes each domain's [`DomainCompletion`] out of it by value:
//! the completions are the only part of a request that outlives the call.
//!
//! The buffer is three lists and a list of requests, each reserved once at the size
//! `Capacity::provider_parts` gives it, and a request that would not fit is an inconsistency, never
//! growth.

use alloc::sync::Weak;
use alloc::vec::Vec;
use core::cell::Cell;
use core::fmt;
use core::iter::FusedIterator;
use core::ops::Range;

use crate::alloc_tags::{tag, tagged};
use crate::capacity::{Capacity, ConstructionError, list, within};
use crate::contract::Identities;
use crate::domains::DomainId;
use crate::ids::Lineage;
use crate::provider::{Answer, DomainCall, DomainCompletion};
use crate::types::{FlushDomain, IoTargetKey};

#[cfg(test)]
mod tests;

/// A request would not fit the buffer made for it: an inconsistency the core reports.
#[derive(Debug)]
pub(crate) struct Overflow;

/// One request's place in the buffer.
struct Request {
    lineage: Lineage,
    seal: u64,
    parts: Range<usize>,
}

/// One domain's part of a request: where its targets are, and whether the provider has taken its
/// completion.
struct Part {
    domain: FlushDomain,
    targets: Range<usize>,
    taken: Cell<bool>,
}

/// One target in a part: where its writes are.
struct Target {
    key: IoTargetKey,
    ids: Range<usize>,
}

/// Where the requests for the consumer's provider are drained to: made by the driver when it builds
/// the instance ([`DurableIoCore::request_buf`](crate::DurableIoCore::request_buf)), filled by the
/// core for each seal that asked ([`drain_requests`](crate::DurableIoCore::drain_requests)), and
/// read as the [`FlushRequest`]s [`requests`](Self::requests) lends. Draining again empties it
/// first, and its storage is kept.
///
/// It is for one thread at a time: its parts record that a provider has taken a completion, in a
/// [`Cell`], so what it lends cannot be sent to another thread.
pub struct RequestBuf<V: Identities> {
    requests: Vec<Request>,
    parts: Vec<Part>,
    targets: Vec<Target>,
    ids: Vec<V::OpId>,
}

impl<V: Identities> RequestBuf<V> {
    /// A buffer for requests to a provider serving `served` domains, with room for every one the
    /// core can have made at once for `capacity`: its storage is reserved now and never grows.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::Table`] if a size is more than a `usize`, or cannot be allocated.
    pub(crate) fn new(capacity: &Capacity, served: usize) -> Result<Self, ConstructionError> {
        let parts = capacity.provider_parts(served);
        Ok(Self {
            requests: tagged(tag::REQUEST_BUF_NEW_REQUESTS, || {
                list("the provider's requests drained", Some(capacity.operations))
            })?,
            parts: tagged(tag::REQUEST_BUF_NEW_PARTS, || {
                list("the domains in the provider's requests", parts)
            })?,
            targets: tagged(tag::REQUEST_BUF_NEW_TARGETS, || {
                list("the targets in the provider's requests", parts)
            })?,
            ids: tagged(tag::REQUEST_BUF_NEW_IDS, || {
                list("the writes in the provider's requests", parts)
            })?,
        })
    }

    /// Whether it holds no request.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// How many requests it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    /// Empty it, keeping its storage. A completion a provider took is its own, and is unaffected.
    pub fn clear(&mut self) {
        self.requests.clear();
        self.parts.clear();
        self.targets.clear();
        self.ids.clear();
    }

    /// Put the request for `lineage`'s seal `seal` in the buffer: a part for each of `asked`, the
    /// domains it asked about in ascending order with the consumer's name for each, and in it a
    /// target for each file `named` gives a write for it, in the order first written, with the
    /// writes in the order `named` gives them. `named` is run again for each domain, and for each
    /// target after it, so nothing is collected.
    ///
    /// # Errors
    ///
    /// [`Overflow`] if it does not fit, with the buffer as it was.
    pub(crate) fn fill<'w, I>(
        &mut self,
        lineage: Lineage,
        seal: u64,
        asked: impl Iterator<Item = (DomainId, FlushDomain)>,
        named: impl Fn() -> I,
    ) -> Result<(), Overflow>
    where
        I: Iterator<Item = (V::OpId, IoTargetKey, &'w [DomainId])>,
    {
        let (parts, targets, ids) = (self.parts.len(), self.targets.len(), self.ids.len());
        let filled = self.fill_parts(lineage, seal, asked, named);
        if filled.is_err() {
            self.parts.truncate(parts);
            self.targets.truncate(targets);
            self.ids.truncate(ids);
        }
        filled
    }

    fn fill_parts<'w, I>(
        &mut self,
        lineage: Lineage,
        seal: u64,
        asked: impl Iterator<Item = (DomainId, FlushDomain)>,
        named: impl Fn() -> I,
    ) -> Result<(), Overflow>
    where
        I: Iterator<Item = (V::OpId, IoTargetKey, &'w [DomainId])>,
    {
        let first_part = self.parts.len();
        for (domain, name) in asked {
            let first_target = self.targets.len();
            within(
                &mut self.parts,
                Part {
                    domain: name,
                    targets: first_target..first_target,
                    taken: Cell::new(false),
                },
            )
            .map_err(|_| Overflow)?;
            // The files with a write for the domain, in the order first written.
            for (_, file, domains) in named() {
                if !domains.contains(&domain) || self.has_target(first_target, file) {
                    continue;
                }
                let at = self.ids.len();
                within(
                    &mut self.targets,
                    Target {
                        key: file,
                        ids: at..at,
                    },
                )
                .map_err(|_| Overflow)?;
            }
            // Each file's writes together, in push order.
            for target in first_target..self.targets.len() {
                let file = self.targets[target].key;
                let start = self.ids.len();
                for (op, written, domains) in named() {
                    if written == file && domains.contains(&domain) {
                        within(&mut self.ids, op).map_err(|_| Overflow)?;
                    }
                }
                self.targets[target].ids = start..self.ids.len();
            }
            if let Some(part) = self.parts.last_mut() {
                part.targets = first_target..self.targets.len();
            }
        }
        within(
            &mut self.requests,
            Request {
                lineage,
                seal,
                parts: first_part..self.parts.len(),
            },
        )
        .map_err(|_| Overflow)
    }

    /// Whether the part whose targets start at `first` has a target for `file`.
    fn has_target(&self, first: usize, file: IoTargetKey) -> bool {
        self.targets[first..]
            .iter()
            .any(|target| target.key == file)
    }

    /// The requests, lent one at a time, in the order drained: each a [`FlushRequest`] whose
    /// completions answer through `home`. A request not taken from the iterator is dropped with it,
    /// which abandons its domains, as a provider that never answered does (DI-D-27): so a panic in
    /// the provider's call leaves no domain unanswered, those of the requests after it included.
    pub fn requests<'a>(&'a self, home: &'a Weak<dyn Answer>) -> Requests<'a, V> {
        Requests {
            buf: self,
            home,
            next: 0,
        }
    }

    /// The buffer's requests as owned lists, for a test to look at.
    #[cfg(test)]
    pub(crate) fn owned(&self) -> Vec<OwnedRequest<V>> {
        self.requests
            .iter()
            .map(|request| OwnedRequest {
                lineage: request.lineage,
                seal: request.seal,
                domains: self.parts[request.parts.clone()]
                    .iter()
                    .map(|part| OwnedPart {
                        domain: part.domain.clone(),
                        targets: self.targets[part.targets.clone()]
                            .iter()
                            .map(|target| OwnedWrites {
                                key: target.key,
                                writes: self.ids[target.ids.clone()].to_vec(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect()
    }
}

impl<V: Identities> fmt::Debug for RequestBuf<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBuf")
            .field("requests", &self.requests.len())
            .field("domains", &self.parts.len())
            .field("targets", &self.targets.len())
            .field("writes", &self.ids.len())
            .finish()
    }
}

/// A request as owned lists, for a test to compare.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct OwnedRequest<V: Identities> {
    pub(crate) lineage: Lineage,
    pub(crate) seal: u64,
    pub(crate) domains: Vec<OwnedPart<V>>,
}

/// One domain's part of an [`OwnedRequest`].
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct OwnedPart<V: Identities> {
    pub(crate) domain: FlushDomain,
    pub(crate) targets: Vec<OwnedWrites<V>>,
}

/// A target's writes in an [`OwnedPart`].
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct OwnedWrites<V: Identities> {
    pub(crate) key: IoTargetKey,
    pub(crate) writes: Vec<V::OpId>,
}

/// The requests of a [`RequestBuf`], lent one at a time.
pub struct Requests<'a, V: Identities> {
    buf: &'a RequestBuf<V>,
    home: &'a Weak<dyn Answer>,
    next: usize,
}

impl<'a, V: Identities> Iterator for Requests<'a, V> {
    type Item = FlushRequest<'a, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.buf.requests.len() {
            return None;
        }
        let request = self.next;
        self.next += 1;
        Some(FlushRequest {
            buf: self.buf,
            request,
            home: self.home,
            narrowed: None,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.buf.requests.len() - self.next;
        (left, Some(left))
    }
}

impl<V: Identities> ExactSizeIterator for Requests<'_, V> {}

impl<V: Identities> FusedIterator for Requests<'_, V> {}

impl<V: Identities> Drop for Requests<'_, V> {
    fn drop(&mut self) {
        // Taking each one and dropping it abandons what it asked.
        for request in self.by_ref() {
            drop(request);
        }
    }
}

impl<V: Identities> fmt::Debug for Requests<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Requests")
            .field("left", &(self.buf.requests.len() - self.next))
            .finish()
    }
}

/// How a view is narrowed to some of a request's domains.
struct Narrowing<'a, V: Identities> {
    keep: &'a dyn Fn(&FlushDomain) -> bool,
    /// The view this one narrows, whose own narrowing holds too.
    within: &'a FlushRequest<'a, V>,
}

/// One seal's request for the consumer's provider, lent for the length of its
/// [`make_durable`](crate::DurabilityProvider::make_durable): its domains, taken one at a time from
/// [`domains`](Self::domains), each with the completion that answers it.
///
/// What is lent is the request's slices, which are the buffer's: they are valid for the call and no
/// longer, so a provider that works after the call returns copies what it needs, and keeps the
/// completions, which are its own. **A domain not taken by the time the request is dropped is
/// abandoned**, as a completion dropped unanswered is (DI-D-27).
pub struct FlushRequest<'a, V: Identities> {
    buf: &'a RequestBuf<V>,
    request: usize,
    home: &'a Weak<dyn Answer>,
    narrowed: Option<Narrowing<'a, V>>,
}

impl<'a, V: Identities> FlushRequest<'a, V> {
    fn request(&self) -> &'a Request {
        &self.buf.requests[self.request]
    }

    fn parts(&self) -> &'a [Part] {
        &self.buf.parts[self.request().parts.clone()]
    }

    /// Whether this view covers `part`, taken or not.
    fn covers(&self, part: &Part) -> bool {
        self.narrowed
            .as_ref()
            .is_none_or(|n| (n.keep)(&part.domain) && n.within.covers(part))
    }

    /// Whether `part` is one of this view's still to be taken.
    fn wanted(&self, part: &Part) -> bool {
        !part.taken.get() && self.covers(part)
    }

    /// How many domains this view still has to take.
    #[must_use]
    pub fn domain_count(&self) -> usize {
        self.parts().iter().filter(|part| self.wanted(part)).count()
    }

    /// The domains this view still has to take, without taking them: for a provider that decides
    /// what to do with the request by what it names.
    pub fn domain_names(&self) -> impl Iterator<Item = &FlushDomain> {
        self.parts()
            .iter()
            .filter(|part| self.wanted(part))
            .map(|part| &part.domain)
    }

    /// The domains this view still has to take, each with its completion, which taking it creates.
    /// Taking is what removes a domain from the view: what the iterator is dropped before taking
    /// stays, to be taken from [`domains`](Self::domains) again or abandoned when the request is
    /// dropped.
    pub fn domains(&self) -> Domains<'_, V> {
        Domains {
            request: self,
            next: 0,
        }
    }

    /// A view of only the domains `keep` names, among those this one still has: a router's way of
    /// handing each provider its own. What the view does not take is abandoned when it is dropped,
    /// and what it takes is gone from this one.
    pub fn only<'b>(&'b self, keep: &'b dyn Fn(&FlushDomain) -> bool) -> FlushRequest<'b, V> {
        FlushRequest {
            buf: self.buf,
            request: self.request,
            home: self.home,
            narrowed: Some(Narrowing { keep, within: self }),
        }
    }

    /// The completion for `part`, which is taken from now.
    fn take(&self, part: &Part) -> DomainCompletion {
        part.taken.set(true);
        let request = self.request();
        DomainCompletion::new(
            Weak::clone(self.home),
            DomainCall {
                lineage: request.lineage,
                seal: request.seal,
                name: part.domain.clone(),
            },
        )
    }
}

impl<V: Identities> Drop for FlushRequest<'_, V> {
    fn drop(&mut self) {
        for part in self.parts() {
            if self.wanted(part) {
                // Dropped unanswered, it is a failure of its domain.
                drop(self.take(part));
            }
        }
    }
}

impl<V: Identities> fmt::Debug for FlushRequest<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlushRequest")
            .field("lineage", &self.request().lineage)
            .field("seal", &self.request().seal)
            .field("domains", &self.domain_count())
            .finish_non_exhaustive()
    }
}

/// The domains of a [`FlushRequest`] still to be taken, each as it is taken.
pub struct Domains<'a, V: Identities> {
    request: &'a FlushRequest<'a, V>,
    next: usize,
}

impl<'a, V: Identities> Iterator for Domains<'a, V> {
    type Item = DomainRequest<'a, V>;

    fn next(&mut self) -> Option<Self::Item> {
        let parts = self.request.parts();
        while self.next < parts.len() {
            let part = &parts[self.next];
            self.next += 1;
            if self.request.wanted(part) {
                return Some(DomainRequest {
                    domain: &part.domain,
                    files: Files {
                        buf: self.request.buf,
                        targets: part.targets.clone(),
                    },
                    completion: self.request.take(part),
                });
            }
        }
        None
    }
}

impl<V: Identities> fmt::Debug for Domains<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Domains").finish_non_exhaustive()
    }
}

/// One flush domain's part of a [`FlushRequest`], and the handle that answers it.
#[derive(Debug)]
pub struct DomainRequest<'a, V: Identities> {
    /// The domain.
    pub domain: &'a FlushDomain,
    /// Each file in the domain with writes named for it, in the order first written.
    pub files: Files<'a, V>,
    /// Answers this domain, once. It is the provider's, to keep and answer from any thread.
    pub completion: DomainCompletion,
}

/// The files of a [`DomainRequest`], each with the writes named for the domain.
pub struct Files<'a, V: Identities> {
    buf: &'a RequestBuf<V>,
    targets: Range<usize>,
}

impl<'a, V: Identities> Iterator for Files<'a, V> {
    type Item = IoTargetWrites<'a, V>;

    fn next(&mut self) -> Option<Self::Item> {
        let at = self.targets.next()?;
        let target = &self.buf.targets[at];
        Some(IoTargetWrites {
            key: target.key,
            writes: &self.buf.ids[target.ids.clone()],
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.targets.size_hint()
    }
}

impl<V: Identities> ExactSizeIterator for Files<'_, V> {}

impl<V: Identities> FusedIterator for Files<'_, V> {}

impl<V: Identities> Clone for Files<'_, V> {
    fn clone(&self) -> Self {
        Self {
            buf: self.buf,
            targets: self.targets.clone(),
        }
    }
}

impl<V: Identities> fmt::Debug for Files<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

/// A target in one domain's part, and the writes to it named for that domain: identities, never
/// extents. Every one has completed. The provider reaches what backs the target by its own means;
/// the request lends it nothing (DI-D-50 P6), and the identities are lent for the call, as the rest
/// of the request is.
#[derive(Debug)]
pub struct IoTargetWrites<'a, V: Identities> {
    /// The consumer's key for the target.
    pub key: IoTargetKey,
    /// The writes named, in push order.
    pub writes: &'a [V::OpId],
}

impl<V: Identities> Clone for IoTargetWrites<'_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V: Identities> Copy for IoTargetWrites<'_, V> {}
