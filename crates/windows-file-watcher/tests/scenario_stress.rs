// Copyright (c) 2026 Mike Grier
//! Data-driven scenario stress suite (M9).
//!
//! A scenario is *data* -- an ordered [`Operation`] sequence plus timing
//! parameters -- not a hardcoded test function. A single shared harness
//! (added in M9.2) executes any scenario and checks it against the same
//! generic invariants; a new scenario is added by describing one, not by
//! writing new test-body logic (see CHECKLIST.md M9). This file currently
//! defines only the data model and its seeded randomness (M9.1); M9.2 adds
//! the execution harness and M9.3 the scenario library.
#![cfg(windows)]
// M9.1 defines the model in full; M9.2/M9.3 are what consume every variant.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

/// A tiny deterministic PRNG (splitmix64's step function), used only to draw
/// wait durations and other scenario choice points. Reproducible by default
/// (D-66): a fixed seed unless overridden, never unseeded/unrepeatable
/// randomness.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // splitmix64 rejects a zero state silently degrading to zero output;
        // folding in the golden-ratio constant keeps `seed == 0` well-behaved.
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform integer in `[low, high]` inclusive.
    fn range(&mut self, low: u64, high: u64) -> u64 {
        assert!(low <= high, "empty range: {low}..={high}");
        let span = high - low + 1;
        low + self.next_u64() % span
    }

    /// A uniform [`Duration`] in `[low, high]` inclusive, at microsecond
    /// resolution.
    fn duration_range(&mut self, low: Duration, high: Duration) -> Duration {
        let lo = low.as_micros() as u64;
        let hi = high.as_micros() as u64;
        Duration::from_micros(self.range(lo, hi))
    }
}

/// The default seed, kept fixed so every default run of this suite is
/// identical run to run (D-66, and the repo's no-random-sampling-without-
/// approval rule). Override with `WINDOWS_FILE_WATCHER_STRESS_SEED` (parsed
/// as `u64`) to explore a different sequence on demand.
const DEFAULT_SEED: u64 = 0x5EED_F17E_1234_5678;

fn seed() -> u64 {
    std::env::var("WINDOWS_FILE_WATCHER_STRESS_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

/// One data-driven filesystem action a [`Scenario`] asks the harness to
/// perform. Paths are relative to the scenario's temp root.
#[derive(Clone, Debug)]
enum Operation {
    /// Create (or overwrite) a file at `path` with a few bytes of content.
    CreateFile { path: PathBuf },
    /// Create a directory at `path`.
    CreateDir { path: PathBuf },
    /// Remove a file at `path`.
    RemoveFile { path: PathBuf },
    /// Remove a directory, and its contents, at `path`.
    RemoveDir { path: PathBuf },
    /// Rename `from` to `to` (whichever exists: file or directory).
    Rename { from: PathBuf, to: PathBuf },
    /// Sleep for a fixed duration before the next operation.
    Wait { duration: Duration },
    /// Sleep for an irregular, PRNG-drawn duration in `[low, high]` before
    /// the next operation -- resolved by the harness's own seeded `Rng` at
    /// execution time, not precomputed, so the same scenario value can be
    /// replayed at any seed.
    WaitRandom { low: Duration, high: Duration },
}

/// A named, ordered sequence of [`Operation`]s. The harness executes a
/// scenario mechanically against only the generic invariants it knows about;
/// a scenario carries no assertions of its own.
#[derive(Clone, Debug)]
struct Scenario {
    label: &'static str,
    operations: Vec<Operation>,
}

impl Scenario {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            operations: Vec::new(),
        }
    }

    fn then(mut self, operation: Operation) -> Self {
        self.operations.push(operation);
        self
    }
}

#[test]
fn the_prng_is_deterministic_for_a_fixed_seed() {
    let mut a = Rng::new(DEFAULT_SEED);
    let mut b = Rng::new(DEFAULT_SEED);
    for _ in 0..1_000 {
        assert_eq!(
            a.next_u64(),
            b.next_u64(),
            "same seed must replay identically"
        );
    }
}

#[test]
fn range_stays_within_bounds_and_seed_env_var_parses() {
    let mut rng = Rng::new(seed());
    for _ in 0..1_000 {
        let value = rng.range(3, 7);
        assert!((3..=7).contains(&value), "{value} out of [3, 7]");
    }
    for _ in 0..1_000 {
        let duration = rng.duration_range(Duration::from_millis(1), Duration::from_millis(50));
        assert!(
            duration >= Duration::from_millis(1) && duration <= Duration::from_millis(50),
            "{duration:?} out of [1ms, 50ms]"
        );
    }
}

#[test]
fn a_scenario_is_built_as_a_plain_ordered_operation_list() {
    let scenario = Scenario::new("smoke")
        .then(Operation::CreateFile {
            path: PathBuf::from("a.txt"),
        })
        .then(Operation::Wait {
            duration: Duration::from_millis(1),
        })
        .then(Operation::Rename {
            from: PathBuf::from("a.txt"),
            to: PathBuf::from("b.txt"),
        });
    assert_eq!(scenario.label, "smoke");
    assert_eq!(scenario.operations.len(), 3);
}
