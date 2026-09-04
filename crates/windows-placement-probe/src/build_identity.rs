// Copyright (c) 2026 Mike Grier
//! Which build produced a measurement.

use std::fmt;

/// Where a binary came from.
///
/// Ordered by how well a build can be traced to the source that made it,
/// `Unknown < Local < Ci`, so the derived `Ord` is that order -- the same shape
/// as `windows_topology_sys::Provenance` one layer down, and for the same
/// reason.
///
/// **Traceability, not trustworthiness.** `Ci` means an artifact that names the
/// commit it was built from; it does not mean the binary is honest, and this
/// enum cannot establish that -- the value is read from an environment variable
/// at build time, so anyone building this crate can set it. Ordering these by
/// "trust" would claim an authentication property that no variant here has.
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
    ///
    /// **This is what the build was told about itself, not something it can
    /// prove.** The value comes from an environment variable read at build
    /// time, so it distinguishes an *accidental* local build from a CI one --
    /// which is what it is for -- and does not authenticate a binary someone
    /// else handed you. See the crate README, "Checking what a binary is": the
    /// release asset's SHA-256 digest is what ties a download to what this
    /// repository published.
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
    /// Renders the unofficial variants in capitals and the CI one in lower
    /// case, so a build that cannot name where it came from is visibly louder
    /// than one that can.
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
    ///
    /// **On a local build this can report a tree cleaner than it was**, and the
    /// limit is disclosed rather than hidden. The answer is taken by a build
    /// script, and cargo re-runs that script only when something it declared an
    /// interest in changes; an uncommitted edit in another crate of the
    /// workspace rebuilds this one without re-running it. A CI build is
    /// unaffected, because there the answer comes from the environment and the
    /// checkout is clean by construction -- and a local build is already marked
    /// `!!UNOFFICIAL!!`, which is the stronger caveat.
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
