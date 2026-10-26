// Copyright (c) 2026 Mike Grier
//! Telling the consumer's failure observer what happens (DI-D-113): given once, before the core is
//! used, and told of each failure and its suspect writes as it is observed. Markings are told
//! where they are made, in `mark`.

use alloc::sync::Arc;

use super::{Durability, V};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::observer::FailureObserver;

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// Whether a failure has been observed: an observer given now would have missed it.
    pub(crate) fn has_observed(&self) -> bool {
        self.next_failure > 0
    }

    /// Give the core its failure observer. `false`, and nothing given, if it has one already.
    pub(crate) fn observed_by(&mut self, observer: Arc<dyn FailureObserver<V<E, I>>>) -> bool {
        if self.observer.is_some() {
            return false;
        }
        self.observer = Some(observer);
        true
    }

    /// Tell the observer of the failure at `at` in the table, just observed: the failure, then
    /// each write it suspects, in push order -- before its `Failed` can be popped.
    pub(super) fn tell_failed(&self, at: usize) {
        let Some(observer) = &self.observer else {
            return;
        };
        let failure = &self.failures[at];
        observer.failed(failure.id, &failure.cause, failure.observed);
        self.visit_suspects(failure, |write| observer.suspected(failure.id, write));
    }
}
