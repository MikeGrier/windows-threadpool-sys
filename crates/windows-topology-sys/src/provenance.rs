// Copyright (c) 2026 Mike Grier
//! Where a topology's content came from.

use std::fmt;

/// Where a [`MachineMemoryTopology`](crate::MachineMemoryTopology)'s content came from.
///
/// # Why this exists
///
/// This crate deliberately lets a topology be built three ways: read from the
/// running system, constructed by hand, or deserialized from a description
/// written for -- in the crate documentation's own words -- "a machine you do
/// not have". That is a feature, and it is the reason a marker is needed:
/// without one the three are indistinguishable once built, and a consumer
/// handed a fabricated or foreign topology treats it as this machine's truth.
///
/// The failure that motivates this is not exotic. A probe measuring NUMA
/// behaviour on a machine with no NUMA needs a synthetic topology to test its
/// selection logic; the whole point of a probe is that its output is believed.
/// A number produced against fabricated topology and quoted without a label is
/// worse than no number, because nothing downstream can tell.
///
/// # Ordering is trust
///
/// The variants are ordered `Synthetic < Restored < Measured`, so the derived
/// `Ord` *is* the trust order and [`Ord::min`] implements "never upgrade".
/// [`Self::downgraded_to`] relies on this.
///
/// # The default is the untrusted value, on purpose
///
/// [`Self::Synthetic`] is [`Default`], so a `MachineMemoryTopology` built by
/// [`Default::default`], completed with `..Default::default()`, or otherwise
/// assembled without a thought about provenance comes out **tainted**. A caller
/// must do work to claim data is real, rather than work to admit it is not.
/// Getting this backwards would mean every forgetful construction silently
/// asserts it measured the machine.
///
/// # The threat model is accident, not forgery
///
/// A caller who writes `provenance: Provenance::Measured` over data they
/// fabricated has lied deliberately, and no type in a crate with public fields
/// prevents that. This defends against *forgetting*, which is the thing that
/// actually happens. The one place forgery is refused is deserialization, where
/// the input is a file rather than a line of code someone had to write on
/// purpose -- see [`Self::downgraded_to`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Provenance {
    /// Constructed by hand, or by [`Default`]. Describes no machine in
    /// particular, and is the default precisely so that forgetting is safe.
    #[default]
    Synthetic,
    /// Deserialized from a description. It may faithfully describe some real
    /// machine -- but not necessarily *this* one, and nothing in the file can
    /// establish which.
    Restored,
    /// Read from the running system by [`MachineMemoryTopology::discover`](crate::MachineMemoryTopology::discover).
    /// The only variant that asserts "this is the machine you are on".
    Measured,
}

impl Provenance {
    /// Whether this describes the machine actually running the code.
    ///
    /// The single question most consumers want to ask, named so that the
    /// answer cannot be got wrong by comparing against the wrong variant.
    #[must_use]
    pub fn is_measured(self) -> bool {
        self == Self::Measured
    }

    /// This provenance, or `ceiling` if this one claims more trust.
    ///
    /// Only ever lowers. Deserialization uses it with a ceiling of
    /// [`Self::Restored`] so a description saying `"measured"` is not honoured:
    /// a file cannot establish that it is the machine you are on, however
    /// sincerely it asserts it. A description saying `"synthetic"` stays
    /// synthetic, because the ceiling is a maximum and not an assignment.
    #[must_use]
    pub fn downgraded_to(self, ceiling: Self) -> Self {
        self.min(ceiling)
    }

    /// A short word for a rendered form.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Synthetic => "SYNTHETIC",
            Self::Restored => "RESTORED",
            Self::Measured => "measured",
        }
    }
}

impl fmt::Display for Provenance {
    /// Renders the untrusted variants in capitals and the trusted one in lower
    /// case, so a tainted value is visibly louder than a real one in any string
    /// it reaches. See [`Self::label`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Deserialize a provenance, refusing any claim above [`Provenance::Restored`].
///
/// Wired onto [`MachineMemoryTopology::provenance`](crate::MachineMemoryTopology::provenance) so the rule
/// holds for every description, including one hand-edited to claim it was
/// measured.
///
/// # Errors
///
/// Returns whatever the underlying deserializer failed with.
#[cfg(feature = "serde")]
pub fn deserialize_downgraded<'de, D>(deserializer: D) -> Result<Provenance, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    Ok(Provenance::deserialize(deserializer)?.downgraded_to(Provenance::Restored))
}

#[cfg(test)]
mod tests;
