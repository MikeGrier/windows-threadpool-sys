// Copyright (c) 2026 Mike Grier
//! Data-driven scenario stress suite (M9).
//!
//! The model and harness ([`Operation`], [`Scenario`], [`Fleet`],
//! [`run_scenario`], ...) live in
//! [`windows_file_watcher::scenario`](windows_file_watcher::scenario), shared
//! with the `run-scenario` binary (M9.5) -- this file only builds the
//! scenario library and drives it through `#[test]`s. See CHECKLIST.md M9 for
//! the full milestone history.
#![cfg(windows)]

use std::path::PathBuf;
use std::time::Duration;

use windows_file_watcher::scenario::{
    DEFAULT_SEED, HarnessParams, Operation, Rng, Scenario, env_u64, run_scenario,
    run_scenario_keep_dir, seed,
};

/// Whether the stress tests below should actually run: they exercise a live
/// `Monitor`, so they are gated the same way as [`stress.rs`](stress.rs)
/// rather than running on every plain `cargo test`.
fn stress_enabled() -> bool {
    std::env::var_os("WINDOWS_FILE_WATCHER_STRESS").is_some()
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
    // Bounds above Windows's ~23ms scheduling floor (D-73): a (1, 40) range
    // would round every draw up to the same one tick, silently degrading
    // "irregular" timing into a fixed delay.
    let scenario = delete_wait_reintroduce_scenario(
        rounds,
        Duration::from_millis(25),
        Duration::from_millis(250),
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
        .then(Operation::OpenSession {
            name: "a".to_string(),
        })
        .then(Operation::Subscribe {
            session: "a".to_string(),
            watch: "a-root".to_string(),
            path: PathBuf::new(),
            subtree: true,
        })
        .then(Operation::CancelWatch {
            watch: "a-root".to_string(),
        })
        .then(Operation::CloseSession {
            name: "a".to_string(),
        });
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
/// back-to-back-churn counterpart reuses the same generator below.
fn session_watch_churn_with_delays_scenario(
    rounds: u64,
    low: Duration,
    high: Duration,
) -> Scenario {
    Scenario::new("session-watch-churn-with-delays").then_repeated(
        rounds,
        vec![
            Operation::OpenSession {
                name: "churn".to_string(),
            },
            Operation::WaitRandom { low, high },
            Operation::Subscribe {
                session: "churn".to_string(),
                watch: "churn-watch".to_string(),
                path: PathBuf::new(),
                subtree: true,
            },
            Operation::WaitRandom { low, high },
            Operation::CreateFile {
                path: PathBuf::from("touched.txt"),
            },
            Operation::WaitRandom { low, high },
            Operation::CancelWatch {
                watch: "churn-watch".to_string(),
            },
            Operation::WaitRandom { low, high },
            Operation::CloseSession {
                name: "churn".to_string(),
            },
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
    // Bounds above Windows's ~23ms scheduling floor (D-73); see the
    // back-to-back variant below for the intentionally near-zero posture.
    let scenario = session_watch_churn_with_delays_scenario(
        rounds,
        Duration::from_millis(25),
        Duration::from_millis(250),
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
        .then(Operation::OpenSession {
            name: "second".to_string(),
        })
        .then(Operation::Subscribe {
            session: "second".to_string(),
            watch: "second-watch".to_string(),
            path: PathBuf::new(),
            subtree: true,
        })
        .then(Operation::CreateFile {
            path: PathBuf::from("seen-by-both.txt"),
        })
        .then(Operation::CancelWatch {
            watch: "second-watch".to_string(),
        })
        .then(Operation::CloseSession {
            name: "second".to_string(),
        });
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches >= 2,
        "expected the initial watch and the second session's watch to each report the same file create, got {} batches",
        outcome.batches
    );
}

// ---------------------------------------------------------------------------
// M9+: concurrent modifiers, spoilers, nesting, and queue overwhelm. All four
// share one prerequisite -- `Fleet` moving behind a `Mutex` so
// `Operation::Concurrent`'s spawned threads can share it -- so they land
// together rather than as four artificially separated commits.
// ---------------------------------------------------------------------------

#[test]
fn concurrent_branches_run_at_the_same_time_and_are_all_accounted_for() {
    if !stress_enabled() {
        return;
    }
    let scenario = Scenario::new("concurrent-branches").then(Operation::Concurrent {
        branches: vec![
            vec![
                Operation::CreateFile {
                    path: PathBuf::from("a.txt"),
                },
                Operation::CreateFile {
                    path: PathBuf::from("b.txt"),
                },
            ],
            vec![
                Operation::CreateFile {
                    path: PathBuf::from("c.txt"),
                },
                Operation::CreateFile {
                    path: PathBuf::from("d.txt"),
                },
            ],
        ],
    });
    let outcome = run_scenario(&scenario, seed(), &HarnessParams::default());
    assert!(
        outcome.batches > 0,
        "expected at least one batch from two concurrent branches"
    );
}

#[test]
fn a_held_open_file_blocks_a_concurrent_delete_with_a_real_sharing_violation() {
    if !stress_enabled() {
        return;
    }
    let scenario = Scenario::new("spoiler-blocks-delete").then(Operation::CreateFile {
        path: PathBuf::from("spoiled.txt"),
    });
    let (_outcome, dir) = run_scenario_keep_dir(&scenario, seed(), &HarnessParams::default());
    let target = dir.path().join("spoiled.txt");

    // A fixed sleep before attempting the delete only *probably* wins the race
    // against the hold's own open call -- on a loaded runner the delete branch
    // can go first and this fixture would then fail nondeterministically
    // despite correct sharing behavior. A real handshake removes the guess: the
    // delete thread blocks on a condition variable that this thread signals
    // only once the handle is genuinely open, so the delete is never attempted
    // before there is something to be blocked by.
    let opened = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let deleter = {
        let target = target.clone();
        let opened = std::sync::Arc::clone(&opened);
        std::thread::spawn(move || {
            let (lock, ready) = &*opened;
            let mut guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            while !*guard {
                guard = ready
                    .wait(guard)
                    .unwrap_or_else(|poison| poison.into_inner());
            }
            drop(guard);
            std::fs::remove_file(&target)
        })
    };

    // The same non-share-delete open `Operation::HoldOpen` uses (M9+.2), just
    // held open by this thread directly so it can control exactly when the
    // deleter is released.
    use std::os::windows::fs::OpenOptionsExt;
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001 | 0x0000_0002) // FILE_SHARE_READ | FILE_SHARE_WRITE, no DELETE
        .open(&target)
        .expect("open the file to hold");
    {
        let (lock, ready) = &*opened;
        *lock.lock().unwrap_or_else(|poison| poison.into_inner()) = true;
        ready.notify_one();
    }
    // A real window for the now-unblocked delete thread to actually attempt
    // (and fail) the removal before the handle closes.
    std::thread::sleep(Duration::from_millis(100));
    drop(handle);

    let delete_result = deleter.join().expect("the delete thread panicked");
    assert!(
        delete_result.is_err(),
        "the concurrent delete succeeded while the handle was still open"
    );
    assert!(
        target.exists(),
        "the spoiler should have blocked the concurrent delete"
    );

    dir.cleanup();
}

#[test]
fn a_deliberately_tiny_queue_bound_never_wedges_under_overwhelming_load() {
    if !stress_enabled() {
        return;
    }
    let scenario = Scenario::new("queue-overwhelm")
        .then(Operation::OpenSessionBounded {
            name: "tiny".to_string(),
            bound: 2,
        })
        .then(Operation::Subscribe {
            session: "tiny".to_string(),
            watch: "tiny-watch".to_string(),
            path: PathBuf::new(),
            subtree: true,
        })
        .then_repeated(
            5_000,
            vec![Operation::CreateFile {
                path: PathBuf::from("overwhelmed.txt"),
            }],
        )
        .then(Operation::CancelWatch {
            watch: "tiny-watch".to_string(),
        })
        .then(Operation::CloseSession {
            name: "tiny".to_string(),
        });
    let params = HarnessParams::for_operation_count(scenario.operation_count());
    // A bound this small, hit this hard, is expected to force backpressure
    // (D-11/D-29) -- the point of this test is that the harness's own
    // deadline assertion inside `run_scenario` never trips, not that zero
    // desyncs occur.
    let outcome = run_scenario(&scenario, seed(), &params);
    assert!(
        outcome.batches > 0,
        "expected at least one batch despite the tiny queue bound"
    );
}

// ---------------------------------------------------------------------------
// M9.5: the scenario library above is also persisted as JSON fixtures under
// `tests/scenarios/`. This test is the generic, data-driven runner: it does
// not know about any particular scenario, only how to load and execute one.
// ---------------------------------------------------------------------------

#[test]
fn every_persisted_json_fixture_runs_through_the_harness() {
    if !stress_enabled() {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    let mut ran = 0;
    for entry in std::fs::read_dir(&dir).expect("read tests/scenarios") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let scenario: Scenario = serde_json::from_str(&contents)
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
        let params = HarnessParams::for_operation_count(scenario.operation_count());
        let outcome = run_scenario(&scenario, seed(), &params);
        assert!(
            outcome.batches > 0,
            "fixture {} produced no batches",
            path.display()
        );
        ran += 1;
    }
    assert!(ran > 0, "no JSON fixtures found under {}", dir.display());
}
