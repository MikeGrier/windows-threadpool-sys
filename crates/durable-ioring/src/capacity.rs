// Copyright (c) 2026 Mike Grier
//! What a consumer declares at construction, and what is refused past it (DI-D-107).
//!
//! The library gives the declaration no default: a consumer states its own numbers, so a wrong
//! silent one is not possible. Each count is a ceiling the instance enforces by refusing, never by
//! growing.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt;

use fixed_hash_table::{HeapMap, ReserveError};

use crate::ordered::Ordered;
use crate::types::FlushDomain;

#[cfg(test)]
mod tests;

/// The most an instance holds of each thing it counts, declared when it is built (DI-D-107).
///
/// Each count sizes tables that are allocated once, when the instance is built (DI-D-106):
/// `operations` sizes the span entries, the places of the submit window and the submissions in
/// flight, and with the provider's domains the open flush pairs; `targets` sizes the targets and
/// the operations counted by target; and `domains` the interned flush domains. A count too large
/// for a table, or for the allocator, fails the build, naming the table.
///
/// ```
/// use durable_ioring::Capacity;
///
/// // A small log: a few hundred operations at once, a handful of everything else.
/// let capacity = Capacity {
///     operations: 256,
///     lineages: 4,
///     imports: 2,
///     targets: 4,
///     domains: 4,
/// };
/// assert_eq!(capacity.lineages, 4);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity {
    /// The operations outstanding at once: pushed and not yet popped as a completion, whether in
    /// flight, held for a gate, or completed and queued, reads and writes alike. A write also
    /// holds its place while it is uncovered, and while any unresolved failure suspects it, so the
    /// window covers written but unsealed data as well as I/O in flight (DI-D-100). A seal the
    /// high-water mark has not passed keeps the place of the first write it covers (DI-D-110).
    pub operations: usize,
    /// The lineages the instance has not retired, the default among them, so at least one. An
    /// ended lineage counts until it is retired.
    pub lineages: usize,
    /// The unresolved imported failures: one counts from its import until it is resolved.
    pub imports: usize,
    /// The targets attached at once.
    pub targets: usize,
    /// The distinct flush domains the instance has been given, the provider's among them. A
    /// domain stays counted after the target that declared it is removed.
    pub domains: usize,
}

impl Capacity {
    /// How many parts of seals' flushes can be out at once when a provider serves `served` domains:
    /// a file's flush for each span entry, and a part for each served domain of each seal, a seal
    /// holding at least one uncovered write. `None` if that is more than a `usize`.
    pub(crate) fn flush_parts(&self, served: usize) -> Option<usize> {
        self.operations.checked_mul(served.checked_add(1)?)
    }

    /// The most failures unresolved at once with a provider serving `served` domains (DI-D-109):
    /// the observed ones, at most the flush parts, and the imported ones, at most `imports`.
    ///
    /// An observed failure is the answer of one part of one seal -- a file's flush, or a served
    /// domain's request -- and a part is issued only for that seal's named writes, so the failure
    /// suspects them; it holds the place of each until it is resolved (DI-D-100). A part answers
    /// once, and a write is named to one seal, whose parts for its file are its flush and the
    /// served domains among its file's: so naming each observed failure by its part and one of the
    /// part's writes names each once, and the names are at most the places, at most the
    /// operations, times one plus the served domains. An answer for a lineage no longer live
    /// observes nothing. `None` if that is more than a `usize`.
    pub(crate) fn failures(&self, served: usize) -> Option<usize> {
        self.flush_parts(served)?.checked_add(self.imports)
    }
}

/// A table named `name`, empty, holding `entries` at least, its storage allocated now and never
/// again (DI-D-106).
///
/// # Errors
///
/// [`ConstructionError::Table`] when `entries` is `None`, the size being more than a `usize`, or no
/// table that large can be made or allocated.
pub(crate) fn table<K, V>(
    name: &'static str,
    entries: Option<usize>,
) -> Result<HeapMap<K, V>, ConstructionError> {
    let entries = entries.ok_or(ConstructionError::Table {
        table: name,
        error: ReserveError::TooLarge,
    })?;
    HeapMap::try_with_capacity(entries)
        .map_err(|error| ConstructionError::Table { table: name, error })
}

/// An ordered table named `name` for `entries` entries (DI-D-109), or why it cannot be made; `None`
/// is a size past what a `usize` holds.
pub(crate) fn ordered<K: Ord, V>(
    name: &'static str,
    entries: Option<usize>,
) -> Result<Ordered<K, V>, ConstructionError> {
    let entries = entries.ok_or(ConstructionError::Table {
        table: name,
        error: ReserveError::TooLarge,
    })?;
    Ordered::with_capacity(entries).map_err(|error| ConstructionError::Table {
        table: name,
        error: ReserveError::Alloc(error),
    })
}

/// A list named `name` with room for `items` reserved now (DI-D-97): the core's per-call lists,
/// each sized from a bound on what one call can put in it, or why it cannot be made; `None` is a
/// size past what a `usize` holds.
pub(crate) fn list<T>(
    name: &'static str,
    items: Option<usize>,
) -> Result<Vec<T>, ConstructionError> {
    let mut list = Vec::new();
    list.try_reserve_exact(reserved(name, items)?)
        .map_err(|error| ConstructionError::Table {
            table: name,
            error: ReserveError::Alloc(error),
        })?;
    Ok(list)
}

/// As [`list`], a queue taken from the front.
pub(crate) fn queue<T>(
    name: &'static str,
    items: Option<usize>,
) -> Result<VecDeque<T>, ConstructionError> {
    let mut queue = VecDeque::new();
    queue
        .try_reserve_exact(reserved(name, items)?)
        .map_err(|error| ConstructionError::Table {
            table: name,
            error: ReserveError::Alloc(error),
        })?;
    Ok(queue)
}

/// The size `items` names, or the refusal for one past a `usize`.
fn reserved(name: &'static str, items: Option<usize>) -> Result<usize, ConstructionError> {
    items.ok_or(ConstructionError::Table {
        table: name,
        error: ReserveError::TooLarge,
    })
}

/// Push `item` onto `list` if its reserved room has space for it, or hand it back: a list made by
/// [`list`] is sized so this never happens, and a caller handed an item back reports the
/// inconsistency rather than let the list grow.
pub(crate) fn within<T>(list: &mut Vec<T>, item: T) -> Result<(), T> {
    if list.len() == list.capacity() {
        return Err(item);
    }
    list.push(item);
    Ok(())
}

/// As [`within`], for a queue.
pub(crate) fn within_queue<T>(queue: &mut VecDeque<T>, item: T) -> Result<(), T> {
    if queue.len() == queue.capacity() {
        return Err(item);
    }
    queue.push_back(item);
    Ok(())
}

/// Which of a [`Capacity`]'s counts was reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Limit {
    /// [`Capacity::operations`].
    Operations,
    /// [`Capacity::lineages`].
    Lineages,
    /// [`Capacity::imports`].
    Imports,
    /// [`Capacity::targets`].
    Targets,
    /// [`Capacity::domains`].
    Domains,
}

impl Limit {
    /// What the count counts.
    fn noun(self) -> &'static str {
        match self {
            Self::Operations => "operations",
            Self::Lineages => "lineages",
            Self::Imports => "unresolved imported failures",
            Self::Targets => "targets",
            Self::Domains => "flush domains",
        }
    }
}

/// A refusal because the instance already holds as many of something as it was built for. Nothing
/// was changed: what was refused can be offered again once something it counts has been let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtCapacity {
    /// The count that was reached.
    pub limit: Limit,
    /// What it was declared as.
    pub declared: usize,
}

impl fmt::Display for AtCapacity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the instance is at its capacity of {} {}",
            self.declared,
            self.limit.noun()
        )
    }
}

impl core::error::Error for AtCapacity {}

/// Why an instance could not be built from the capacity and the provider it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConstructionError {
    /// [`Capacity::lineages`] is zero, and the default lineage is one.
    NoLineages,
    /// The provider named a domain twice: the domain.
    DuplicateDomain(FlushDomain),
    /// The provider serves more domains than [`Capacity::domains`] declares.
    TooManyDomains {
        /// How many the provider serves.
        served: usize,
        /// What was declared.
        declared: usize,
    },
    /// One of the tables the declaration sizes could not be made: its size is more than a table can
    /// be, or the allocator could not provide it.
    Table {
        /// Which table, by what it holds.
        table: &'static str,
        /// Why.
        error: ReserveError,
    },
}

impl fmt::Display for ConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLineages => {
                f.write_str("the capacity declares no lineage, and the default is one")
            }
            Self::DuplicateDomain(domain) => {
                write!(f, "the provider named the domain {domain:?} twice")
            }
            Self::TooManyDomains { served, declared } => write!(
                f,
                "the provider serves {served} flush domains and the capacity declares {declared}"
            ),
            Self::Table { table, error } => {
                write!(f, "the table of {table} could not be made: {error}")
            }
        }
    }
}

impl core::error::Error for ConstructionError {}
