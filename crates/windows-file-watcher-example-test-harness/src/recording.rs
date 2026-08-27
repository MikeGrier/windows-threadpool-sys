// Copyright (c) 2026 Mike Grier
//! Capturing a pathology as a replayable JSON artifact.
//!
//! A [`Recording`] is what turns a pathology found once into a deterministic
//! regression: the seed and the schedule it produced, plus the outcome that run
//! reported. Loading it back and re-running its schedule reproduces that
//! outcome exactly, because the schedule -- not the seed -- is what the driver
//! actually replays; the seed is kept only as provenance (crate DESIGN-NOTES
//! D-5/D-7: schedules, not seeds, are the unit of reproduction, since a
//! hand-authored or mutated schedule has no seed at all).
//!
//! The JSON schema is a tool I/O format, not a data contract (DESIGN-NOTES D-4):
//! it may change shape in any release.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Outcome, Schedule};

/// A captured schedule, its provenance, and the outcome it produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recording {
    /// The seed that generated [`Recording::schedule`], for provenance --
    /// re-generating from it is not guaranteed to reproduce anything by itself
    /// (see the module docs); replay [`Recording::schedule`] instead.
    pub seed: u64,
    /// The schedule that was run.
    pub schedule: Schedule,
    /// What running it produced.
    pub outcome: Outcome,
}

impl Recording {
    /// Bundle a schedule, its seed, and the outcome it produced.
    #[must_use]
    pub fn new(seed: u64, schedule: Schedule, outcome: Outcome) -> Self {
        Self {
            seed,
            schedule,
            outcome,
        }
    }

    /// Serialize to pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails (it should not, for this type).
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize from JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if `json` is not a valid `Recording`.
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    /// Write as JSON to `path`, creating or overwriting it.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the write fails.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let json = self
            .to_json()
            .map_err(|error| io::Error::other(error.to_string()))?;
        fs::write(path, json)
    }

    /// Read and deserialize a `Recording` from `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the read or the deserialization fails.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let json = fs::read_to_string(path)?;
        Self::from_json(&json).map_err(|error| io::Error::other(error.to_string()))
    }
}
