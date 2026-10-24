// Copyright (c) 2026 Mike Grier
//! What a consumer declares at construction, and what is refused past it (DI-D-107).
//!
//! The library gives the declaration no default: a consumer states its own numbers, so a wrong
//! silent one is not possible. Each count is a ceiling the instance enforces by refusing, never by
//! growing.

use core::fmt;

use crate::types::FlushDomain;

/// The most an instance holds of each thing it counts, declared when it is built (DI-D-107).
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
    /// window covers written but unsealed data as well as I/O in flight (DI-D-100).
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
        }
    }
}

impl core::error::Error for ConstructionError {}
