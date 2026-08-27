// Copyright (c) 2026 Mike Grier
//! M2.3: the generator is reproducible, and the schedules it produces respect
//! the legal-envelope rules the generator promises (crate DESIGN-NOTES D-5).

#![cfg(windows)]

use std::collections::BTreeMap;

use windows_file_watcher::ContractChecker;
use windows_file_watcher_example_test_harness::{
    DesyncCauseSpec, Generator, GeneratorConfig, NotificationSpec, OutcomeSpec, WatchModeSpec,
};

#[test]
fn a_fixed_seed_generates_an_identical_schedule() {
    let generator = Generator::new();
    let first = generator.generate(0x0ABC_D123_4567_89AB);
    let second = generator.generate(0x0ABC_D123_4567_89AB);
    assert_eq!(
        first, second,
        "the same seed must produce the same schedule"
    );
    assert!(!first.is_empty());
}

#[test]
fn different_seeds_produce_different_schedules() {
    let generator = Generator::new();
    assert_ne!(generator.generate(1), generator.generate(2));
}

/// Group a schedule's steps by watch, preserving each watch's relative order
/// (per-watch order is what carries sequencing meaning; cross-watch order is
/// free -- schedule docs).
fn by_watch(
    schedule: &windows_file_watcher_example_test_harness::Schedule,
) -> BTreeMap<u64, Vec<&NotificationSpec>> {
    let mut grouped: BTreeMap<u64, Vec<&NotificationSpec>> = BTreeMap::new();
    for step in &schedule.steps {
        grouped.entry(step.watch()).or_default().push(step);
    }
    grouped
}

/// A handful of seeds and shapes, not just one -- the review that found the
/// original bugs here was explicit that a single seed/configuration is not
/// enough coverage for a "every schedule is legal" claim.
fn sample_configs() -> Vec<GeneratorConfig> {
    vec![
        GeneratorConfig::default(),
        GeneratorConfig {
            watches: 6,
            steps_per_watch: 30,
            liveness_percent: 100,
            interactive_percent: 100,
            cancel_percent: 60,
            ..GeneratorConfig::default()
        },
        GeneratorConfig {
            watches: 2,
            steps_per_watch: 25,
            liveness_percent: 0,
            interactive_percent: 100,
            volume_change_percent: 100,
            ..GeneratorConfig::default()
        },
        GeneratorConfig {
            watches: 2,
            steps_per_watch: 25,
            liveness_percent: 100,
            interactive_percent: 0,
            ..GeneratorConfig::default()
        },
    ]
}

#[test]
fn this_generator_never_opens_a_watch_other_than_established_or_subscribed() {
    // A GENERATOR property, deliberately narrower than the contract.
    //
    // windows-file-watcher sends the initial Established from inside route
    // establishment and only afterward reports Completion { Subscribed }, so
    // for a watch that establishes against a healthy directory those are the
    // first two notifications in that order. This generator only ever models
    // that case, which is what this asserts.
    //
    // The contract is WIDER, and an earlier version of this test asserted the
    // narrow rule under a contract-sounding name. A route that coalesces onto
    // an already-faulted watcher never sees an initial Established at all --
    // it is suppressed, because there is no settled tier to name -- so it
    // observes Completion { Subscribed } first and its first Established only
    // after recovery, behind Desync { Reestablished } and Resumed. See
    // windows-file-watcher's M14 audit. A handler must not assume Established
    // comes first; this test does not license that assumption, and its name
    // now says whose property it is.
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                match steps.first() {
                    Some(NotificationSpec::Established { .. }) => {
                        assert!(
                            matches!(
                                steps.get(1),
                                Some(NotificationSpec::Completion {
                                    outcome: OutcomeSpec::Subscribed,
                                    ..
                                })
                            ),
                            "seed {seed}, watch {watch}: this generator emits Established \
                             immediately followed by Completion {{ Subscribed }}, got {:?}",
                            steps.get(1)
                        );
                    }
                    Some(NotificationSpec::Completion {
                        outcome: OutcomeSpec::Subscribed,
                        ..
                    }) => {}
                    other => panic!(
                        "seed {seed}, watch {watch}: this generator opens a watch with \
                         Established or Completion {{ Subscribed }}, got {other:?}"
                    ),
                }
            }
        }
    }
}

#[test]
fn this_generator_resolves_every_question_contiguously() {
    // A GENERATOR property. The contract requires only that a question is
    // eventually followed by its bracket's resolution or a terminator; this
    // generator emits them contiguously, which is narrower, and that narrowness
    // is what this asserts.
    //
    // `RetryMode::Interactive` and `VolumeChangePolicy::Confirm` are
    // independent options, so a single recovery may surface a `RetryQuestion`
    // followed by a `VolumeChanged` (or just one, or neither) before the
    // resolution -- so the step immediately after a question is either the
    // other kind of question or the resolution itself.
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                for (index, step) in steps.iter().enumerate() {
                    let is_question = matches!(
                        step,
                        NotificationSpec::RetryQuestion { .. }
                            | NotificationSpec::VolumeChanged { .. }
                    );
                    if is_question {
                        assert!(
                            matches!(
                                steps.get(index + 1),
                                Some(NotificationSpec::RetryQuestion { .. })
                                    | Some(NotificationSpec::VolumeChanged { .. })
                            ) || matches!(
                                steps.get(index + 1),
                                Some(NotificationSpec::Desync { cause, .. })
                                    if matches!(
                                        cause,
                                        windows_file_watcher_example_test_harness::DesyncCauseSpec::Reestablished
                                    )
                            ),
                            "seed {seed}, watch {watch}: a question must be immediately \
                             followed by another question or resolved with \
                             Desync {{ Reestablished }}, got {:?}",
                            steps.get(index + 1)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn this_generator_always_pairs_resumed_with_established() {
    // A GENERATOR property, not a contract rule. `resolve_fault_success`
    // attempts the two back to back, but each is a separate best-effort
    // observation send (file-watcher D-57), so a saturated queue can take
    // `Resumed` and latch `Established` into a `Desync { QueueFull }`. This
    // generator models the unsaturated case, which is what this asserts; a
    // schedule carrying `Resumed` alone is legal and a handler must tolerate it.
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..10 {
            let schedule = generator.generate(seed);
            for (watch, steps) in by_watch(&schedule) {
                for (index, step) in steps.iter().enumerate() {
                    if matches!(step, NotificationSpec::Resumed { .. }) {
                        assert!(
                            matches!(
                                steps.get(index + 1),
                                Some(NotificationSpec::Established { .. })
                            ),
                            "seed {seed}, watch {watch}: Resumed must be immediately followed \
                             by Established, got {:?}",
                            steps.get(index + 1)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn an_interactive_watchs_fault_recovery_always_asks_a_retry_question() {
    // RetryQuestion is unconditional for an interactive watch (enter_fault,
    // watcher.rs: every interactive route is asked on every fault, with no
    // probability involved) -- a schedule where one is skipped is not one
    // file-watcher could produce. liveness_percent: 100 makes every
    // fault-recovery bracket start with Suspended, unambiguously marking
    // where to look; volume_confirm_percent: 0 keeps VolumeChanged out of the
    // way so RetryQuestion is exactly the next step.
    let generator = Generator::with_config(GeneratorConfig {
        watches: 4,
        steps_per_watch: 20,
        liveness_percent: 100,
        interactive_percent: 100,
        volume_confirm_percent: 0,
        weight_batch: 1,
        weight_desync: 1,
        weight_fault_recovery: 3,
        ..GeneratorConfig::default()
    });
    for seed in 0..20 {
        let schedule = generator.generate(seed);
        for (watch, steps) in by_watch(&schedule) {
            for (index, step) in steps.iter().enumerate() {
                if matches!(step, NotificationSpec::Suspended { .. }) {
                    assert!(
                        matches!(
                            steps.get(index + 1),
                            Some(NotificationSpec::RetryQuestion { .. })
                        ),
                        "seed {seed}, watch {watch}: an interactive watch's fault recovery \
                         must ask a RetryQuestion immediately after Suspended, got {:?}",
                        steps.get(index + 1)
                    );
                }
            }
        }
    }
}

#[test]
fn every_generated_schedule_satisfies_the_contract() {
    // The point of this milestone: the sequencing rules are asserted by the
    // crate's own `ContractChecker`, not restated here. Four hand-written tests
    // collapsed into this one -- terminality of `Cancelled`, tier-conditioned
    // `Desync` legality, `VolumeChanged` distinctness, and `VolumeChanged`
    // continuity -- each of which was a copy of a contract rule that could
    // drift from it, and one of which already had.
    //
    // Adding a rule to the contract now covers the generator automatically; a
    // rule the generator violates fails here without anyone remembering to
    // write a matching assertion.
    for config in sample_configs() {
        let generator = Generator::with_config(config);
        for seed in 0..20 {
            let schedule = generator.generate(seed);
            let mut checker = ContractChecker::new();
            for (index, spec) in schedule.steps.iter().enumerate() {
                let notification = spec.to_notification();
                if let Err(violation) = checker.observe(&notification) {
                    panic!(
                        "seed {seed}, step {index}: the generator produced a schedule the \
                         watcher could never emit: {violation:?} on {notification:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn this_generator_exercises_a_coarse_watchs_queue_full_loss() {
    // A GENERATOR coverage property, not a contract rule. `QueueFull` is
    // tier-independent, so a coarse watch may report it -- and this generator
    // must actually produce that case, or the contract check above would pass
    // just as well against a generator that quietly never explored it.
    //
    // Coverage is the one thing a contract checker cannot tell you: it says
    // nothing illegal was emitted, never that anything interesting was.
    let generator = Generator::with_config(GeneratorConfig {
        watches: 4,
        steps_per_watch: 30,
        liveness_percent: 100,
        weight_batch: 2,
        weight_desync: 2,
        weight_fault_recovery: 2,
        ..GeneratorConfig::default()
    });
    let mut saw_coarse_queue_full = false;
    for seed in 0..40 {
        let schedule = generator.generate(seed);
        for (_watch, steps) in by_watch(&schedule) {
            let mut mode = WatchModeSpec::Detailed;
            for step in &steps {
                match step {
                    NotificationSpec::Established {
                        mode: established_mode,
                        ..
                    } => mode = established_mode.clone(),
                    NotificationSpec::Desync { cause, .. }
                        if mode == WatchModeSpec::Coarse
                            && *cause == DesyncCauseSpec::QueueFull =>
                    {
                        saw_coarse_queue_full = true;
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(
        saw_coarse_queue_full,
        "the generator must actually exercise a Coarse watch's QueueFull loss"
    );
}
