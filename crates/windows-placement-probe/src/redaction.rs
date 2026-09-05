// Copyright (c) 2026 Mike Grier
//! Which secondary metadata a submission carries.
//!
//! # The measurement is not negotiable; the context is
//!
//! A record holds two different kinds of thing. The **measurement** -- the
//! topology, the placements, the timings -- is the reason the record exists,
//! and redacting it would leave a file that says nothing. The **context** --
//! when the run happened, what CPU it was, which OS build, whether a hypervisor
//! was detected -- explains a measurement without being one.
//!
//! Only the context is redactable, and it is redacted by default. A submission
//! is a favour done by a stranger, and the default should ask for the least
//! that still answers the question.
//!
//! # Why the default flipped
//!
//! The tool began by collecting all of it and offering `--no-cpu-model` as the
//! single escape hatch. That is the wrong way round: it asks a runner to
//! recognise, in advance, which field they would rather not send. Defaulting to
//! redacted asks nothing, and a runner who wants to help more can say so.
//!
//! What that costs is real and is stated rather than glossed: a defect that
//! appears only on one OS build, or only under one hypervisor, is exactly what
//! the context is for, and a corpus without it cannot show that correlation.
//! See this crate's `README.md`.
//!
//! # Suppression is recorded, never merely absent
//!
//! Every field this policy can withhold carries a way to say it was withheld,
//! because "the runner did not send this" and "the host would not answer" are
//! different facts and a collector that cannot tell them apart will eventually
//! read one as the other. The mechanism differs by field only because the types
//! differ: a `None` beside a `*_suppressed` flag for the optional strings, and a
//! dedicated [`Suppressed`](crate::machine::VirtualisationHint::Suppressed)
//! variant for the virtualisation hint, whose other variants are all claims
//! about what was observed.

/// Which secondary metadata a record includes.
///
/// Constructed by the tool from what the runner asked for, then carried into
/// [`MachineDescription::read`](crate::machine::MachineDescription::read) and
/// [`SubmissionRecord::new`](crate::record::SubmissionRecord::new) so that one
/// decision governs every field rather than each site deciding again.
///
/// [`Default`] is [`redacted`](Self::redacted), so a caller that does not think
/// about this collects the least.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetadataPolicy {
    include_timestamp: bool,
    include_cpu_model: bool,
    include_os_build: bool,
    include_virtualisation: bool,
}

impl MetadataPolicy {
    /// Withhold every piece of secondary metadata. The default.
    #[must_use]
    pub const fn redacted() -> Self {
        Self {
            include_timestamp: false,
            include_cpu_model: false,
            include_os_build: false,
            include_virtualisation: false,
        }
    }

    /// Include every piece of secondary metadata, at the runner's request.
    #[must_use]
    pub const fn included() -> Self {
        Self {
            include_timestamp: true,
            include_cpu_model: true,
            include_os_build: true,
            include_virtualisation: true,
        }
    }

    /// The same policy with the CPU model withheld.
    ///
    /// The one subtraction the tool offers, and it is offered because the case
    /// is real: an unreleased part has a name that must not travel while its OS
    /// build and hypervisor are as ordinary as anyone else's. It does not make
    /// confidential hardware safe to submit -- the topology describes the part
    /// whether or not it is named, and the topology is the measurement.
    #[must_use]
    pub const fn without_cpu_model(self) -> Self {
        Self {
            include_cpu_model: false,
            ..self
        }
    }

    /// Whether the record carries when the run happened.
    #[must_use]
    pub const fn includes_timestamp(self) -> bool {
        self.include_timestamp
    }

    /// Whether the record carries the processor's marketing name.
    #[must_use]
    pub const fn includes_cpu_model(self) -> bool {
        self.include_cpu_model
    }

    /// Whether the record carries the OS build.
    #[must_use]
    pub const fn includes_os_build(self) -> bool {
        self.include_os_build
    }

    /// Whether the record carries the virtualisation hint and its name.
    #[must_use]
    pub const fn includes_virtualisation(self) -> bool {
        self.include_virtualisation
    }

    /// Whether anything at all is included.
    ///
    /// Used by the collection notice to decide whether to describe an opt-in or
    /// a subtraction: advising `--include-metadata` to a runner who has already
    /// passed it is noise, and so is advising `--no-cpu-model` to one who is
    /// sending no metadata at all.
    #[must_use]
    pub const fn includes_anything(self) -> bool {
        self.include_timestamp
            || self.include_cpu_model
            || self.include_os_build
            || self.include_virtualisation
    }
}

#[cfg(test)]
mod tests;
