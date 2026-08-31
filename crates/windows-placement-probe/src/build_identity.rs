// Copyright (c) 2026 Mike Grier
//! Which build produced a measurement.

use std::fmt;

/// Where a binary came from.
///
/// Ordered by trust, `Unknown < Local < Ci`, so the derived `Ord` is the trust
/// order -- the same shape as `windows_topology_sys::Provenance` one layer down,
/// and for the same reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum BuildSource {
    /// Could not be established. A `cargo install` from a crates.io tarball
    /// takes this path, and so does a source archive with no repository in it.
    #[default]
    Unknown,
    /// Built from a working copy on someone's machine.
    Local,
    /// Built by this repository's CI, which is the only path that produces an
    /// artifact traceable to the commit that made it.
    Ci,
}

impl BuildSource {
    /// A short word for a rendered line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Local => "LOCAL",
            Self::Ci => "ci",
        }
    }
}

impl fmt::Display for BuildSource {
    /// Renders the untrusted variants in capitals and the trusted one in lower
    /// case, so a build that cannot vouch for itself is visibly louder than one
    /// that can.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// The identity of the binary that produced a measurement.
///
/// # Why a measurement carries this
///
/// Submissions arrive over months from builds nobody here has a copy of. A
/// number that cannot name the code that produced it cannot be compared against
/// one taken later, and cannot be re-examined when a defect is found -- the
/// exact failure `windows_topology_sys::Provenance` fixes for *topology*, one
/// layer down and for the same reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BuildIdentity {
    /// The crate version this binary was built from.
    pub crate_version: &'static str,
    /// The commit, shortened, or `None` when it could not be determined.
    pub commit: Option<&'static str>,
    /// Whether the working tree had uncommitted changes at build time.
    ///
    /// `None` means the question could not be asked -- no repository, or no
    /// `git` -- which is a different fact from "clean" and is kept distinct
    /// from it.
    pub dirty: Option<bool>,
    /// Where the binary came from.
    pub source: BuildSource,
}

impl BuildIdentity {
    /// This binary's identity, as its build script stamped it.
    #[must_use]
    pub fn current() -> Self {
        Self {
            crate_version: env!("CARGO_PKG_VERSION"),
            commit: non_empty(env!("PLACEMENT_PROBE_COMMIT_OUT")),
            dirty: match env!("PLACEMENT_PROBE_DIRTY_OUT") {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            },
            source: match env!("PLACEMENT_PROBE_SOURCE_OUT") {
                "ci" => BuildSource::Ci,
                "local" => BuildSource::Local,
                _ => BuildSource::Unknown,
            },
        }
    }

    /// Whether this is an official build: from CI, at a known commit, with a
    /// clean tree.
    ///
    /// **All three, and every unknown counts against.** A result from an
    /// unofficial build is still worth having; it is not worth *pooling* with
    /// official ones without being able to tell them apart, because a defect
    /// found later can only be traced through a build that can name its source.
    #[must_use]
    pub fn is_official(self) -> bool {
        self.source == BuildSource::Ci && self.commit.is_some() && self.dirty == Some(false)
    }
}

impl fmt::Display for BuildIdentity {
    /// Renders a taint marker unless the build is official, in the same shape
    /// the fingerprint uses, so the two read alike wherever they appear
    /// together.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_official() {
            write!(f, "!!UNOFFICIAL!! ")?;
        }
        write!(f, "v{}", self.crate_version)?;
        match self.commit {
            Some(commit) => write!(f, " {commit}")?,
            None => write!(f, " commit-unknown")?,
        }
        match self.dirty {
            Some(true) => write!(f, " DIRTY")?,
            Some(false) => {}
            None => write!(f, " dirty-unknown")?,
        }
        write!(f, " [{}]", self.source)
    }
}

/// An environment stamp that was not set renders as empty, which means "not
/// determined" rather than "the empty string".
fn non_empty(value: &'static str) -> Option<&'static str> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests;
