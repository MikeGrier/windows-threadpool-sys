// Copyright (c) 2026 Mike Grier
//! Absence with its reason attached.
//!
//! [`Observed`] is the vocabulary [D-13](../DESIGN-NOTES.md) asks for: an
//! `Option` spells three different facts identically, and a consumer that
//! cannot tell them apart will eventually read one as the other.

/// A value that may be absent, saying **which** absence it means.
///
/// # Why not `Option`
///
/// `Option<T>` has one `None` and this crate has three distinct facts to put
/// in it (D-13):
///
/// - **not observed** -- nothing asked, so the value may well exist;
/// - **observed and absent** -- something asked, and the answer was "none";
/// - **a negative result** -- not an absence at all, but a computed answer
///   whose value happens to be "no".
///
/// The third is deliberately **not** a variant here. It is not an absence, so
/// giving it one would re-create the conflation this type exists to remove: a
/// computed "no" is an ordinary value and belongs in `T`, or in an `Option`
/// documented as meaning exactly that. This type covers the two that are
/// genuinely about *whether we know*.
///
/// # Why this matters more than it looks
///
/// Both confusions are silent and both invent something. Reading "we did not
/// look" as "there is none" invents a fact about the machine; reading "there
/// is none" as "we did not look" sends a caller off to re-derive something
/// already settled. Neither fails a test that only checks the happy path.
///
/// # The contested case
///
/// Per [D-19](../DESIGN-NOTES.md), a subject the two Win32 sources genuinely
/// disagreed about is one the unified view does not cover -- which is
/// [`Observed::NotObserved`], not a fourth state.
///
/// [D-16](../DESIGN-NOTES.md)'s retry runs before anything is represented, and
/// removes the transient case it can settle: two enumerations describing
/// different sets of processors because the machine changed between the calls.
/// What it deliberately does not retry is a disagreement about how processors
/// are *grouped*, which [D-17](../DESIGN-NOTES.md) establishes is persistent in
/// the field -- re-reading cannot settle those, so they are carried as separate
/// per-source observations instead. Either way "we cannot say" is the honest
/// answer, and [`crate::Coherence`] on the topology says which of the two
/// happened during collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Observed<T> {
    /// The platform was asked and reported this value.
    Known(T),
    /// The platform was asked and reported that there is none.
    ///
    /// A positive statement about the machine, not a gap in what we did.
    Absent,
    /// Nothing asked, or there is no way to ask.
    ///
    /// Says nothing about whether the value exists. This is the variant a
    /// hand-written description leaves behind when it omits a field, and the
    /// one a contested subject collapses to (D-19).
    NotObserved,
}

impl<T> Observed<T> {
    /// The value, if the platform reported one.
    ///
    /// **Discards the reason for an absence**, which is the whole point of
    /// this type -- so reach for it only where the caller genuinely does not
    /// care why the value is missing, and not merely to get back to a
    /// familiar shape.
    pub fn known(self) -> Option<T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Absent | Self::NotObserved => None,
        }
    }

    /// Whether the platform was asked at all.
    ///
    /// True for both [`Observed::Known`] and [`Observed::Absent`], because
    /// both are answers. This is the question a caller asks before deciding
    /// whether re-deriving a value could possibly help: it cannot, if the
    /// platform already said there is none.
    pub fn was_observed(&self) -> bool {
        !matches!(self, Self::NotObserved)
    }

    /// Apply `f` to a known value, preserving the reason for an absence.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Observed<U> {
        match self {
            Self::Known(value) => Observed::Known(f(value)),
            Self::Absent => Observed::Absent,
            Self::NotObserved => Observed::NotObserved,
        }
    }
}

impl<T> Default for Observed<T> {
    /// [`Observed::NotObserved`], because the safe default is the one that
    /// claims nothing.
    ///
    /// Same reasoning as [`Provenance`](crate::Provenance)'s default pointing
    /// at distrust (D-12): forgetting to set a field must not assert
    /// something about the machine, so the default is the variant that says
    /// only "nobody looked".
    fn default() -> Self {
        Self::NotObserved
    }
}

#[cfg(test)]
mod tests;
