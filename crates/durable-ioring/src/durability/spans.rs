// Copyright (c) 2026 Mike Grier
//! What a seal's work on a file is charged to (DI-D-98, DI-D-107): one entry per (lineage, file,
//! seal span) with an uncovered write in it, made by the first write to the file in the span and
//! released with the last to leave it.
//!
//! A write joins its lineage's unsealed span when it is pushed. A seal that names it moves it to
//! the seal's own span, so a seal divides an unsealed span when it names some of a file's writes
//! and not the rest. Either way an entry holds at least one write, so there are never more
//! entries than uncovered writes, which the window bounds (DI-D-107); a seal's flush of a file,
//! the commit in flight for it, and its flush pair each belong to one entry.

use alloc::sync::Arc;
use std::collections::HashMap;

use super::{Durability, Key, Stage, Write};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::domains::DomainId;
use crate::types::IoTargetKey;

/// Which span of its lineage a write is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Span {
    /// Not yet named to a seal.
    Unsealed,
    /// Named to the lineage's seal with this sequence number.
    Seal(u64),
}

impl Span {
    /// The span a write at `stage` is in.
    pub(super) fn of(stage: Stage) -> Self {
        match stage {
            Stage::InFlight | Stage::Completed => Self::Unsealed,
            Stage::Named(seq) => Self::Seal(seq),
        }
    }
}

/// A span entry's key: the lineage, the file, and the span.
pub(super) type SpanKey = (Key, IoTargetKey, Span);

/// Every span entry, by key.
pub(super) type Spans = HashMap<SpanKey, SpanFile>;

/// One (lineage, file, seal span) entry.
pub(super) struct SpanFile {
    /// Its uncovered writes; the entry is released with the last.
    pub(super) writes: usize,
    /// The file's flush domains, which a failed flush of it reaches.
    pub(super) domains: Arc<[DomainId]>,
    /// The seal's flush of the file was issued: the built-in default answers for it.
    pub(super) flushed: bool,
    /// That flush answered.
    pub(super) answered: bool,
    /// That flush answered with success.
    pub(super) ok: bool,
}

impl SpanFile {
    /// An entry with no writes yet, for a file declared with `domains`.
    pub(super) fn new(domains: Arc<[DomainId]>) -> Self {
        Self {
            writes: 0,
            domains,
            flushed: false,
            answered: false,
            ok: false,
        }
    }
}

/// Take one write out of the entry under `key`, releasing the entry with its last write: `false`
/// if there is no such entry.
pub(super) fn leave(spans: &mut Spans, key: SpanKey) -> bool {
    let Some(entry) = spans.get_mut(&key) else {
        return false;
    };
    entry.writes -= 1;
    if entry.writes == 0 {
        spans.remove(&key);
    }
    true
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// Let go every uncovered write `leaves` picks, each out of its span's entry.
    pub(super) fn let_go(&mut self, mut leaves: impl FnMut(&Write<E, T>) -> bool) {
        let mut lost = None;
        for (_, write) in self.writes.extract_if(.., |_, write| leaves(write)) {
            if !leave(
                &mut self.spans,
                (write.lineage, write.file, Span::of(write.stage)),
            ) {
                lost = Some(write.op);
            }
        }
        if let Some(op) = lost {
            self.inconsistent(
                "an uncovered write was in no span",
                format_args!("write {op:?}"),
            );
        }
    }

    /// Let go the uncovered write pushed `seq`th, out of its span's entry.
    pub(super) fn let_go_one(&mut self, seq: u64) {
        let Some(write) = self.writes.remove(&seq) else {
            return;
        };
        if !leave(
            &mut self.spans,
            (write.lineage, write.file, Span::of(write.stage)),
        ) {
            self.inconsistent(
                "an uncovered write was in no span",
                format_args!("write {:?}", write.op),
            );
        }
    }

    /// Whether the span entries are exactly the uncovered writes' spans, each counting its
    /// writes: no entry without a write, and no write without its entry.
    #[cfg(test)]
    pub(super) fn spans_agree(&self) -> bool {
        let mut counts: HashMap<SpanKey, usize> = HashMap::new();
        for write in self.writes.values() {
            *counts
                .entry((write.lineage, write.file, Span::of(write.stage)))
                .or_insert(0) += 1;
        }
        counts.len() == self.spans.len()
            && counts
                .iter()
                .all(|(key, &n)| self.spans.get(key).is_some_and(|entry| entry.writes == n))
    }

    /// How many span entries there are.
    #[cfg(test)]
    pub(super) fn span_entries(&self) -> usize {
        self.spans.len()
    }
}
