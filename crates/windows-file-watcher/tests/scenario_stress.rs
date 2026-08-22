// Copyright (c) 2026 Mike Grier
//! Data-driven scenario stress suite (M9).
//!
//! A scenario is *data* -- an ordered [`Operation`] sequence plus timing
//! parameters -- not a hardcoded test function. The [`run_scenario`] harness
//! executes any scenario and checks it against the same generic invariants; a
//! new scenario is added by describing one, not by writing new test-body
//! logic (see CHECKLIST.md M9). This file defines the data model, its seeded
//! randomness (M9.1), and the execution harness (M9.2); M9.3 adds the basic
//! scenario library, M9.4 adds session/watch lifecycle operations (sessions
//! and watches opening and closing mid-scenario, not just the filesystem
//! underneath a single fixed watch), and M9.5 wires the complete set into an
//! opt-in test run.
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_file_watcher::{Monitor, Notification, Receiver, Session, Watch, WatchOptions};

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
    /// Open a new session, named for later operations to reference (M9.4).
    /// `Monitor::session` mints an independent channel per call (D-2), so
    /// every open session has its own receiver the harness must drain.
    /// Opening a name that is already open is a scenario-authoring bug.
    OpenSession { name: &'static str },
    /// Close a previously opened session: every watch still registered
    /// through it is cancelled first (in arbitrary order), then the session
    /// itself is dropped. Closing a name that is not open is a
    /// scenario-authoring bug.
    CloseSession { name: &'static str },
    /// Subscribe a watch through an already-open, named session, at `path`
    /// (relative to the scenario root), naming the watch for later
    /// reference. Re-using a watch name that already exists is a
    /// scenario-authoring bug.
    Subscribe {
        session: &'static str,
        watch: &'static str,
        path: PathBuf,
        subtree: bool,
    },
    /// Cancel (drop) a previously subscribed watch by name. Cancelling a
    /// name that does not exist is a scenario-authoring bug.
    CancelWatch { watch: &'static str },
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

impl HarnessParams {
    /// A timeout scaled for `operation_count` concrete filesystem actions,
    /// for a scenario large enough that the default 120s budget is a budget
    /// for *applying the operations themselves* (each a real syscall), not
    /// evidence of a watcher wedge. 500 ops/sec is a deliberately
    /// conservative floor -- well under the ~1,800 ops/sec `std::fs::write`
    /// churn measured on development hardware -- plus a fixed 60s allowance
    /// for the watch to settle afterward.
    fn for_operation_count(operation_count: u64) -> Self {
        Self {
            timeout: Duration::from_secs(operation_count / 500 + 60).max(Self::default().timeout),
            ..Self::default()
        }
    }
}

/// The name reserved for the one session/watch every scenario gets for free
/// (matching M9.1-M9.3's original "a watch on the scenario root" behavior).
/// `Operation::OpenSession`/`Subscribe` reject reusing it, so a scenario
/// cannot silently shadow the implicit watch.
const INITIAL_SESSION: &str = "__initial_session__";
const INITIAL_WATCH: &str = "__initial_watch__";

/// The live sessions and watches a scenario has opened so far, keyed by the
/// name the scenario gave them (M9.4). `Monitor::session` mints an
/// independent channel per call (D-2), so tracking "the current watch" as a
/// single value (M9.1-M9.3's model) stops working once a scenario can open a
/// second session -- this is a name-keyed table instead, and every session's
/// receiver is drained on every pass, not just one.
struct Fleet<'m> {
    monitor: &'m Monitor,
    sessions: HashMap<&'static str, (Session, Receiver)>,
    watches: HashMap<&'static str, (&'static str, Watch)>,
}

impl<'m> Fleet<'m> {
    fn new(monitor: &'m Monitor) -> Self {
        Self {
            monitor,
            sessions: HashMap::new(),
            watches: HashMap::new(),
        }
    }

    fn open_session(&mut self, name: &'static str) {
        assert!(
            !self.sessions.contains_key(name),
            "scenario bug: session '{name}' is already open"
        );
        let (session, receiver) = self.monitor.session();
        self.sessions.insert(name, (session, receiver));
    }

    /// Cancels every watch still registered through `name`, then drops the
    /// session itself.
    fn close_session(&mut self, name: &'static str) {
        let orphaned: Vec<&'static str> = self
            .watches
            .iter()
            .filter(|(_, (owner, _))| *owner == name)
            .map(|(watch_name, _)| *watch_name)
            .collect();
        for watch_name in orphaned {
            self.watches.remove(watch_name); // Drop cancels (D-5).
        }
        self.sessions
            .remove(name)
            .unwrap_or_else(|| panic!("scenario bug: session '{name}' is not open"));
    }

    fn subscribe(
        &mut self,
        session_name: &'static str,
        watch_name: &'static str,
        path: &Path,
        subtree: bool,
    ) {
        assert!(
            !self.watches.contains_key(watch_name),
            "scenario bug: watch '{watch_name}' already exists"
        );
        let (session, _) = self
            .sessions
            .get(session_name)
            .unwrap_or_else(|| panic!("scenario bug: session '{session_name}' is not open"));
        let watch = session
            .subscribe(path, WatchOptions::new().subtree(subtree))
            .expect("subscribe");
        self.watches.insert(watch_name, (session_name, watch));
    }

    fn cancel_watch(&mut self, watch_name: &'static str) {
        let removed = self
            .watches
            .remove(watch_name)
            .unwrap_or_else(|| panic!("scenario bug: watch '{watch_name}' does not exist"));
        drop(removed); // Drop cancels (D-5).
    }

    /// Drain whatever is already queued on every open session's receiver,
    /// without blocking, so a long operation loop never lets the crate's
    /// bounded queue back up (D-11) between the non-blocking checks a
    /// scenario with hundreds of thousands of operations relies on.
    fn drain_available(&self, outcome: &mut HarnessOutcome) {
        for (_, receiver) in self.sessions.values() {
            while let Some(notification) = receiver.try_recv() {
                outcome.record(&notification);
            }
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

    /// A single number that changes whenever any tally does -- used only to
    /// detect "did anything arrive during this poll", not as a meaningful
    /// count on its own.
    fn total(&self) -> u64 {
        self.batches
            + self.changes
            + self.desyncs
            + self.suspensions
            + self.resumptions
            + self.establishments
            + self.completions
            + self.retry_questions
    }
}

/// Applies one [`Operation`] (recursively expanding [`Operation::Repeat`])
/// against `root`/`fleet`, drawing any `WaitRandom` duration from `rng`.
/// Individual filesystem calls are best-effort: `Remove*`/`Rename` are
/// allowed to fail (a scenario may target a path that a prior step already
/// removed, and the M9+ "spoiler" work will make failure routine), while
/// `Create*` failures abort the run -- a scenario that cannot even establish
/// its own inputs is a broken scenario, not interesting fault behavior.
/// Session/watch lifecycle operations (M9.4) instead assert on misuse (an
/// unknown or already-open/closed name): that is a scenario-authoring bug,
/// never a fault the harness tolerates.
fn apply_operation(root: &Path, fleet: &mut Fleet<'_>, operation: &Operation, rng: &mut Rng) {
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
                    apply_operation(root, fleet, step, rng);
                }
            }
        }
        Operation::OpenSession { name } => fleet.open_session(name),
        Operation::CloseSession { name } => fleet.close_session(name),
        Operation::Subscribe {
            session,
            watch,
            path,
            subtree,
        } => fleet.subscribe(session, watch, &root.join(path), *subtree),
        Operation::CancelWatch { watch } => fleet.cancel_watch(watch),
    }
}

/// Executes `scenario` against a real temp directory and a live
/// [`Monitor`], checking only the invariants this harness itself knows
/// about: the run completes within `params.timeout` (a wedge, not a slow
/// pass, is the only failure this generic layer can detect), applying
/// operations never panics, and every notification -- across every session
/// the scenario opens (M9.4) -- is tallied so a desync is always a
/// *counted*, reported loss rather than silence (D-12). A scenario carries
/// no assertions of its own; the caller inspects the returned
/// [`HarnessOutcome`] for anything scenario-specific. The temp directory is
/// removed before this returns; use [`run_scenario_keep_dir`] when a
/// scenario-specific check also needs the real end-state on disk.
fn run_scenario(scenario: &Scenario, seed: u64, params: &HarnessParams) -> HarnessOutcome {
    let (outcome, dir) = run_scenario_keep_dir(scenario, seed, params);
    dir.cleanup();
    outcome
}

/// Same as [`run_scenario`], but returns the [`TempDir`] instead of cleaning
/// it up, for a scenario-specific check that needs to inspect the real
/// filesystem end state -- e.g. confirming two racing renames both actually
/// landed, independent of what the notification stream reported.
fn run_scenario_keep_dir(
    scenario: &Scenario,
    seed: u64,
    params: &HarnessParams,
) -> (HarnessOutcome, TempDir) {
    let dir = TempDir::new(scenario.label);
    let monitor = Monitor::new().expect("create the monitor");
    let mut fleet = Fleet::new(&monitor);
    fleet.open_session(INITIAL_SESSION);
    fleet.subscribe(INITIAL_SESSION, INITIAL_WATCH, dir.path(), true);

    let deadline = Instant::now() + params.timeout;
    let mut rng = Rng::new(seed);
    let mut outcome = HarnessOutcome::default();

    for operation in &scenario.operations {
        apply_operation(dir.path(), &mut fleet, operation, &mut rng);
        fleet.drain_available(&mut outcome);
        assert!(
            Instant::now() < deadline,
            "scenario '{}' wedged applying its operations",
            scenario.label
        );
    }

    // Keep draining every open session until none of them yield anything for
    // a full `quiet_period`, so anything still in flight when the last
    // operation returned is still counted -- bounded by the same overall
    // deadline, so a genuine wedge here still fails rather than hanging the
    // run.
    let poll_interval = Duration::from_millis(10).min(params.quiet_period);
    let mut last_activity = Instant::now();
    while last_activity.elapsed() < params.quiet_period {
        let before = outcome.total();
        fleet.drain_available(&mut outcome);
        if outcome.total() != before {
            last_activity = Instant::now();
        } else {
            std::thread::sleep(poll_interval);
        }
        assert!(
            Instant::now() < deadline,
            "scenario '{}' wedged draining its notifications",
            scenario.label
        );
    }

    drop(fleet);
    drop(monitor);
    (outcome, dir)
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

/// Reads a `u64` scenario-scale parameter from an environment variable,
/// falling back to `default` -- the mechanism every M9.3 scenario below uses
/// to stay parameterizable (entity counts, round counts) without a code
/// change.
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// M9.3: the basic scenario library. Each function below returns a `Scenario`
// -- a value -- built from the M9.1 model; none of them contains
// scenario-specific test logic. The `#[test]` that follows each one only
// runs it through the M9.2 harness and checks whatever that particular
// scenario needs beyond the harness's own generic invariants.
// ---------------------------------------------------------------------------

/// (a) A handful of files, hit with a burst of changes -- scaled with
/// [`Operation::Repeat`] so the same small scenario value can describe
/// anywhere from a smoke-test handful up to the hundreds of thousands of
/// operations a real stress run is expected to exercise, purely by raising
/// `WINDOWS_FILE_WATCHER_SCENARIO_CHURN_FILES` / `_TOUCHES`.
fn churn_scenario(files: u64, touches_per_file: u64) -> Scenario {
    let mut scenario = Scenario::new("churn");
    for index in 0..files {
        let path = PathBuf::from(format!("churn-{index}.txt"));
        scenario = scenario.then_repeated(
            touches_per_file,
            vec![Operation::CreateFile { path: path.clone() }],
        );
    }
    scenario
}

/// (b) Delete, wait an irregular amount of time, and reintroduce the same
/// file, `rounds` times -- the wait between delete and recreate (and again
/// before the next round) is PRNG-drawn from `[low, high]`, not fixed, so a
/// harness run at a different seed exercises different timing without any
/// code change.
fn delete_wait_reintroduce_scenario(rounds: u64, low: Duration, high: Duration) -> Scenario {
    let path = PathBuf::from("marker.txt");
    Scenario::new("delete-wait-reintroduce")
        .then(Operation::CreateFile { path: path.clone() })
        .then_repeated(
            rounds,
            vec![
                Operation::RemoveFile { path: path.clone() },
                Operation::WaitRandom { low, high },
                Operation::CreateFile { path: path.clone() },
                Operation::WaitRandom { low, high },
            ],
        )
}

/// (c) Plain renames: create `count` files, then rename every one of them.
fn rename_scenario(count: u64) -> Scenario {
    let mut scenario = Scenario::new("rename");
    for index in 0..count {
        scenario = scenario.then(Operation::CreateFile {
            path: PathBuf::from(format!("before-{index}.txt")),
        });
    }
    for index in 0..count {
        scenario = scenario.then(Operation::Rename {
            from: PathBuf::from(format!("before-{index}.txt")),
            to: PathBuf::from(format!("after-{index}.txt")),
        });
    }
    scenario
}

/// (d) Cross-type name reuse: a file named `thing` is removed and a
/// *directory* takes the same name, then that directory is removed and a
/// *file* takes the name back -- probing whether the watcher confuses a
/// path's identity with the path string alone.
fn cross_type_name_reuse_scenario() -> Scenario {
    let path = PathBuf::from("thing");
    Scenario::new("cross-type-name-reuse")
        .then(Operation::CreateFile { path: path.clone() })
        .then(Operation::RemoveFile { path: path.clone() })
        .then(Operation::CreateDir { path: path.clone() })
        .then(Operation::RemoveDir { path: path.clone() })
        .then(Operation::CreateFile { path })
}

/// (e) A fast two-entity swap: file `x` is renamed to `y` immediately
/// followed (no `Wait` between them) by directory `z` being renamed to `x`,
/// so the watcher's decode path sees `x` vacated and reoccupied back to
/// back. Real *concurrent* (multi-thread) racing is M9+.1; this scenario
/// probes the single-threaded back-to-back case that M9 covers today.
fn fast_two_entity_swap_scenario() -> Scenario {
    Scenario::new("fast-two-entity-swap")
        .then(Operation::CreateFile {
            path: PathBuf::from("x"),
        })
        .then(Operation::CreateDir {
            path: PathBuf::from("z"),
        })
        .then(Operation::Rename {
            from: PathBuf::from("x"),
            to: PathBuf::from("y"),
        })
        .then(Operation::Rename {
            from: PathBuf::from("z"),
            to: PathBuf::from("x"),
        })
}

#[test]
fn a_burst_of_churn_across_a_few_files_is_observed() {
    if !stress_enabled() {
        return;
    }
    let files = env_u64("WINDOWS_FILE_WATCHER_SCENARIO_CHURN_FILES", 5);
    let touches = env_u64("WINDOWS_FILE_WATCHER_SCENARIO_CHURN_TOUCHES", 50_000);
    let scenario = churn_scenario(files, touches);
    let params = HarnessParams::for_operation_count(scenario.operation_count());
    let outcome = run_scenario(&scenario, seed(), &params);
    assert!(
        outcome.batches > 0,
        "expected at least one batch from {} operations",
        scenario.operation_count()
    );
}

#[test]
fn delete_wait_reintroduce_survives_irregular_timing() {
    if !stress_enabled() {
        return;
    }
    let rounds = env_u64("WINDOWS_FILE_WATCHER_SCENARIO_REINTRODUCE_ROUNDS", 25);
    let scenario = delete_wait_reintroduce_scenario(
        rounds,
        Duration::from_millis(1),
        Duration::from_millis(40),
    );
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from {rounds} delete/reintroduce rounds"
    );
}

#[test]
fn plain_renames_are_observed() {
    if !stress_enabled() {
        return;
    }
    let count = env_u64("WINDOWS_FILE_WATCHER_SCENARIO_RENAME_COUNT", 500);
    let scenario = rename_scenario(count);
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from {count} renames"
    );
}

#[test]
fn a_directory_can_reuse_a_removed_files_name_and_back() {
    if !stress_enabled() {
        return;
    }
    let scenario = cross_type_name_reuse_scenario();
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from the cross-type name-reuse sequence"
    );
}

#[test]
fn a_fast_two_entity_swap_leaves_the_filesystem_in_the_expected_end_state() {
    if !stress_enabled() {
        return;
    }
    let scenario = fast_two_entity_swap_scenario();
    let (outcome, dir) = run_scenario_keep_dir(&scenario, seed(), &HarnessParams::default());

    // Whatever the watcher reported, the OS itself must have applied both
    // renames without the second clobbering the first: the real end state is
    // what actually answers "did the swap confuse anything" independent of
    // the notification stream.
    assert!(dir.path().join("y").is_file(), "x -> y did not land");
    assert!(dir.path().join("x").is_dir(), "z -> x did not land");
    assert!(!dir.path().join("z").exists(), "z should no longer exist");
    assert!(
        outcome.batches > 0,
        "expected at least one batch from the swap"
    );

    dir.cleanup();
}

// ---------------------------------------------------------------------------
// M9.4: session/watch lifecycle operations. A scenario can now open/close
// named sessions and subscribe/cancel named watches mid-run, not just churn
// the filesystem underneath one fixed watch (M9.1-M9.3).
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_operations_are_built_as_plain_named_steps() {
    let scenario = Scenario::new("lifecycle-smoke")
        .then(Operation::OpenSession { name: "a" })
        .then(Operation::Subscribe {
            session: "a",
            watch: "a-root",
            path: PathBuf::new(),
            subtree: true,
        })
        .then(Operation::CancelWatch { watch: "a-root" })
        .then(Operation::CloseSession { name: "a" });
    assert_eq!(scenario.operations.len(), 4);
}

/// A session/watch churns `rounds` times: open a session, wait an irregular
/// amount before subscribing (mirroring a real client that does not dial in
/// the instant it starts), touch a file so the new watch has something to
/// see, wait again before cancelling the watch, and wait once more before
/// closing the session -- every transition separated by a PRNG-drawn delay
/// rather than back-to-back churn. Every round reuses the same names, since
/// the prior round's `CloseSession` frees them.
///
/// Real stress runs need both timing postures, not just this one: hammering
/// continuously finds throughput bugs, but a fault or a race is often a
/// *timing-window* problem, which only shows up when transitions are spaced
/// out enough to land while other activity (the filesystem churn here) is
/// mid-flight. This scenario is the "spaced out" posture; a tighter,
/// back-to-back-churn counterpart belongs in M9.5's fuller library.
fn session_watch_churn_with_delays_scenario(
    rounds: u64,
    low: Duration,
    high: Duration,
) -> Scenario {
    Scenario::new("session-watch-churn-with-delays").then_repeated(
        rounds,
        vec![
            Operation::OpenSession { name: "churn" },
            Operation::WaitRandom { low, high },
            Operation::Subscribe {
                session: "churn",
                watch: "churn-watch",
                path: PathBuf::new(),
                subtree: true,
            },
            Operation::WaitRandom { low, high },
            Operation::CreateFile {
                path: PathBuf::from("touched.txt"),
            },
            Operation::WaitRandom { low, high },
            Operation::CancelWatch {
                watch: "churn-watch",
            },
            Operation::WaitRandom { low, high },
            Operation::CloseSession { name: "churn" },
            Operation::WaitRandom { low, high },
        ],
    )
}

#[test]
fn sessions_and_watches_enter_and_exit_with_delays_between_transitions() {
    if !stress_enabled() {
        return;
    }
    let rounds = env_u64("WINDOWS_FILE_WATCHER_SCENARIO_LIFECYCLE_ROUNDS", 25);
    let scenario = session_watch_churn_with_delays_scenario(
        rounds,
        Duration::from_millis(1),
        Duration::from_millis(30),
    );
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from {rounds} session/watch churn rounds"
    );
}

/// The same churn, but with no delay between transitions at all -- the
/// "hit it hard, continuously" posture, kept alongside the delayed one above
/// so both timing postures are exercised, not just the spaced-out one.
#[test]
fn sessions_and_watches_enter_and_exit_back_to_back() {
    if !stress_enabled() {
        return;
    }
    let rounds = env_u64("WINDOWS_FILE_WATCHER_SCENARIO_LIFECYCLE_ROUNDS", 25);
    let scenario =
        session_watch_churn_with_delays_scenario(rounds, Duration::ZERO, Duration::from_micros(1));
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from {rounds} back-to-back session/watch churn rounds"
    );
}

#[test]
fn multiple_sessions_can_be_open_at_once_and_each_is_drained() {
    if !stress_enabled() {
        return;
    }
    let scenario = Scenario::new("two-sessions")
        .then(Operation::OpenSession { name: "second" })
        .then(Operation::Subscribe {
            session: "second",
            watch: "second-watch",
            path: PathBuf::new(),
            subtree: true,
        })
        .then(Operation::CreateFile {
            path: PathBuf::from("seen-by-both.txt"),
        })
        .then(Operation::CancelWatch {
            watch: "second-watch",
        })
        .then(Operation::CloseSession { name: "second" });
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches >= 2,
        "expected the initial watch and the second session's watch to each report the same file create, got {} batches",
        outcome.batches
    );
}
