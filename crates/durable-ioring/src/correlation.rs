// Copyright (c) 2026 Mike Grier
//! Correlation ids (DI-D-80): what pairs a begin event with its end, so a sink's analysis computes
//! the time between them and nothing in dioring measures it.
//!
//! An id is a `u64` from one counter for the whole process, so it is unique among the ids the
//! process has minted and says nothing about any other process's. It is minted only for a pair a
//! sink wants: [`CorrelationId::for_pair`] asks the sink first, so a sink that wants nothing costs
//! the counter nothing.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::alloc_tags::{tag, tagged};
use crate::diagnostics::{DiagnosticKind, DiagnosticSink};

#[cfg(test)]
mod tests;

/// The process's counter. Relaxed is enough: an id must differ from every other, which the
/// counter's single modification order gives, and orders nothing else (DI-D-80).
static NEXT: AtomicU64 = AtomicU64::new(FIRST);

/// The first id a counter mints.
const FIRST: u64 = 1;

/// What pairs a begin event with its end: unique among the ids this process has minted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CorrelationId(u64);

impl CorrelationId {
    /// A new id for a pair of events of `kind`, if `sink` wants that kind, from the process's
    /// counter; `None` if it does not, and then nothing is minted. Asked once, when the begin is
    /// built, so the begin and its end carry the same id.
    ///
    /// The counter starts at 1 and wraps on overflow, as `fetch_add` does.
    #[must_use]
    pub fn for_pair(sink: &dyn DiagnosticSink, kind: DiagnosticKind) -> Option<Self> {
        mint_for(&NEXT, sink, kind)
    }

    /// The id's number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
    /// An id with the number a test chose, which no counter minted.
    #[cfg(test)]
    pub(crate) const fn chosen(n: u64) -> Self {
        Self(n)
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// [`CorrelationId::for_pair`], from `counter`: the process's, or a test's own.
fn mint_for(
    counter: &AtomicU64,
    sink: &dyn DiagnosticSink,
    kind: DiagnosticKind,
) -> Option<CorrelationId> {
    tagged(tag::CORRELATION_MINT_FOR_WANTS, || sink.wants(kind))
        .then(|| CorrelationId(counter.fetch_add(1, Ordering::Relaxed)))
}
