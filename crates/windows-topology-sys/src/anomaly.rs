// Copyright (c) 2026 Mike Grier
//! What an enumeration observed about the shape of what it was reading.

use crate::observation::Source;

/// A record that did not fit the buffer that contained it.
///
/// Per [D-24](../DESIGN-NOTES.md#d-24) a structurally incoherent record is
/// **recorded rather than thrown or swallowed**. It is not evidence that this
/// crate reached an inconsistent state, so it is not a panic; and dropping it
/// silently would leave a consumer with a short list and no way to tell a
/// truncated enumeration from a small machine.
///
/// This is [`crate::AttributeObservation`]'s instinct one layer down: the
/// topology carries what was observed, including the observation that the
/// bytes did not describe what they claimed to.
///
/// None of these are expected. Windows does not produce them, and a
/// `MachineMemoryTopology` from [`crate::MachineMemoryTopology::discover`] on a
/// healthy machine carries none. They exist so that a machine which *is*
/// misbehaving -- a defective hypervisor, a driver corrupting a buffer -- is
/// diagnosable from the returned data rather than from a debugger.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnumerationAnomaly {
    /// Which enumeration was being read.
    pub source: Source,
    /// The byte offset within that enumeration's buffer where it stopped.
    pub offset: usize,
    /// What was wrong there.
    pub kind: AnomalyKind,
}

/// What made a record undecodable.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum AnomalyKind {
    /// The record declared a length too small to hold its own fixed fields.
    ///
    /// A `Size` of zero is this case, and it is the one that would otherwise
    /// loop forever: the walk advances by `Size`.
    Undersized {
        /// The length the record declared.
        declared: usize,
        /// The smallest length that could hold the record's fixed fields.
        minimum: usize,
    },
    /// The record declared a length longer than the bytes remaining.
    OverrunsBuffer {
        /// The length the record declared.
        declared: usize,
        /// The bytes actually left in the buffer.
        remaining: usize,
    },
    /// Bytes were left over that cannot hold another record's length field.
    TrailingBytes {
        /// How many bytes were left.
        remaining: usize,
    },
    /// A record's trailing array declared more entries than the record held.
    ///
    /// The entries that did fit are decoded and kept; this records that the
    /// count claimed more.
    TruncatedArray {
        /// The entry count the record declared.
        declared: usize,
        /// How many were actually read before the record ended.
        decoded: usize,
    },
}

impl EnumerationAnomaly {
    pub(crate) fn undersized(
        source: Source,
        offset: usize,
        declared: usize,
        minimum: usize,
    ) -> Self {
        Self {
            source,
            offset,
            kind: AnomalyKind::Undersized { declared, minimum },
        }
    }

    pub(crate) fn overruns(
        source: Source,
        offset: usize,
        declared: usize,
        remaining: usize,
    ) -> Self {
        Self {
            source,
            offset,
            kind: AnomalyKind::OverrunsBuffer {
                declared,
                remaining,
            },
        }
    }

    pub(crate) fn trailing_bytes(source: Source, offset: usize, remaining: usize) -> Self {
        Self {
            source,
            offset,
            kind: AnomalyKind::TrailingBytes { remaining },
        }
    }

    pub(crate) fn truncated_array(
        source: Source,
        offset: usize,
        declared: usize,
        decoded: usize,
    ) -> Self {
        Self {
            source,
            offset,
            kind: AnomalyKind::TruncatedArray { declared, decoded },
        }
    }
}
