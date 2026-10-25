// Copyright (c) 2026 Mike Grier
//! The core's flush-domain lists (DI-D-108): the table the I/O core claims its targets' lists in
//! and lets them go from, a failure reaching by an import's list letting it go when it is
//! resolved, and the census that nothing names a list whose row is free.

use super::{Durability, Reach};
use crate::clock::Instant;
use crate::contract::EpochId;
#[cfg(test)]
use crate::domain_lists::DomainList;
use crate::domain_lists::DomainLists;
#[cfg(test)]
use crate::domains::DomainId;

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// The flush-domain lists.
    pub(crate) fn lists(&self) -> &DomainLists {
        &self.lists
    }

    /// The flush-domain lists, for the I/O core to claim a target's in and let it go from.
    pub(crate) fn lists_mut(&mut self) -> &mut DomainLists {
        &mut self.lists
    }

    /// A failure reaching by `reach` is resolved, or was never recorded: an import's list is its
    /// own, and is let go; a file's is the file's.
    pub(super) fn release_reach(&mut self, reach: Reach) {
        if let Reach::Named(list) = reach {
            self.lists.release(list);
        }
    }

    /// A list naming `ids`, one per distinct set: for tests that push writes to files no target
    /// was declared for.
    #[cfg(test)]
    pub(crate) fn test_domains(&mut self, ids: &[DomainId]) -> DomainList {
        self.lists.shared(&mut self.test_lists, ids)
    }

    /// Whether every list a write, a span, a kept record or a failure's reach names is held.
    #[cfg(test)]
    pub(super) fn lists_agree(&self) -> bool {
        let reached = self
            .failures
            .iter()
            .filter_map(|failure| match failure.reach {
                Reach::File(list) | Reach::Named(list) => Some(list),
                Reach::All | Reach::Domain(_) => None,
            });
        self.writes
            .values()
            .map(|write| write.domains)
            .chain(self.spans.values().map(|entry| entry.domains))
            .chain(self.kept.writes().map(|write| write.domains))
            .chain(reached)
            .all(|list| self.lists.is_held(list))
    }
}
