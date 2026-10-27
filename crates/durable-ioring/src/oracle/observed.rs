// Copyright (c) 2026 Mike Grier
//! What a failure observer was told, kept for a harness to read (DI-D-113): each failure's suspect
//! writes and markings, in the order told, the order the failures were told in, and the stamp of
//! each failure and marking in the order told.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::contract::Identities;
use crate::observer::FailureObserver;
use crate::types::{FailureCause, Marking, SuspectWrite};

/// A [`FailureObserver`] that keeps everything it is told, for a test harness or a conformance
/// check to read back: give it to the instance as it is built, and read a failure's suspect writes
/// once its `Failed` is popped, and its markings once the failure's `Abandoned` or `Healed` is: the
/// completion of a marked write is popped before it, and a resolved failure is told no more.
/// [`ConformanceOracle`](super::ConformanceOracle) keeps one, which it reads these from.
pub struct ObservedFailures<V: Identities> {
    told: Mutex<Told<V>>,
}

/// What has been told, by failure.
struct Told<V: Identities> {
    /// Each failure, in the order told.
    order: Vec<V::FailureId>,
    records: HashMap<V::FailureId, Record<V>>,
    /// The stamp of each failure and each marking, with the failure, in the order told.
    stamps: Vec<(V::FailureId, V::Instant)>,
}

/// One failure's record, as told.
struct Record<V: Identities> {
    suspect: Vec<SuspectWrite<V>>,
    markings: Vec<Marking<V>>,
}

impl<V: Identities> ObservedFailures<V> {
    /// Nothing told yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            told: Mutex::new(Told {
                order: Vec::new(),
                records: HashMap::new(),
                stamps: Vec::new(),
            }),
        }
    }

    /// The failures told of, in the order told.
    #[must_use]
    pub fn failures(&self) -> Vec<V::FailureId> {
        self.lock().order.clone()
    }

    /// The writes `failure` was told to suspect, in the order told; `None` if it was never told of.
    #[must_use]
    pub fn suspects(&self, failure: V::FailureId) -> Option<Vec<SuspectWrite<V>>> {
        self.lock()
            .records
            .get(&failure)
            .map(|record| record.suspect.clone())
    }

    /// The markings told for `failure`, in the order told; `None` if it was never told of.
    #[must_use]
    pub fn markings(&self, failure: V::FailureId) -> Option<Vec<Marking<V>>> {
        self.lock()
            .records
            .get(&failure)
            .map(|record| record.markings.clone())
    }

    /// The stamps told, from the `from`th on, each with the failure it was told for: a failure's
    /// own, then those of its markings, in the order told.
    #[must_use]
    pub fn stamps_from(&self, from: usize) -> Vec<(V::FailureId, V::Instant)> {
        self.lock().stamps.iter().skip(from).copied().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Told<V>> {
        self.told.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<V: Identities> Default for ObservedFailures<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Identities> FailureObserver<V> for ObservedFailures<V>
where
    Self: Send + Sync,
{
    fn failed(&self, failure: V::FailureId, _cause: &FailureCause<V>, observed: V::Instant) {
        let mut told = self.lock();
        assert!(
            !told.records.contains_key(&failure),
            "the observer was told of failure {failure:?} twice"
        );
        told.order.push(failure);
        told.stamps.push((failure, observed));
        told.records.insert(
            failure,
            Record {
                suspect: Vec::new(),
                markings: Vec::new(),
            },
        );
    }

    fn suspected(&self, failure: V::FailureId, write: SuspectWrite<V>) {
        let mut told = self.lock();
        let record = told
            .records
            .get_mut(&failure)
            .unwrap_or_else(|| panic!("a suspect write before failure {failure:?} was told of"));
        assert!(
            record.markings.is_empty(),
            "a suspect write of failure {failure:?} after a marking"
        );
        record.suspect.push(write);
    }

    fn marked(&self, failure: V::FailureId, marking: &Marking<V>) {
        let mut told = self.lock();
        let record = told
            .records
            .get_mut(&failure)
            .unwrap_or_else(|| panic!("a marking for failure {failure:?}, never told of"));
        record.markings.push(marking.clone());
        told.stamps.push((failure, marking.observed));
    }
}
