// Copyright (c) 2026 Mike Grier
//! Data-driven scenario stress model and harness (M9), shared by the
//! `run-scenario` binary and the `scenario_stress` integration test.
//!
//! A scenario is *data* -- a [`Scenario`] value naming an ordered
//! [`Operation`] sequence -- not a hardcoded test function. [`run_scenario`]
//! executes any scenario and checks it against the same generic invariants
//! (no wedge, no panic, every desync counted -- D-12); a new scenario is
//! added by describing one, not by writing new test-body logic. See
//! `CHECKLIST.md` M9 in the crate's repository for the full milestone
//! history.
//!
//! # The JSON schema is not part of this crate's semver contract
//!
//! [`Operation`] and [`Scenario`] derive [`serde::Serialize`]/
//! [`serde::Deserialize`] (M9.5) so a scenario can be persisted as JSON and
//! handed to the `run-scenario` binary as a file path. That JSON schema is a
//! **testing/ops tool input**, not a documented data format this crate
//! promises to keep reading forever: its shape may change, gain fields, or
//! rename fields in any release, including a patch release, without that
//! being treated as a breaking change. Only this module's own Rust API
//! surface (types, function signatures) is covered by the crate's normal
//! semver guarantees, exactly as for any other `pub` item.
//!
//! Stress runs are expected to describe **hundreds of thousands of
//! operations**. Two consequences follow throughout this module:
//! [`Operation::Repeat`] lets a scenario stay a small value instead of
//! materializing every repetition, and [`run_scenario`] tracks only bounded
//! tallies ([`HarnessOutcome`]) rather than collecting every observed
//! notification.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::{Monitor, Notification, Receiver, Session, Watch, WatchOptions};

/// A tiny deterministic PRNG (splitmix64's step function), used only to draw
/// wait durations and other scenario choice points. Reproducible by default
/// (D-66): a fixed seed unless overridden, never unseeded/unrepeatable
/// randomness.
pub struct Rng(u64);

impl Rng {
    /// Creates a generator seeded from `seed`.
    pub fn new(seed: u64) -> Self {
        // splitmix64 rejects a zero state silently degrading to zero output;
        // folding in the golden-ratio constant keeps `seed == 0` well-behaved.
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// The next pseudo-random `u64` in the sequence.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform integer in `[low, high]` inclusive.
    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        assert!(low <= high, "empty range: {low}..={high}");
        // `high - low + 1` overflows exactly when the requested range is the
        // full width of `u64` (`low == 0 && high == u64::MAX`) -- the only
        // case in which `wrapping_add` yields 0, since `high - low` itself
        // cannot overflow given the assertion above. Every `u64` is in range
        // then, so the raw output already is one; no modulus is needed (a
        // literal span of `2^64` cannot be represented, and `% 0` would
        // panic regardless of build profile).
        let span = (high - low).wrapping_add(1);
        if span == 0 {
            return self.next_u64();
        }
        // Plain `% span` is biased toward the low end whenever `span` does not
        // evenly divide `2^64` -- rejecting draws past the last full multiple
        // of `span` (`limit`) restores a uniform distribution. Expected extra
        // draws are bounded (worst case span just over `u64::MAX / 2`, still
        // under one retry on average).
        let limit = u64::MAX - (u64::MAX % span);
        loop {
            let draw = self.next_u64();
            if draw < limit {
                return low + draw % span;
            }
        }
    }

    /// A uniform [`Duration`] in `[low, high]` inclusive, at microsecond
    /// resolution.
    pub fn duration_range(&mut self, low: Duration, high: Duration) -> Duration {
        // Saturating rather than truncating: a `Duration` beyond `u64::MAX`
        // microseconds (~584,942 years) is already absurd for a scenario
        // bound, but truncating it via `as u64` would wrap to something far
        // *smaller* -- silently defeating the caller's timeout/backpressure
        // intent rather than merely clamping it.
        let lo = u64::try_from(low.as_micros()).unwrap_or(u64::MAX);
        let hi = u64::try_from(high.as_micros()).unwrap_or(u64::MAX);
        Duration::from_micros(self.range(lo, hi))
    }
}

/// The default seed, kept fixed so every default run of this suite is
/// identical run to run (D-66, and the repo's no-random-sampling-without-
/// approval rule). Override with `WINDOWS_FILE_WATCHER_STRESS_SEED` (parsed
/// as `u64`) to explore a different sequence on demand.
pub const DEFAULT_SEED: u64 = 0x5EED_F17E_1234_5678;

/// Reads the PRNG seed from `WINDOWS_FILE_WATCHER_STRESS_SEED`, falling back
/// to [`DEFAULT_SEED`].
pub fn seed() -> u64 {
    std::env::var("WINDOWS_FILE_WATCHER_STRESS_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

/// Reads a `u64` scenario-scale parameter from an environment variable,
/// falling back to `default` -- the mechanism a scenario library stays
/// parameterizable (entity counts, round counts) without a code change.
pub fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// A [`Duration`] as JSON milliseconds, so [`Operation`]'s `Wait`/`WaitRandom`
/// fields round-trip through the persisted schema (which has no native
/// duration type) as plain numbers.
mod millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub(super) fn serialize<S: Serializer>(
        duration: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        // A checked conversion rather than `as u64`: silently truncating a
        // duration that does not fit would persist a *different*, shorter
        // duration than the one the caller actually has, rather than failing
        // loudly on input this schema cannot represent.
        u64::try_from(duration.as_millis())
            .map_err(|_| {
                serde::ser::Error::custom(
                    "duration exceeds u64::MAX milliseconds and cannot be persisted as JSON",
                )
            })?
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(deserializer)?))
    }
}

/// One data-driven filesystem or session/watch-lifecycle action a
/// [`Scenario`] asks the harness to perform. Paths are relative to the
/// scenario's temp root.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    /// Create (or overwrite) a file at `path` with a few bytes of content.
    CreateFile {
        /// The file's path, relative to the scenario root.
        path: PathBuf,
    },
    /// Create a directory at `path`.
    CreateDir {
        /// The directory's path, relative to the scenario root.
        path: PathBuf,
    },
    /// Remove a file at `path`.
    RemoveFile {
        /// The file's path, relative to the scenario root.
        path: PathBuf,
    },
    /// Remove a directory, and its contents, at `path`.
    RemoveDir {
        /// The directory's path, relative to the scenario root.
        path: PathBuf,
    },
    /// Rename `from` to `to` (whichever exists: file or directory).
    Rename {
        /// The current path, relative to the scenario root.
        from: PathBuf,
        /// The destination path, relative to the scenario root.
        to: PathBuf,
    },
    /// Sleep for a fixed duration before the next operation.
    Wait {
        /// How long to sleep, at millisecond resolution in JSON.
        #[serde(with = "millis")]
        duration: Duration,
    },
    /// Sleep for an irregular, PRNG-drawn duration in `[low, high]` before
    /// the next operation -- resolved by the harness's own seeded [`Rng`] at
    /// execution time, not precomputed, so the same scenario value can be
    /// replayed at any seed.
    ///
    /// **Choose bounds above Windows's scheduling floor (D-73).**
    /// `std::thread::sleep` cannot sleep for less than the OS scheduling
    /// quantum -- commonly ~15.6ms, though this crate's own stress runs
    /// measure an effective floor closer to ~23ms -- so a 1ms request and a
    /// 20ms request both round up to the same one tick. Bounds entirely
    /// below that floor silently collapse "spaced out" timing into
    /// back-to-back timing without the scenario's author noticing; prefer
    /// something like `(25, 250)` over `(1, 20)` when the intent is genuinely
    /// irregular delays.
    WaitRandom {
        /// The inclusive lower bound, at millisecond resolution in JSON.
        #[serde(with = "millis")]
        low: Duration,
        /// The inclusive upper bound, at millisecond resolution in JSON.
        #[serde(with = "millis")]
        high: Duration,
    },
    /// Execute `pattern` in order, `count` times. A scenario describing
    /// hundreds of thousands of operations stays a handful of bytes by
    /// nesting this rather than unrolling every repetition into the `Vec`.
    Repeat {
        /// How many times to run `pattern`.
        count: u64,
        /// The operations to repeat, in order.
        pattern: Vec<Operation>,
    },
    /// Open a new session, named for later operations to reference (M9.4).
    /// `Monitor::session` mints an independent channel per call (D-2), so
    /// every open session has its own receiver the harness must drain.
    /// Opening a name that is already open is a scenario-authoring bug.
    OpenSession {
        /// The name this session is known by for the rest of the scenario.
        name: String,
    },
    /// Close a previously opened session: every watch still registered
    /// through it is cancelled first (in arbitrary order), then the session
    /// itself is dropped. Closing a name that is not open is a
    /// scenario-authoring bug.
    CloseSession {
        /// The session's name, as given to a prior `OpenSession`.
        name: String,
    },
    /// Subscribe a watch through an already-open, named session, at `path`
    /// (relative to the scenario root), naming the watch for later
    /// reference. Re-using a watch name that already exists is a
    /// scenario-authoring bug.
    Subscribe {
        /// The owning session's name, as given to a prior `OpenSession`.
        session: String,
        /// The name this watch is known by for the rest of the scenario.
        watch: String,
        /// The watched path, relative to the scenario root.
        path: PathBuf,
        /// Whether the watch covers the subtree below `path`.
        subtree: bool,
    },
    /// Cancel (drop) a previously subscribed watch by name. Cancelling a
    /// name that does not exist is a scenario-authoring bug.
    CancelWatch {
        /// The watch's name, as given to a prior `Subscribe`.
        watch: String,
    },
    /// Open a new session named `name`, like `OpenSession`, but with an
    /// explicit queue `bound` instead of the crate's default (M9+.4) --
    /// small enough to deliberately overwhelm under load, so a scenario can
    /// exercise the documented backpressure behavior (a full queue stops the
    /// producer rather than dropping data, D-11/D-29) instead of the
    /// crate's ordinary, much larger capacity.
    OpenSessionBounded {
        /// The name this session is known by for the rest of the scenario.
        name: String,
        /// The queue bound; must be nonzero.
        bound: usize,
    },
    /// Open (or create) the file at `path` without `FILE_SHARE_DELETE`, hold
    /// it open for `duration`, then close it (M9+.2) -- a *real* Win32
    /// sharing violation for a concurrent `Rename`/`RemoveFile`/`RemoveDir`
    /// targeting the same path, a genuine spoiler rather than a simulated
    /// one. `path` must already exist. Typically placed in one branch of a
    /// `Concurrent` alongside the operation it is meant to block.
    HoldOpen {
        /// The file's path, relative to the scenario root. Must exist.
        path: PathBuf,
        /// How long to hold the handle open, at millisecond resolution in
        /// JSON.
        #[serde(with = "millis")]
        duration: Duration,
        /// A named [`Operation::Barrier`] to rendezvous on the instant the
        /// handle is open, before sleeping (PR #20 review response): a
        /// concurrent antagonist naming the same barrier is then guaranteed
        /// to run *after* the handle is genuinely open, rather than guessing
        /// a fixed delay is enough of a head start. `#[serde(default)]` so
        /// existing persisted scenarios that predate this field still parse.
        #[serde(default)]
        ready_barrier: Option<String>,
    },
    /// Rendezvous with exactly one other operation naming the same `name`
    /// (PR #20 review response): blocks until both have reached this point.
    /// The deterministic way to sequence a `Concurrent` branch against
    /// another -- for example, waiting for [`Operation::HoldOpen`]'s
    /// `ready_barrier` -- instead of guessing a fixed delay is enough of a
    /// head start. Each name must be used by exactly two operations across
    /// the whole scenario (including a `HoldOpen.ready_barrier` using the
    /// same name); a third use blocks forever waiting for a rendezvous that
    /// already happened, which the harness's own deadline eventually reports
    /// as a wedge rather than hanging silently.
    Barrier {
        /// The rendezvous point's name, shared with exactly one other
        /// operation.
        name: String,
    },
    /// Run every operation list in `branches` concurrently, each on its own
    /// thread, waiting for all branches to finish before the next top-level
    /// operation runs (M9+.1). This is the model's only concurrency
    /// primitive; nesting (M9+.3) falls out for free, since a branch is
    /// itself an ordinary operation list that may contain another
    /// `Concurrent`, a `Repeat`, or anything else in this enum.
    Concurrent {
        /// Independent operation sequences to run at the same time.
        branches: Vec<Vec<Operation>>,
    },
}

/// A named, ordered sequence of [`Operation`]s. The harness executes a
/// scenario mechanically against only the generic invariants it knows about;
/// a scenario carries no assertions of its own.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    /// A short, human-readable name; used only for temp-directory naming and
    /// diagnostics, never interpreted.
    pub label: String,
    /// The operations to perform, in order.
    pub operations: Vec<Operation>,
}

impl Scenario {
    /// Creates an empty scenario named `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            operations: Vec::new(),
        }
    }

    /// Appends one operation, returning `self` for chaining.
    #[must_use]
    pub fn then(mut self, operation: Operation) -> Self {
        self.operations.push(operation);
        self
    }

    /// Appends `pattern` `count` times as a single [`Operation::Repeat`],
    /// without ever materializing the expansion.
    #[must_use]
    pub fn then_repeated(mut self, count: u64, pattern: Vec<Operation>) -> Self {
        self.operations.push(Operation::Repeat { count, pattern });
        self
    }

    /// The number of concrete actions this scenario describes, counting
    /// through every [`Operation::Repeat`] and every [`Operation::Concurrent`]
    /// branch -- for diagnostics and tests, not evaluated on the hot path.
    /// Saturates rather than overflows, since a persisted scenario's `Repeat`
    /// counts are untrusted input.
    pub fn operation_count(&self) -> u64 {
        fn count(operations: &[Operation]) -> u64 {
            operations
                .iter()
                .map(|operation| match operation {
                    Operation::Repeat { count: n, pattern } => n.saturating_mul(count(pattern)),
                    Operation::Concurrent { branches } => branches
                        .iter()
                        .map(|branch| count(branch))
                        .fold(0u64, |total, branch_count| {
                            total.saturating_add(branch_count)
                        }),
                    _ => 1,
                })
                .fold(0u64, |total, one| total.saturating_add(one))
        }
        count(&self.operations)
    }
}

/// A minimal self-cleaning temp directory.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a fresh, empty temp directory, its name incorporating `label`.
    ///
    /// `label` is sanitized before it becomes part of a path component: a
    /// scenario's label is untrusted input (persisted JSON, M9.5), and a
    /// label containing a path separator or a `..` component would otherwise
    /// let `temp_dir().join(...)` normalize outside the system temp
    /// directory -- after which [`TempDir::cleanup`] would recursively
    /// remove whatever ended up there instead. The unsanitized label is
    /// never lost: it stays on [`Scenario::label`] for diagnostics.
    pub fn new(label: &str) -> Self {
        let sanitized: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-file-watcher-scenario-{sanitized}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temp dir");
        Self { path }
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the directory and everything in it.
    pub fn cleanup(self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Parameters the harness itself needs, independent of any scenario's
/// content.
pub struct HarnessParams {
    /// Overall wall-clock budget for applying every operation and draining
    /// whatever the watch reports afterward. Exceeding it means the scenario
    /// wedged, not that it merely ran long -- callers describing hundreds of
    /// thousands of operations should raise this accordingly.
    pub timeout: Duration,
    /// How long the queue must stay silent after the last operation before
    /// the harness considers the scenario settled.
    pub quiet_period: Duration,
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
    /// A timeout scaled for `operation_count` concrete actions, for a
    /// scenario large enough that the default 120s budget is a budget for
    /// *applying the operations themselves* (each a real syscall), not
    /// evidence of a watcher wedge (D-68). 500 ops/sec is a deliberately
    /// conservative floor -- well under the ~1,800 ops/sec `std::fs::write`
    /// churn measured on development hardware -- plus a fixed 60s allowance
    /// for the watch to settle afterward.
    pub fn for_operation_count(operation_count: u64) -> Self {
        Self {
            timeout: Duration::from_secs(operation_count / 500 + 60).max(Self::default().timeout),
            ..Self::default()
        }
    }
}

/// A two-party rendezvous point that gives up at a deadline instead of
/// blocking forever (PR #20 review response).
///
/// `std::sync::Barrier::wait` has no timeout, so a malformed scenario --
/// naming a barrier once with no partner, reusing one across a mismatched
/// number of `Repeat` iterations on each side, or (subtler still) giving both
/// uses to the same sequentially-executing thread instead of two genuinely
/// concurrent branches -- would block that thread forever. Because that
/// thread is inside the `std::thread::scope` an `Operation::Concurrent`
/// spawned it from, the harness's own deadline check (run between top-level
/// operations on the caller's thread) never gets a chance to run either, so
/// the malformed scenario wedges the whole runner rather than failing loudly.
/// Bounding every wait against the same `deadline` `apply_operation` already
/// threads through everything else turns every one of those structural
/// mistakes into an ordinary "wedged" panic instead, without needing to
/// prove the two participants are concurrent ahead of time.
struct DeadlineBarrier {
    /// How many parties have arrived for the round currently forming. Reset
    /// to `0` the instant it reaches `2`, so the same barrier can be reused
    /// for a later round (e.g. a later `Repeat` iteration) without needing a
    /// fresh object.
    arrived: Mutex<usize>,
    ready: std::sync::Condvar,
}

impl DeadlineBarrier {
    fn new() -> Self {
        Self {
            arrived: Mutex::new(0),
            ready: std::sync::Condvar::new(),
        }
    }

    /// Blocks until a second party calls `wait` on this same barrier, or
    /// panics once `deadline` passes without one arriving.
    fn wait(&self, deadline: Instant, label: &str) {
        let mut arrived = self
            .arrived
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *arrived += 1;
        if *arrived >= 2 {
            *arrived = 0;
            self.ready.notify_all();
            return;
        }
        loop {
            let now = Instant::now();
            assert!(
                now < deadline,
                "a Barrier operation named '{label}' wedged: no second participant \
                 arrived before the harness's overall deadline -- check that the name \
                 is used by exactly two operations on two genuinely concurrent branches"
            );
            let (guard, timeout) = self
                .ready
                .wait_timeout(arrived, deadline - now)
                .unwrap_or_else(|poison| poison.into_inner());
            arrived = guard;
            if *arrived == 0 {
                // The second party arrived, reset the round, and notified.
                return;
            }
            if timeout.timed_out() {
                continue; // Re-check against `deadline` directly above.
            }
        }
    }
}

/// The name reserved for the one session/watch every scenario gets for free
/// (matching M9.1-M9.3's original "a watch on the scenario root" behavior).
/// `Operation::OpenSession`/`Subscribe` reject reusing it, so a scenario
/// cannot silently shadow the implicit watch.
pub const INITIAL_SESSION: &str = "__initial_session__";
/// See [`INITIAL_SESSION`].
pub const INITIAL_WATCH: &str = "__initial_watch__";

/// The live sessions and watches a scenario has opened so far, keyed by the
/// name the scenario gave them (M9.4). `Monitor::session` mints an
/// independent channel per call (D-2), so tracking "the current watch" as a
/// single value (M9.1-M9.3's model) stops working once a scenario can open a
/// second session -- this is a name-keyed table instead (D-69), and every
/// session's receiver is drained on every pass, not just one.
pub struct Fleet<'m> {
    monitor: &'m Monitor,
    sessions: HashMap<String, (Session, Receiver)>,
    watches: HashMap<String, (String, Watch)>,
    /// Named two-party rendezvous points (PR #20 review response), shared
    /// between [`Operation::HoldOpen`]'s `ready_barrier` and
    /// [`Operation::Barrier`], created lazily so a scenario never has to
    /// declare its barriers up front.
    barriers: HashMap<String, Arc<DeadlineBarrier>>,
}

impl<'m> Fleet<'m> {
    /// Creates an empty fleet against `monitor`.
    pub fn new(monitor: &'m Monitor) -> Self {
        Self {
            monitor,
            sessions: HashMap::new(),
            watches: HashMap::new(),
            barriers: HashMap::new(),
        }
    }

    /// The two-party barrier named `name`, creating it on its first use.
    ///
    /// Returns the `Arc` rather than waiting on it directly: a caller must
    /// drop this fleet's lock before calling `DeadlineBarrier::wait` on the
    /// result, or a concurrent branch that also needs the fleet (to reach its
    /// own side of the same rendezvous, or for an unrelated session/watch
    /// operation) would deadlock against this one.
    fn barrier(&mut self, name: &str) -> Arc<DeadlineBarrier> {
        Arc::clone(
            self.barriers
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(DeadlineBarrier::new())),
        )
    }

    /// Opens a new session named `name`. Panics if `name` is already open.
    pub fn open_session(&mut self, name: &str) {
        assert!(
            !self.sessions.contains_key(name),
            "scenario bug: session '{name}' is already open"
        );
        let (session, receiver) = self.monitor.session();
        self.sessions.insert(name.to_string(), (session, receiver));
    }

    /// Like [`Self::open_session`], but with an explicit queue `bound`
    /// (M9+.4) instead of the crate's default. Panics if `name` is already
    /// open or `bound` is zero.
    pub fn open_session_bounded(&mut self, name: &str, bound: usize) {
        assert!(
            !self.sessions.contains_key(name),
            "scenario bug: session '{name}' is already open"
        );
        let bound = std::num::NonZeroUsize::new(bound)
            .unwrap_or_else(|| panic!("scenario bug: bound must be nonzero"));
        let (session, receiver) = self.monitor.session_with_bound(bound);
        self.sessions.insert(name.to_string(), (session, receiver));
    }

    /// Cancels every watch still registered through `name`, then drops the
    /// session itself. Panics if `name` is not open.
    pub fn close_session(&mut self, name: &str) {
        let orphaned: Vec<String> = self
            .watches
            .iter()
            .filter(|(_, (owner, _))| owner == name)
            .map(|(watch_name, _)| watch_name.clone())
            .collect();
        for watch_name in orphaned {
            self.watches.remove(&watch_name); // Drop cancels (D-5).
        }
        self.sessions
            .remove(name)
            .unwrap_or_else(|| panic!("scenario bug: session '{name}' is not open"));
    }

    /// Subscribes a watch named `watch_name` through the already-open session
    /// `session_name`, at `path`. Panics if `session_name` is not open or
    /// `watch_name` already exists.
    pub fn subscribe(&mut self, session_name: &str, watch_name: &str, path: &Path, subtree: bool) {
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
        self.watches
            .insert(watch_name.to_string(), (session_name.to_string(), watch));
    }

    /// Cancels (drops) the watch named `watch_name`. Panics if it does not
    /// exist.
    pub fn cancel_watch(&mut self, watch_name: &str) {
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
    pub fn drain_available(&self, outcome: &mut HarnessOutcome) {
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
pub struct HarnessOutcome {
    /// How many [`Notification::Batch`]es arrived.
    pub batches: u64,
    /// The total number of changes across every batch.
    pub changes: u64,
    /// How many [`Notification::Desync`]s arrived.
    pub desyncs: u64,
    /// How many [`Notification::Suspended`]s arrived.
    pub suspensions: u64,
    /// How many [`Notification::Resumed`]s arrived.
    pub resumptions: u64,
    /// How many [`Notification::Established`]s arrived.
    pub establishments: u64,
    /// How many [`Notification::Completion`]s arrived.
    pub completions: u64,
    /// How many [`Notification::RetryQuestion`]s arrived.
    pub retry_questions: u64,
    /// How many [`Notification::VolumeChanged`]s arrived.
    pub volume_changes: u64,
}

impl HarnessOutcome {
    /// Folds one notification into the tallies.
    pub fn record(&mut self, notification: &Notification) {
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
            Notification::VolumeChanged { .. } => self.volume_changes += 1,
        }
    }

    /// A single number that changes whenever any tally does -- used only to
    /// detect "did anything arrive during this poll", not as a meaningful
    /// count on its own.
    pub fn total(&self) -> u64 {
        self.batches
            + self.changes
            + self.desyncs
            + self.suspensions
            + self.resumptions
            + self.establishments
            + self.completions
            + self.retry_questions
            + self.volume_changes
    }
}

/// The two Win32 sharing flags a [`Operation::HoldOpen`] handle is opened
/// with -- `FILE_SHARE_READ | FILE_SHARE_WRITE`, deliberately omitting
/// `FILE_SHARE_DELETE` so a concurrent rename or delete of the same path
/// fails with a real sharing violation (M9+.2).
mod share_mode {
    pub(super) const READ_WRITE_NO_DELETE: u32 = 0x0000_0001 | 0x0000_0002;
}

/// Applies one [`Operation`] (recursively expanding [`Operation::Repeat`]/
/// [`Operation::Concurrent`]) against `root`/`fleet`, drawing any
/// `WaitRandom` duration from `rng`. Individual filesystem calls are
/// best-effort: `Remove*`/`Rename` are allowed to fail (a scenario may
/// target a path that a prior step already removed, or that an
/// `Operation::HoldOpen` spoiler is deliberately blocking -- M9+.2), while
/// `Create*`/`HoldOpen` failures abort the run -- a scenario that cannot even
/// establish its own inputs is a broken scenario, not interesting fault
/// behavior. Session/watch lifecycle operations (M9.4/M9+.4) instead assert
/// on misuse (an unknown or already-open/closed name): that is a
/// scenario-authoring bug, never a fault the harness tolerates.
///
/// `fleet` is a `Mutex` (not a plain `&mut`) so that [`Operation::Concurrent`]
/// (M9+.1) can share it across the threads it spawns for its branches; the
/// lock is held only for the brief Fleet-mutating operations
/// (`OpenSession`/`Subscribe`/... ), never around a filesystem call or sleep.
///
/// `deadline` bounds every operation that can itself block for a
/// scenario-specified duration -- `Wait`, `WaitRandom`, `HoldOpen`, and each
/// iteration of `Repeat` -- rather than only being checked by the caller
/// between top-level operations (`run_scenario_keep_dir`'s own loop), which a
/// single long `Wait`, a large `Repeat`, or a `Concurrent` branch could
/// otherwise block through, hanging well past `params.timeout` despite the
/// harness's bounded/wedge-detection contract. Exceeding it here panics with
/// the same "wedged" framing the caller's own checks use, which composes
/// correctly with `Concurrent`'s `thread::scope`: a panic in a spawned branch
/// is re-raised by `scope` once every branch has been joined.
pub fn apply_operation(
    root: &Path,
    fleet: &Mutex<Fleet<'_>>,
    operation: &Operation,
    rng: &mut Rng,
    deadline: Instant,
) {
    /// Panics if `duration` from now would run past `deadline`, instead of
    /// sleeping through it -- checked *before* the sleep so even a single
    /// oversized `Wait` is caught immediately rather than after it elapses.
    fn check_bounded_sleep(duration: Duration, deadline: Instant, label: &str) {
        assert!(
            Instant::now() + duration <= deadline,
            "a {label} operation's duration would exceed the harness's overall deadline"
        );
    }

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
        Operation::Wait { duration } => {
            check_bounded_sleep(*duration, deadline, "Wait");
            std::thread::sleep(*duration);
        }
        Operation::WaitRandom { low, high } => {
            let duration = rng.duration_range(*low, *high);
            check_bounded_sleep(duration, deadline, "WaitRandom");
            std::thread::sleep(duration);
        }
        Operation::Repeat { count, pattern } => {
            for _ in 0..*count {
                assert!(
                    Instant::now() < deadline,
                    "a Repeat operation wedged applying its pattern"
                );
                for step in pattern {
                    apply_operation(root, fleet, step, rng, deadline);
                }
            }
        }
        Operation::OpenSession { name } => fleet.lock().unwrap().open_session(name),
        Operation::OpenSessionBounded { name, bound } => {
            fleet.lock().unwrap().open_session_bounded(name, *bound)
        }
        Operation::CloseSession { name } => fleet.lock().unwrap().close_session(name),
        Operation::Subscribe {
            session,
            watch,
            path,
            subtree,
        } => fleet
            .lock()
            .unwrap()
            .subscribe(session, watch, &root.join(path), *subtree),
        Operation::CancelWatch { watch } => fleet.lock().unwrap().cancel_watch(watch),
        Operation::HoldOpen {
            path,
            duration,
            ready_barrier,
        } => {
            use std::os::windows::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share_mode::READ_WRITE_NO_DELETE)
                .open(root.join(path))
                .expect("open file to hold");
            if let Some(name) = ready_barrier {
                let barrier = fleet.lock().unwrap().barrier(name);
                barrier.wait(deadline, name);
            }
            check_bounded_sleep(*duration, deadline, "HoldOpen");
            std::thread::sleep(*duration);
            drop(file);
        }
        Operation::Barrier { name } => {
            let barrier = fleet.lock().unwrap().barrier(name);
            barrier.wait(deadline, name);
        }
        Operation::Concurrent { branches } => {
            // Draw each branch's seed on the calling thread, before
            // spawning, so the whole scenario stays reproducible for a
            // given top-level seed (D-66) regardless of how the OS
            // schedules the branches.
            let branch_seeds: Vec<u64> = branches.iter().map(|_| rng.next_u64()).collect();
            std::thread::scope(|scope| {
                for (branch, branch_seed) in branches.iter().zip(branch_seeds) {
                    scope.spawn(move || {
                        let mut branch_rng = Rng::new(branch_seed);
                        for step in branch {
                            apply_operation(root, fleet, step, &mut branch_rng, deadline);
                        }
                    });
                }
            });
        }
    }
}

/// Rejects a path that is not confined to the scenario root: absolute paths
/// (including a bare drive prefix like `C:`) and any `..` component are
/// refused. Every path-bearing [`Operation`] is joined directly onto the
/// scenario's real temp directory, and a persisted JSON scenario is
/// untrusted-by-default input to `run-scenario` -- without this check, an
/// absolute path would replace the root entirely and a `..` component would
/// escape it, letting a crafted scenario file make `RemoveDir` (or any other
/// operation) act on an arbitrary caller-accessible path.
fn validate_confined(path: &Path) -> Result<(), String> {
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                return Err(format!(
                    "path {path:?} is not relative to the scenario root"
                ));
            }
            std::path::Component::ParentDir => {
                return Err(format!("path {path:?} escapes the scenario root via '..'"));
            }
            std::path::Component::CurDir | std::path::Component::Normal(_) => {}
        }
    }
    Ok(())
}

/// Validates every path-bearing operation in `operations`, recursing through
/// [`Operation::Repeat`]'s pattern and every [`Operation::Concurrent`] branch,
/// so a path smuggled into a nested or concurrent operation is caught just as
/// surely as a top-level one.
fn validate_paths(operations: &[Operation]) -> Result<(), String> {
    for operation in operations {
        match operation {
            Operation::CreateFile { path }
            | Operation::CreateDir { path }
            | Operation::RemoveFile { path }
            | Operation::RemoveDir { path } => validate_confined(path)?,
            Operation::Rename { from, to } => {
                validate_confined(from)?;
                validate_confined(to)?;
            }
            Operation::Subscribe { path, .. } => validate_confined(path)?,
            Operation::HoldOpen { path, .. } => validate_confined(path)?,
            Operation::Wait { .. }
            | Operation::WaitRandom { .. }
            | Operation::OpenSession { .. }
            | Operation::OpenSessionBounded { .. }
            | Operation::CloseSession { .. }
            | Operation::Barrier { .. }
            | Operation::CancelWatch { .. } => {}
            Operation::Repeat { pattern, .. } => validate_paths(pattern)?,
            Operation::Concurrent { branches } => {
                for branch in branches {
                    validate_paths(branch)?;
                }
            }
        }
    }
    Ok(())
}

/// Counts every use of each named [`Operation::Barrier`] rendezvous point --
/// both a bare `Barrier` and a `HoldOpen.ready_barrier` count as one use of
/// that name -- recursing through `Repeat` and `Concurrent` exactly like
/// [`validate_paths`].
fn count_barrier_uses(operations: &[Operation], counts: &mut HashMap<String, u64>) {
    for operation in operations {
        match operation {
            Operation::Barrier { name } => *counts.entry(name.clone()).or_insert(0) += 1,
            Operation::HoldOpen {
                ready_barrier: Some(name),
                ..
            } => *counts.entry(name.clone()).or_insert(0) += 1,
            Operation::Repeat { pattern, .. } => count_barrier_uses(pattern, counts),
            Operation::Concurrent { branches } => {
                for branch in branches {
                    count_barrier_uses(branch, counts);
                }
            }
            _ => {}
        }
    }
}

/// Rejects a scenario whose named [`Operation::Barrier`] rendezvous points
/// are used an odd number of times (PR #20 review response). A cheap,
/// precise early rejection for the unambiguous mistake -- one use, or any
/// odd count, always leaves at least one participant with no partner --
/// checked up front, before anything runs, so that case fails with a clear
/// scenario-authoring-bug message immediately rather than only once
/// something times out.
///
/// An **even** count is deliberately accepted rather than requiring exactly
/// 2: [`DeadlineBarrier`] resets after each round, so the same name is
/// legitimately reusable for several independent sequential rendezvous pairs
/// (e.g. two unrelated `Barrier` operations later in the same scenario),
/// which a stricter "exactly 2" check rejected even though nothing about it
/// is malformed.
///
/// This is a **necessary but not sufficient** check: an even count does not
/// prove the uses can ever be paired into genuinely concurrent rendezvous
/// (two top-level `Barrier` operations, or two uses within the same `Repeat`
/// pattern, both pass this check yet only ever run on one thread,
/// sequentially, so the first would still wait forever for a partner that
/// can never arrive on the same thread). Proving genuine concurrency ahead of
/// time would mean tracking which `Concurrent` branch each nested
/// `Repeat`/`Concurrent` ultimately executes on -- so instead of attempting
/// that, [`DeadlineBarrier`] itself is bounded by the harness's own deadline:
/// a pairing that passes this count check but can never actually rendezvous
/// still fails, just later, as an ordinary "wedged" panic rather than a
/// permanent hang.
fn validate_barriers(operations: &[Operation]) -> Result<(), String> {
    let mut counts = HashMap::new();
    count_barrier_uses(operations, &mut counts);
    for (name, count) in counts {
        if count % 2 != 0 {
            return Err(format!(
                "barrier '{name}' is used {count} time(s), but each rendezvous round needs \
                 exactly 2 uses; an odd count always leaves at least one \
                 Barrier/HoldOpen.ready_barrier use with no partner, which would wait forever"
            ));
        }
    }
    Ok(())
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
pub fn run_scenario(scenario: &Scenario, seed: u64, params: &HarnessParams) -> HarnessOutcome {
    let (outcome, dir) = run_scenario_keep_dir(scenario, seed, params);
    dir.cleanup();
    outcome
}

/// Same as [`run_scenario`], but returns the [`TempDir`] instead of cleaning
/// it up, for a scenario-specific check that needs to inspect the real
/// filesystem end state -- e.g. confirming two racing renames both actually
/// landed, independent of what the notification stream reported.
///
/// Panics if any path-bearing operation (including nested inside a `Repeat`
/// or a `Concurrent` branch) is not confined to the scenario root -- every
/// path is rejected if absolute or containing a `..` component -- *before*
/// creating the temp directory or applying anything, so an unconfined path
/// never reaches a real filesystem call. Also panics upfront if any named
/// `Operation::Barrier` is used an odd number of times (see
/// `validate_barriers`); a barrier used an even number of times but never
/// actually reachable concurrently instead panics later, once
/// `DeadlineBarrier` gives up at this call's own deadline -- either way, a
/// malformed barrier fails loudly rather than hanging the runner.
pub fn run_scenario_keep_dir(
    scenario: &Scenario,
    seed: u64,
    params: &HarnessParams,
) -> (HarnessOutcome, TempDir) {
    if let Err(reason) = validate_paths(&scenario.operations) {
        panic!("scenario '{}' is unsafe to run: {reason}", scenario.label);
    }
    if let Err(reason) = validate_barriers(&scenario.operations) {
        panic!("scenario '{}' is unsafe to run: {reason}", scenario.label);
    }

    let dir = TempDir::new(&scenario.label);
    let monitor = Monitor::new().expect("create the monitor");
    let fleet = Mutex::new(Fleet::new(&monitor));
    fleet.lock().unwrap().open_session(INITIAL_SESSION);
    fleet
        .lock()
        .unwrap()
        .subscribe(INITIAL_SESSION, INITIAL_WATCH, dir.path(), true);

    let deadline = Instant::now() + params.timeout;
    let mut rng = Rng::new(seed);
    let mut outcome = HarnessOutcome::default();

    for operation in &scenario.operations {
        apply_operation(dir.path(), &fleet, operation, &mut rng, deadline);
        fleet.lock().unwrap().drain_available(&mut outcome);
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
        fleet.lock().unwrap().drain_available(&mut outcome);
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
