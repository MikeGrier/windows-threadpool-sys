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

use fixed_hash_table::HeapMap;

use super::kept::{Gone, Kept};
use super::window::{Holder, Window};
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

/// Every span entry, by key. Never more than the operations declared, since each holds a write
/// and each write holds a place (DI-D-107).
pub(super) type Spans = HeapMap<SpanKey, SpanFile>;

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

/// `write` is no longer uncovered: out of its span's entry, its hold on its place released, and kept
/// in `kept` if an unresolved failure suspects it (DI-D-108), nameable until `now` on the failure
/// clock unless its lineage's end stopped it earlier. What about that was inconsistent, if
/// anything.
fn forget<E: EpochId + 'static, T, I: Instant>(
    spans: &mut Spans,
    window: &mut Window,
    kept: &mut Kept<E, I>,
    write: &Write<E, T>,
    now: u64,
) -> Option<&'static str> {
    let mut lost = None;
    if !leave(spans, (write.lineage, write.file, Span::of(write.stage))) {
        lost = Some("an uncovered write was in no span");
    }
    if let Err(what) = window.release(write.op.seq, Holder::Uncovered) {
        lost = lost.or(Some(what));
    }
    if window.suspected(write.op.seq)
        && let Err(what) = kept.keep(Gone {
            op: write.op,
            lineage: write.lineage,
            epoch: write.epoch,
            file: write.file,
            domains: Arc::clone(&write.domains),
            born: write.born,
            until: write.until.unwrap_or(now),
        })
    {
        lost = lost.or(Some(what));
    }
    lost
}

impl<E: EpochId + 'static, T, I: Instant, K> Durability<E, T, I, K> {
    /// Let go every uncovered write `leaves` picks, each out of its span's entry.
    pub(super) fn let_go(&mut self, mut leaves: impl FnMut(&Write<E, T>) -> bool) {
        let mut lost = None;
        let now = self.next_failure;
        let (spans, window, kept) = (&mut self.spans, &mut self.window, &mut self.kept);
        self.writes.take_where(
            |_, write| leaves(write),
            |_, write| {
                if let Some(what) = forget(spans, window, kept, &write, now) {
                    lost.get_or_insert((what, write.op));
                }
            },
        );
        if let Some((what, op)) = lost {
            self.inconsistent(what, format_args!("write {op:?}"));
        }
    }

    /// Let go every uncovered write of `lineage`, which has ended, except those named to a seal whose
    /// flush of their file is out: its commit is in flight, and its answer lets them go (DI-D-98).
    pub(super) fn let_go_ended(&mut self, lineage: Key) {
        let now = self.next_failure;
        let (spans, window, kept) = (&mut self.spans, &mut self.window, &mut self.kept);
        let mut lost = None;
        self.writes.retain(|_, write| {
            if write.lineage != lineage || write.stage == Stage::InFlight {
                return true;
            }
            if let Stage::Named(seq) = write.stage
                && spans
                    .get(&(lineage, write.file, Span::Seal(seq)))
                    .is_some_and(|entry| entry.flushed && !entry.answered)
            {
                return true;
            }
            if let Some(what) = forget(spans, window, kept, write, now) {
                lost.get_or_insert((what, write.op));
            }
            false
        });
        if let Some((what, op)) = lost {
            self.inconsistent(what, format_args!("write {op:?}"));
        }
    }

    /// Let go the uncovered write pushed `seq`th, out of its span's entry.
    pub(super) fn let_go_one(&mut self, seq: u64) {
        let Some(write) = self.writes.remove(&seq) else {
            return;
        };
        let now = self.next_failure;
        if let Some(what) = forget(
            &mut self.spans,
            &mut self.window,
            &mut self.kept,
            &write,
            now,
        ) {
            self.inconsistent(what, format_args!("write {:?}", write.op));
        }
    }

    /// Whether the span entries are exactly the uncovered writes' spans, each counting its
    /// writes: no entry without a write, and no write without its entry.
    #[cfg(test)]
    pub(super) fn spans_agree(&self) -> bool {
        let mut counts: std::collections::HashMap<SpanKey, usize> =
            std::collections::HashMap::new();
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
