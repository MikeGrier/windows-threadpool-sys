// Copyright (c) 2026 Mike Grier
//! Tracking which parts of a poisoned region were *permitted* to change, so
//! everything else can be held to its poison at teardown.
//!
//! # Why this is not just [`crate::poison::first_mismatch`]
//!
//! Checking a buffer right after one operation answers "did *that* operation
//! stay inside its span". It cannot answer the question this module exists
//! for: after a whole run of operations, is there a byte that changed which no
//! operation ever accounted for?
//!
//! That second question is the one a stray write actually fails. A write
//! attributable to no particular operation -- a kernel touching a buffer after
//! its completion was reported, a slot recycled while still in flight, an
//! index computed from the wrong base -- shows up nowhere in a per-operation
//! check, because by construction each individual check looks only at the
//! buffer it was told about, over the span it was told about.
//!
//! # How the accounting works
//!
//! A [`Witness`] starts owning a whole region and permits nothing. Each
//! completed operation [`permit`](Witness::permit)s exactly the bytes it was
//! entitled to change -- for a read, the span offset and the *transferred*
//! count rather than the requested one. At teardown [`verify`](Witness::verify)
//! walks the **gaps** between permitted ranges and requires every byte in them
//! to still be poison.
//!
//! Ranges are merged rather than assumed disjoint, so a slot legitimately
//! written twice, or written by overlapping operations, does not produce a
//! false accusation. Over-reporting a defect is the same failure as missing
//! one.

use std::ops::Range;

use crate::poison;

/// A byte that changed without any operation having accounted for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breach {
    /// Offset into the region, from its start.
    pub at: usize,
    /// The poison byte that should have been there.
    pub expected: u8,
    /// What was found instead.
    pub found: u8,
}

impl std::fmt::Display for Breach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "byte {} changed without being accounted for: expected poison {:#04x}, found {:#04x}",
            self.at, self.expected, self.found
        )
    }
}

/// Accounting for one poisoned region: what it is, and what was allowed to
/// change it.
#[derive(Clone, Debug)]
pub struct Witness {
    ordinal: u64,
    len: usize,
    permitted: Vec<Range<usize>>,
}

impl Witness {
    /// Begin witnessing a `len`-byte region poisoned for `ordinal`, with
    /// nothing yet permitted to change.
    #[must_use]
    pub fn new(ordinal: u64, len: usize) -> Self {
        Self {
            ordinal,
            len,
            permitted: Vec::new(),
        }
    }

    /// The poison ordinal this region was filled for.
    #[must_use]
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// Record that `len` bytes from `offset` were legitimately changed.
    ///
    /// Pass the count actually **transferred**, not the count requested: a
    /// short read leaves the rest of its span untouched, and permitting the
    /// whole span would excuse a write that never should have happened.
    ///
    /// Out-of-range portions are clamped rather than rejected, so a caller
    /// reporting a completion honestly cannot produce a panic here; what
    /// matters is that nothing beyond the region is silently forgiven.
    pub fn permit(&mut self, offset: usize, len: usize) {
        let start = offset.min(self.len);
        let end = offset.saturating_add(len).min(self.len);
        if start < end {
            self.permitted.push(start..end);
        }
    }

    /// How many bytes are currently accounted for, after merging overlaps.
    #[must_use]
    pub fn permitted_bytes(&self) -> usize {
        merged(&self.permitted).iter().map(Range::len).sum()
    }

    /// Check every byte that was **not** permitted to change against its
    /// poison.
    ///
    /// # Errors
    ///
    /// The first unaccounted-for byte, as a [`Breach`].
    pub fn verify(&self, seed: u64, bytes: &[u8]) -> Result<(), Breach> {
        let end = self.len.min(bytes.len());
        let mut cursor = 0_usize;
        for range in merged(&self.permitted) {
            self.check_gap(seed, bytes, cursor..range.start.min(end))?;
            cursor = range.end;
            if cursor >= end {
                return Ok(());
            }
        }
        self.check_gap(seed, bytes, cursor..end)
    }

    /// Hold one unpermitted stretch to its poison.
    fn check_gap(&self, seed: u64, bytes: &[u8], gap: Range<usize>) -> Result<(), Breach> {
        if gap.start >= gap.end {
            return Ok(());
        }
        let region = &bytes[gap.start..gap.end];
        match poison::first_mismatch(seed, self.ordinal, gap.start, region) {
            None => Ok(()),
            Some(offset) => {
                let at = gap.start + offset;
                Err(Breach {
                    at,
                    expected: poison::byte_at(seed, self.ordinal, at),
                    found: bytes[at],
                })
            }
        }
    }
}

/// Sort and coalesce, so overlapping or out-of-order permissions describe one
/// coherent set of accounted-for bytes.
fn merged(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut sorted: Vec<Range<usize>> = ranges.to_vec();
    sorted.sort_unstable_by_key(|range| range.start);

    let mut out: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match out.last_mut() {
            // Touching counts as overlapping: `0..4` and `4..8` leave no gap
            // between them, so treating them as separate would invent one.
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

#[cfg(test)]
mod tests;
