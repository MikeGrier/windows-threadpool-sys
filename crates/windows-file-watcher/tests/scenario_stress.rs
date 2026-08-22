// Copyright (c) 2026 Mike Grier
//! Data-driven scenario stress suite (M9).
//!
//! A scenario is *data* -- an ordered [`Operation`] sequence plus timing
//! parameters -- not a hardcoded test function. The [`run_scenario`] harness
//! executes any scenario and checks it against the same generic invariants; a
//! new scenario is added by describing one, not by writing new test-body
//! logic (see CHECKLIST.md M9). This file defines the data model, its seeded
//! randomness (M9.1), and the execution harness (M9.2); M9.3 adds the basic
//! scenario library and M9.4 wires it into an opt-in test run.
//!
//! Stress runs are expected to describe **hundreds of thousands of
//! operations**. Two consequences follow throughout this file: [`Operation`]
//! has a [`Operation::Repeat`] combinator so a scenario stays a small value
//! instead of materializing every repetition, and [`run_scenario`] tracks
//! only bounded tallies rather than collecting every observed notification.
#![cfg(windows)]
// M9.1/M9.2 define the model and harness in full; M9.3 is what consumes every
// variant and calls `run_scenario` from a real `#[test]`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_file_watcher::{Monitor, Notification, Receiver, WatchOptions};

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
    /// Execute `pattern` in order, `count` times. A scenario describing
    /// hundreds of thousands of operations stays a handful of bytes by
    /// nesting this rather than unrolling every repetition into the `Vec`.
    Repeat { count: u64, pattern: Vec<Operation> },
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

    /// Append `pattern` `count` times as a single [`Operation::Repeat`],
    /// without ever materializing the expansion.
    fn then_repeated(mut self, count: u64, pattern: Vec<Operation>) -> Self {
        self.operations.push(Operation::Repeat { count, pattern });
        self
    }

    /// The number of concrete filesystem actions this scenario describes,
    /// counting through every [`Operation::Repeat`] -- for diagnostics and
    /// tests, not evaluated on the hot path.
    fn operation_count(&self) -> u64 {
        fn count(operations: &[Operation]) -> u64 {
            operations
                .iter()
                .map(|operation| match operation {
                    Operation::Repeat { count: n, pattern } => n * count(pattern),
                    _ => 1,
                })
                .sum()
        }
        count(&self.operations)
    }
}

/// A minimal self-cleaning temp directory, matching the one in
/// [`stress.rs`](stress.rs) -- each integration-test binary is compiled and
/// linked separately, so there is no shared support module to draw this from.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-file-watcher-scenario-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Parameters the harness itself needs, independent of any scenario's
/// content.
struct HarnessParams {
    /// Overall wall-clock budget for applying every operation and draining
    /// whatever the watch reports afterward. Exceeding it means the scenario
    /// wedged, not that it merely ran long -- callers describing hundreds of
    /// thousands of operations should raise this accordingly.
    timeout: Duration,
    /// How long the queue must stay silent after the last operation before
    /// the harness considers the scenario settled.
    quiet_period: Duration,
}

impl Default for HarnessParams {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            quiet_period: Duration::from_millis(300),
        }
    }
}

/// Bounded tallies from one [`run_scenario`] call. Deliberately **not** a
/// `Vec<Notification>`: a run describing hundreds of thousands of operations
/// can produce a comparable number of notifications, and this harness's own
/// generic invariants (D-12: a desync is a reported loss, never silence) only
/// need counts, not the full history.
#[derive(Debug, Default)]
struct HarnessOutcome {
    batches: u64,
    changes: u64,
    desyncs: u64,
    suspensions: u64,
    resumptions: u64,
    establishments: u64,
    completions: u64,
    retry_questions: u64,
}

impl HarnessOutcome {
    fn record(&mut self, notification: &Notification) {
        match notification {
            Notification::Batch { changes, .. } => {
                self.batches += 1;
                self.changes += changes.len() as u64;
            }
            Notification::Desync { .. } => self.desyncs += 1,
            Notification::Suspended { .. } => self.suspensions += 1,
            Notification::Resumed { .. } => self.resumptions += 1,
            Notification::Established { .. } => self.establishments += 1,
            Notification::Completion { .. } => self.completions += 1,
            Notification::RetryQuestion { .. } => self.retry_questions += 1,
        }
    }
}

/// Drain whatever is already queued without blocking, so a long operation
/// loop never lets the crate's bounded queue back up (D-11) between the
/// non-blocking checks a scenario with hundreds of thousands of operations
/// relies on.
fn drain_available(receiver: &Receiver, outcome: &mut HarnessOutcome) {
    while let Some(notification) = receiver.try_recv() {
        outcome.record(&notification);
    }
}

/// Applies one [`Operation`] (recursively expanding [`Operation::Repeat`])
/// against `root`, drawing any `WaitRandom` duration from `rng`. Individual
/// filesystem calls are best-effort: `Remove*`/`Rename` are allowed to fail
/// (a scenario may target a path that a prior step already removed, and the
/// M9+ "spoiler" work will make failure routine), while `Create*` failures
/// abort the run -- a scenario that cannot even establish its own inputs is a
/// broken scenario, not interesting fault behavior.
fn apply_operation(root: &Path, operation: &Operation, rng: &mut Rng) {
    match operation {
        Operation::CreateFile { path } => {
            let target = root.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("create parent directory");
            }
            std::fs::write(&target, b"x").expect("create file");
        }
        Operation::CreateDir { path } => {
            std::fs::create_dir_all(root.join(path)).expect("create directory");
        }
        Operation::RemoveFile { path } => {
            let _ = std::fs::remove_file(root.join(path));
        }
        Operation::RemoveDir { path } => {
            let _ = std::fs::remove_dir_all(root.join(path));
        }
        Operation::Rename { from, to } => {
            let _ = std::fs::rename(root.join(from), root.join(to));
        }
        Operation::Wait { duration } => std::thread::sleep(*duration),
        Operation::WaitRandom { low, high } => {
            std::thread::sleep(rng.duration_range(*low, *high));
        }
        Operation::Repeat { count, pattern } => {
            for _ in 0..*count {
                for step in pattern {
                    apply_operation(root, step, rng);
                }
            }
        }
    }
}

/// Executes `scenario` against a real temp directory and a live
/// [`Monitor`]/[`Session`], checking only the invariants this harness itself
/// knows about: the run completes within `params.timeout` (a wedge, not a
/// slow pass, is the only failure this generic layer can detect), applying
/// operations never panics, and every notification is tallied so a desync is
/// always a *counted*, reported loss rather than silence (D-12). A scenario
/// carries no assertions of its own; the caller inspects the returned
/// [`HarnessOutcome`] for anything scenario-specific.
fn run_scenario(scenario: &Scenario, seed: u64, params: &HarnessParams) -> HarnessOutcome {
    let dir = TempDir::new(scenario.label);
    let monitor = Monitor::new().expect("create the monitor");
    let (session, receiver) = monitor.session();
    let watch = session
        .subscribe(dir.path(), WatchOptions::new().subtree(true))
        .expect("register");

    let deadline = Instant::now() + params.timeout;
    let mut rng = Rng::new(seed);
    let mut outcome = HarnessOutcome::default();

    for operation in &scenario.operations {
        apply_operation(dir.path(), operation, &mut rng);
        drain_available(&receiver, &mut outcome);
        assert!(
            Instant::now() < deadline,
            "scenario '{}' wedged applying its operations",
            scenario.label
        );
    }

    // Keep draining until the queue stays quiet for `quiet_period`, so
    // anything still in flight when the last operation returned is still
    // counted -- bounded by the same overall deadline, so a genuine wedge
    // here still fails rather than hanging the run.
    while let Some(notification) = receiver.recv_timeout(params.quiet_period) {
        outcome.record(&notification);
        assert!(
            Instant::now() < deadline,
            "scenario '{}' wedged draining its notifications",
            scenario.label
        );
    }

    drop(watch);
    drop(monitor);
    dir.cleanup();
    outcome
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

#[test]
fn repeat_counts_hundreds_of_thousands_of_operations_without_materializing_them() {
    let scenario = Scenario::new("repeat-count").then_repeated(
        250_000,
        vec![Operation::CreateFile {
            path: PathBuf::from("f.txt"),
        }],
    );
    // The `Vec` itself holds exactly one `Repeat` entry; only the logical
    // count, not the allocation, scales with the repetition.
    assert_eq!(scenario.operations.len(), 1);
    assert_eq!(scenario.operation_count(), 250_000);
}

#[test]
fn a_small_scenario_runs_through_the_harness_and_is_fully_accounted_for() {
    if !stress_enabled() {
        return;
    }
    let scenario = Scenario::new("harness-smoke")
        .then(Operation::CreateFile {
            path: PathBuf::from("a.txt"),
        })
        .then(Operation::CreateFile {
            path: PathBuf::from("b.txt"),
        })
        .then(Operation::Rename {
            from: PathBuf::from("a.txt"),
            to: PathBuf::from("c.txt"),
        })
        .then(Operation::RemoveFile {
            path: PathBuf::from("b.txt"),
        });

    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from {} operations",
        scenario.operation_count()
    );
}

/// Whether the harness smoke test above should actually run: it exercises a
/// live `Monitor`, so it is gated the same way as [`stress.rs`](stress.rs)
/// rather than running on every plain `cargo test`.
fn stress_enabled() -> bool {
    std::env::var_os("WINDOWS_FILE_WATCHER_STRESS").is_some()
}
