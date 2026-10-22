// Copyright (c) 2026 Mike Grier
//! Each pair's ETW activity id (DE-D-6): one created by `EventActivityIdControl` when the pair's begin is written, kept until
//! its end is, and written on both, so a reader groups them by an id the platform guarantees unique across every process of
//! the system until it restarts.
//!
//! The pair id is unique only within its process, and a process id can be reused within a trace, so neither alone nor
//! together do they make an id a later process cannot repeat.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use durable_ioring::CorrelationId;
use win_etw_sys::Guid;

#[cfg(test)]
mod tests;

/// The activity ids of the pairs whose begin was written and whose end has not been, for the whole process.
///
/// Shared by every sink because a pair is one pair however many sinks write it: a sink cloned, or two given to a fan-out,
/// write each begin and end once each, and both copies carry one activity. Its lock is taken only here, around map work and
/// the creation of an id, and nothing is called with it held that reaches dioring, which is the rule `DiagnosticSink` gives a
/// sink's own synchronization.
static HELD: Mutex<Activities> = Mutex::new(Activities::new());

/// The activity of a pair whose begin is being written: the one already held for it, or one `create` makes. `None` if
/// `create` fails, and then the pair has none.
pub(crate) fn begun(pair: CorrelationId, create: impl FnOnce() -> Option<Guid>) -> Option<Guid> {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .begun(pair, create)
}

/// The activity of a pair whose end is being reported, which is let go when its last holder ends; `None` if its begin was
/// not written.
pub(crate) fn ended(pair: CorrelationId) -> Option<Guid> {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ended(pair)
}

/// The pairs' activities, each with how many begins written for it have not yet been ended.
#[derive(Debug, Default)]
pub(crate) struct Activities {
    held: BTreeMap<CorrelationId, (Guid, u32)>,
}

impl Activities {
    pub(crate) const fn new() -> Self {
        Self {
            held: BTreeMap::new(),
        }
    }

    /// As [`begun`], over this map.
    pub(crate) fn begun(
        &mut self,
        pair: CorrelationId,
        create: impl FnOnce() -> Option<Guid>,
    ) -> Option<Guid> {
        if let Some((id, holders)) = self.held.get_mut(&pair) {
            *holders += 1;
            return Some(*id);
        }
        let id = create()?;
        self.held.insert(pair, (id, 1));
        Some(id)
    }

    /// As [`ended`], over this map.
    pub(crate) fn ended(&mut self, pair: CorrelationId) -> Option<Guid> {
        let (id, holders) = self.held.get_mut(&pair)?;
        let id = *id;
        *holders -= 1;
        if *holders == 0 {
            self.held.remove(&pair);
        }
        Some(id)
    }

    /// How many pairs hold an activity.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.held.len()
    }
}
