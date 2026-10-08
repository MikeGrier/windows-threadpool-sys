// Copyright (c) 2026 Mike Grier
//! One lineage's durability state, without I/O: which writes are in flight, which have completed
//! and await a seal, the seals in progress, and the high-water mark (DI-D-9, DI-D-22).
//!
//! Every change returns what it makes due -- flushes to push, and epochs newly durable -- and the
//! caller does the I/O and appends the entries. So this module decides and the relay acts, as
//! DI-D-18's I/O-free core intends, and these rules are tested without a ring.
//!
//! **How a seal proceeds.** `seal(n)` waits until no write of the lineage at or below `n` is in
//! flight, then names every completed write at or below `n` not yet named, by file, and asks for
//! one flush per file. Each write belongs to exactly one seal -- the first at or above its epoch --
//! because writes at or below a seal are refused once it is made. The seal is done when every one
//! of its flushes succeeds, and the high-water mark advances over done seals in order, so a seal
//! done before an earlier one waits for it (the prefix property).
//!
//! **Not yet here**, and left visibly undone rather than approximated: a failed flush leaves its
//! seal pending, because failures are `DI-3.2.4`'s; and a write to a file a consumer provider
//! serves leaves its seal pending, because providers are `DI-3.2.6`'s.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::ids::OpId;
use crate::types::FileKey;

#[cfg(test)]
mod tests;

/// Who must answer for a file's writes (DI-D-27): the built-in default, by flushing the file,
/// and the consumer's provider, for the domains it serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Routing {
    /// The file has no declared domain, or one the consumer's provider does not serve.
    pub(crate) default: bool,
    /// The file has a domain the consumer's provider serves.
    pub(crate) provider: bool,
}

/// A flush now due: seal `through`'s flush of `file`, through `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Flush<E, T> {
    pub(crate) through: E,
    pub(crate) file: FileKey,
    pub(crate) target: T,
}

/// What a change made due.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Due<E, T> {
    /// Flushes to push.
    pub(crate) flushes: Vec<Flush<E, T>>,
    /// Seals now durable, in order: one `Durable` entry each.
    pub(crate) durable: Vec<E>,
}

impl<E, T> Default for Due<E, T> {
    fn default() -> Self {
        Self {
            flushes: Vec::new(),
            durable: Vec::new(),
        }
    }
}

/// An epoch's state as this lineage sees it; the contract's `EpochState` adds the failure states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Open,
    Pending,
    Durable,
}

/// The answer to a seal request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Sealing<E, T> {
    /// A new seal, and what it made due at once.
    Submitted(Due<E, T>),
    /// At or below the seal point: the epoch's state, and nothing changes.
    AlreadySealed(State),
}

/// A write pushed and not yet completed.
struct InFlight<E, T> {
    epoch: E,
    file: FileKey,
    target: T,
    routing: Routing,
}

/// A write completed successfully and not yet named to a seal.
struct Completed<T> {
    file: FileKey,
    target: T,
    routing: Routing,
}

enum SealState {
    /// A write at or below the seal is still in flight.
    Waiting,
    /// Its flushes are out. `files` are those not yet answered; `failed` once one failed; and
    /// `provider_owed` while a write of it needs a provider answer no provider is asked for yet.
    Flushing {
        files: Vec<FileKey>,
        failed: bool,
        provider_owed: bool,
    },
    Done,
}

struct Seal<E> {
    through: E,
    state: SealState,
}

/// One lineage's durability state.
pub(crate) struct Durability<E, T> {
    sealed_through: Option<E>,
    durable_through: Option<E>,
    in_flight: HashMap<OpId, InFlight<E, T>>,
    in_flight_by_epoch: BTreeMap<E, usize>,
    completed: BTreeMap<E, Vec<Completed<T>>>,
    seals: VecDeque<Seal<E>>,
}

impl<E: Copy + Ord, T: Clone> Durability<E, T> {
    pub(crate) fn new() -> Self {
        Self {
            sealed_through: None,
            durable_through: None,
            in_flight: HashMap::new(),
            in_flight_by_epoch: BTreeMap::new(),
            completed: BTreeMap::new(),
            seals: VecDeque::new(),
        }
    }

    pub(crate) fn sealed_through(&self) -> Option<E> {
        self.sealed_through
    }

    pub(crate) fn durable_through(&self) -> Option<E> {
        self.durable_through
    }

    /// The seal point, if a write tagged `epoch` would land at or below it (guarantee 6).
    pub(crate) fn refuses(&self, epoch: E) -> Option<E> {
        self.sealed_through.filter(|&sealed| epoch <= sealed)
    }

    /// An epoch's state. Failures (`Blocked`, `Abandoned`) are `DI-3.2.4`'s.
    pub(crate) fn state(&self, epoch: E) -> State {
        match (self.sealed_through, self.durable_through) {
            (Some(sealed), _) if epoch > sealed => State::Open,
            (None, _) => State::Open,
            (_, Some(durable)) if epoch <= durable => State::Durable,
            _ => State::Pending,
        }
    }

    /// A write was accepted by the ring.
    pub(crate) fn pushed(
        &mut self,
        op: OpId,
        epoch: E,
        file: FileKey,
        target: T,
        routing: Routing,
    ) {
        debug_assert!(
            self.refuses(epoch).is_none(),
            "a write at or below the seal"
        );
        *self.in_flight_by_epoch.entry(epoch).or_insert(0) += 1;
        self.in_flight.insert(
            op,
            InFlight {
                epoch,
                file,
                target,
                routing,
            },
        );
    }

    /// A write completed. One that failed is reported on its own completion and named to no
    /// flush (guarantee 1).
    pub(crate) fn completed(&mut self, op: OpId, succeeded: bool) -> Due<E, T> {
        let Some(write) = self.in_flight.remove(&op) else {
            debug_assert!(false, "a write completed that was never recorded as pushed");
            return Due::default();
        };
        if let Some(count) = self.in_flight_by_epoch.get_mut(&write.epoch) {
            *count -= 1;
            if *count == 0 {
                self.in_flight_by_epoch.remove(&write.epoch);
            }
        }
        if succeeded {
            self.completed
                .entry(write.epoch)
                .or_default()
                .push(Completed {
                    file: write.file,
                    target: write.target,
                    routing: write.routing,
                });
        }
        self.advance()
    }

    /// Seal every epoch at or below `through` (DI-D-9).
    pub(crate) fn seal(&mut self, through: E) -> Sealing<E, T> {
        if self.sealed_through.is_some_and(|sealed| through <= sealed) {
            return Sealing::AlreadySealed(self.state(through));
        }
        self.sealed_through = Some(through);
        self.seals.push_back(Seal {
            through,
            state: SealState::Waiting,
        });
        Sealing::Submitted(self.advance())
    }

    /// Seal `through`'s flush of `file` completed.
    pub(crate) fn flushed(&mut self, through: E, file: FileKey, succeeded: bool) -> Due<E, T> {
        let seal = self.seals.iter_mut().find(|seal| seal.through == through);
        let Some(Seal {
            state: SealState::Flushing { files, failed, .. },
            ..
        }) = seal
        else {
            debug_assert!(false, "a flush completed for a seal not flushing");
            return Due::default();
        };
        files.retain(|f| *f != file);
        *failed |= !succeeded;
        self.advance()
    }

    /// Start every seal whose writes have all completed, and advance the high-water mark over the
    /// seals done.
    fn advance(&mut self) -> Due<E, T> {
        let mut due = Due::default();
        let oldest_in_flight = self.in_flight_by_epoch.keys().next().copied();
        for index in 0..self.seals.len() {
            let through = self.seals[index].through;
            if !matches!(self.seals[index].state, SealState::Waiting) {
                continue;
            }
            if oldest_in_flight.is_some_and(|oldest| oldest <= through) {
                // Seals are in ascending order, so no later one is ready either.
                break;
            }
            let named: Vec<E> = self.completed.range(..=through).map(|(e, _)| *e).collect();
            let mut files: Vec<FileKey> = Vec::new();
            let mut provider_owed = false;
            for epoch in named {
                for write in self.completed.remove(&epoch).unwrap_or_default() {
                    provider_owed |= write.routing.provider;
                    if write.routing.default && !files.contains(&write.file) {
                        files.push(write.file);
                        due.flushes.push(Flush {
                            through,
                            file: write.file,
                            target: write.target,
                        });
                    }
                }
            }
            self.seals[index].state = SealState::Flushing {
                files,
                failed: false,
                provider_owed,
            };
        }
        for seal in &mut self.seals {
            if let SealState::Flushing {
                files,
                failed: false,
                provider_owed: false,
            } = &seal.state
                && files.is_empty()
            {
                seal.state = SealState::Done;
            }
        }
        while let Some(Seal {
            state: SealState::Done,
            through,
        }) = self.seals.front()
        {
            let through = *through;
            self.seals.pop_front();
            self.durable_through = Some(through);
            due.durable.push(through);
        }
        due
    }
}
