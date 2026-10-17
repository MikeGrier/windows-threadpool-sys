// Copyright (c) 2026 Mike Grier
//! What a target states about itself (DI-D-48, DI-D-50): the [`IoTarget`] trait.
//!
//! The obligations a target meets are the contract's ("What this contract requires of a target");
//! this module carries what a target *reports* -- its flush domains, warranty class, geometry,
//! capabilities and data access. The driver hands the target itself to the backend, which runs its
//! I/O, and its [`IoTargetDescription`] to the core, which decides with it (DI-D-52).

use std::fmt;

use crate::access::Access;
use crate::types::FlushDomain;

#[cfg(test)]
mod tests;

/// How far a target honours the contract's obligations (DI-D-50 P3). Information only: dioring
/// behaves identically for every class and does not label a `Durable` with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WarrantyClass {
    /// Reports every failure affecting its writes.
    Complete,
    /// Reports every failure of its own, but not a failure of another writer sharing its device's
    /// volatile cache.
    SharedDevice,
    /// May report success for a commit over data already lost.
    BestEffort,
}

/// The alignment a target's offsets, lengths and buffer addresses need, each a power of two, one
/// meaning none. Built through [`Geometry::new`] or [`Geometry::uniform`], which refuse any other
/// value, so a `Geometry` always holds powers of two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Geometry {
    offset: u32,
    length: u32,
    buffer: u32,
}

/// A geometry was asked for with an alignment that is not a power of two (zero included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryError {
    /// The alignment refused.
    pub alignment: u32,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "alignment {} is not a power of two", self.alignment)
    }
}

impl std::error::Error for GeometryError {}

fn power_of_two(alignment: u32) -> Result<u32, GeometryError> {
    if alignment.is_power_of_two() {
        Ok(alignment)
    } else {
        Err(GeometryError { alignment })
    }
}

impl Geometry {
    /// No alignment needed for anything.
    pub const UNALIGNED: Self = Self {
        offset: 1,
        length: 1,
        buffer: 1,
    };

    /// The alignments of offsets, lengths and buffer addresses.
    ///
    /// # Errors
    ///
    /// [`GeometryError`] naming the first of them, in that order, that is not a power of two.
    pub fn new(offset: u32, length: u32, buffer: u32) -> Result<Self, GeometryError> {
        Ok(Self {
            offset: power_of_two(offset)?,
            length: power_of_two(length)?,
            buffer: power_of_two(buffer)?,
        })
    }

    /// One alignment for offsets, lengths and buffer addresses alike, as a device's sector size
    /// is for unbuffered I/O.
    ///
    /// # Errors
    ///
    /// [`GeometryError`] if `alignment` is not a power of two.
    pub fn uniform(alignment: u32) -> Result<Self, GeometryError> {
        Self::new(alignment, alignment, alignment)
    }

    /// The alignment an offset needs.
    #[must_use]
    pub const fn offset_alignment(&self) -> u32 {
        self.offset
    }

    /// The alignment a length needs.
    #[must_use]
    pub const fn length_alignment(&self) -> u32 {
        self.length
    }

    /// The alignment a buffer's address needs.
    #[must_use]
    pub const fn buffer_alignment(&self) -> u32 {
        self.buffer
    }

    /// Whether an operation at `offset` of `length` bytes is aligned as this geometry needs.
    #[must_use]
    pub const fn admits(&self, offset: u64, length: u64) -> bool {
        offset.is_multiple_of(self.offset as u64) && length.is_multiple_of(self.length as u64)
    }

    /// Whether a buffer at `address` is aligned as this geometry needs.
    #[must_use]
    pub const fn admits_buffer(&self, address: usize) -> bool {
        address.is_multiple_of(self.buffer as usize)
    }
}

impl Default for Geometry {
    fn default() -> Self {
        Self::UNALIGNED
    }
}

/// What a target offers beyond the contract's obligations. Built with setters from
/// [`Capabilities::NONE`]; the fields are private, so a capability added later is not a breaking
/// change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[must_use]
pub struct Capabilities {
    write_through: bool,
}

impl Capabilities {
    /// Nothing beyond the obligations.
    pub const NONE: Self = Self {
        write_through: false,
    };

    /// The target honours a write's `WriteCaching::WriteThrough`.
    pub const fn with_write_through(mut self) -> Self {
        self.write_through = true;
        self
    }

    /// Whether the target honours a write's `WriteCaching::WriteThrough`.
    #[must_use]
    pub const fn write_through(&self) -> bool {
        self.write_through
    }
}

/// What a target states about itself, taken once when it is given to an instance: what the core
/// decides with, while the backend holds the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoTargetDescription {
    /// Every flush domain its writes depend on; none means it shares fate with every target.
    pub flush_domains: Vec<FlushDomain>,
    /// How far it honours its obligations.
    pub warranty: WarrantyClass,
    /// The alignment its operations need.
    pub geometry: Geometry,
    /// What it offers beyond its obligations.
    pub capabilities: Capabilities,
    /// What it can do with its data: an operation this does not cover is refused (DI-D-66).
    pub access: Access,
}

/// A target: what a write or a read addresses and a commit makes durable, defined by the
/// contract's obligations and by what it reports here (DI-D-50).
///
/// A target is not where its I/O runs: the backend it is attached to runs that, by whatever means
/// it has to reach what backs the target.
pub trait IoTarget: Send {
    /// Every flush domain its writes depend on. The declaration must be complete; an extra domain
    /// is safe, a missing one is not. None means it shares fate with every target.
    fn flush_domains(&self) -> &[FlushDomain];

    /// How far it honours its obligations.
    fn warranty(&self) -> WarrantyClass;

    /// The alignment its operations need.
    fn geometry(&self) -> Geometry;

    /// What it offers beyond its obligations.
    fn capabilities(&self) -> Capabilities;

    /// What it can do with its data: read it, write it, or both. A backend refuses a write to a target
    /// that cannot write and a read from one that cannot read, with
    /// [`RefusalCause::AccessDenied`](crate::backend::RefusalCause::AccessDenied). There is no
    /// default: a target that has not said what it can do has not said it can do everything.
    fn access(&self) -> Access;

    /// Everything it states, as the core takes it.
    fn describe(&self) -> IoTargetDescription {
        IoTargetDescription {
            flush_domains: self.flush_domains().to_vec(),
            warranty: self.warranty(),
            geometry: self.geometry(),
            capabilities: self.capabilities(),
            access: self.access(),
        }
    }
}
