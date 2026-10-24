// Copyright (c) 2026 Mike Grier
//! The core's failures (DI-D-12, DI-D-34): resolving them, importing one learned of outside the
//! instance, and the inventory.

use super::{CoreActions, DurableIoCore, V};
use crate::capacity::{AtCapacity, Limit};
use crate::clock::StampClock;
use crate::contract::EpochId;
use crate::domains::import_reach;
use crate::ids::{FailureId, FailureToken};
use crate::types::{
    FailureCause, FailureInfo, ImportError, ImportScope, Imported, Resolution, ResolveError,
};

/// What an import recorded and made due, or why it was refused.
type ImportAnswer<E, K> = Result<(Imported<V<E, K>>, CoreActions), ImportError<V<E, K>>>;

impl<E: EpochId + 'static, B, C, K: StampClock> DurableIoCore<E, B, C, K> {
    /// Resolve failures, validated whole and applied together: a refused call changes nothing and
    /// hands every token back.
    ///
    /// # Errors
    ///
    /// [`ResolveError`], naming why and handing back the tokens.
    pub fn resolve(
        &mut self,
        items: Vec<(FailureToken, Resolution)>,
    ) -> Result<CoreActions, ResolveError<V<E, K>>> {
        let was_empty = self.queue.is_empty();
        let due = self.durability.resolve(items)?;
        Ok(self.act(due, was_empty))
    }

    /// Record a failure the consumer learned of outside the instance (DI-D-12 (h)), reaching the
    /// uncovered writes `scope` names.
    ///
    /// # Errors
    ///
    /// [`ImportError::UnknownLineage`] for an [`ImportScope::Lineage`] that is not this core's live
    /// one, and [`ImportError::AtCapacity`] when the core holds as many unresolved imported
    /// failures as it was built for (DI-D-107); nothing is recorded.
    pub fn import_failure(&mut self, scope: ImportScope<V<E, K>>) -> ImportAnswer<E, K> {
        let (reach, only, unmatched) =
            import_reach(&scope, &self.domains, |lineage| self.is_live(lineage))?;
        if self.durability.imports() >= self.capacity.imports {
            return Err(ImportError::AtCapacity(AtCapacity {
                limit: Limit::Imports,
                declared: self.capacity.imports,
            }));
        }
        let was_empty = self.queue.is_empty();
        let (failure, due) = self
            .durability
            .import(FailureCause::Imported { scope }, &reach, only);
        Ok((Imported { failure, unmatched }, self.act(due, was_empty)))
    }

    /// The unresolved failures, in observation order.
    #[must_use]
    pub fn failures(&self) -> Vec<FailureInfo<V<E, K>>> {
        self.durability.failures()
    }

    /// A new token for `failure`, if it is unresolved and its token was dropped.
    pub fn take_token(&mut self, failure: FailureId) -> Option<FailureToken> {
        self.durability.take_token(failure)
    }
}
