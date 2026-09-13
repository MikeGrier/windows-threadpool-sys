// Copyright (c) Mike Grier.

//! Tests for the observation-to-verdict invariants.
//!
//! Every violation branch is reachable here because [`super::check`] takes the
//! VERDICT rather than deriving it. That is not a convenience: `cross_check` is
//! what makes these invariants hold, so no observation can violate them while it
//! is correct, and a derived version would leave each branch reachable only by
//! editing the source. Supplying the verdict lets a test hand over the answer a
//! broken `cross_check` would give.
//!
//! Half of these assert ACCEPTANCE. An invariant that fires on a legal pair
//! costs a reader more than one that misses an illegal one, because noise trains
//! them to ignore the instrument -- the same rule the report oracle is built on.

use super::{BlockingState, Violation, blocking_states, check};
use crate::topology::{
    BracketOutcome, CacheLevel, CoreShape, Observation, PartitioningCache, Verdict,
};

/// An observation whose every check passes, to perturb one field at a time.
///
/// Deliberately a copy of the shape `crate::tests::agreeing_observation` builds
/// rather than a call to it: that helper is private to a sibling test module,
/// and a fixture reaching across module boundaries couples two suites that
/// should be free to change apart.
fn agreeing() -> Observation {
    Observation {
        online_processors: 4,
        groups: 1,
        numa_domains: 1,
        numa_domains_without_processors: 0,
        numa_domains_only_in_cpu_sets: 0,
        numa_domains_unreported: 0,
        numa_domains_with_conflicting_labels: 0,
        topology_was_measured: true,
        cores_only_in_cpu_sets: 0,
        cores_without_processors: 0,
        packages_without_processors: 0,
        overlapping_walk_relations: 0,
        described_relations: 0,
        unreported_relations: 0,
        processor_attribute_conflicts: 0,
        highest_numa_node: Some(0),
        packages: 1,
        cores: vec![CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 4,
        }],
        caches: vec![CacheLevel {
            level: 1,
            processors_per_domain: vec![4],
        }],
        partitioning_cache_level: None,
        enumeration_anomalies: Vec::new(),
        coherence: windows_topology_sys::Coherence::Agreed,
        raw_active_processors: 4,
        raw_group_count: 1,
        raw_highest_numa_node: Some(0),
        bracket: BracketOutcome::HeldStill,
    }
}

#[test]
fn the_observation_this_crate_produces_is_in_no_blocking_state() {
    // The acceptance half, and the premise every perturbation below rests on:
    // if the fixture already blocked agreement, each test would be asserting
    // against two states instead of the one it introduced.
    assert_eq!(blocking_states(&agreeing()), Vec::<BlockingState>::new());
    assert_eq!(check(&agreeing(), Verdict::Agree), Vec::new());
}

#[test]
fn the_verdict_the_crate_actually_draws_holds_every_invariant() {
    // Against the REAL pair rather than a supplied verdict, so this would catch
    // an invariant that is wrong about what `cross_check` does.
    let observation = agreeing();
    let verdict = observation.cross_check().verdict();

    assert_eq!(verdict, Verdict::Agree);
    assert_eq!(check(&observation, verdict), Vec::new());
}

/// Each blocking state, the field that produces it, and the name it reports.
///
/// A table rather than a test each, because the property is identical and the
/// interesting part is that NONE of them is missing. That completeness is
/// asserted by `every_blocking_state_has_a_perturbation`, which compares this
/// table against what `blocking_states` can actually produce -- so a state added
/// to the invariant with no entry here fails rather than going quietly
/// unexercised.
///
/// This doc claimed such a relation before one existed. Found by a review: the
/// table happened to match, which is the condition under which nobody notices.
type Perturbation = (BlockingState, Box<dyn Fn(&mut Observation)>);

fn perturbations() -> Vec<Perturbation> {
    vec![
        (
            BlockingState::PartitioningSummaryMissing,
            Box::new(|o: &mut Observation| o.partitioning_cache_level = Some(9)),
        ),
        (
            BlockingState::EnumerationAnomalies,
            Box::new(|o: &mut Observation| {
                o.enumeration_anomalies = vec![windows_topology_sys::EnumerationAnomaly {
                    source: windows_topology_sys::Source::CpuSets,
                    offset: 0,
                    kind: windows_topology_sys::AnomalyKind::TrailingBytes { remaining: 3 },
                }];
            }),
        ),
        (
            BlockingState::NotMeasured,
            Box::new(|o: &mut Observation| o.topology_was_measured = false),
        ),
        (
            BlockingState::NoCacheLevels,
            Box::new(|o: &mut Observation| o.caches = Vec::new()),
        ),
        (
            BlockingState::NoPackages,
            Box::new(|o: &mut Observation| o.packages = 0),
        ),
        (
            BlockingState::NoCores,
            Box::new(|o: &mut Observation| o.cores = Vec::new()),
        ),
        (
            BlockingState::ContradictoryCore,
            Box::new(|o: &mut Observation| {
                o.cores = vec![CoreShape {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                    processors: 4,
                }];
            }),
        ),
        (
            BlockingState::UnnumberedCacheLevel,
            Box::new(|o: &mut Observation| {
                o.caches = vec![CacheLevel {
                    level: 0,
                    processors_per_domain: vec![4],
                }];
            }),
        ),
        (
            BlockingState::EnumerationsDisagreed,
            Box::new(|o: &mut Observation| {
                o.coherence = windows_topology_sys::Coherence::NotCollected;
            }),
        ),
        (
            BlockingState::BracketNotHeld,
            Box::new(|o: &mut Observation| o.bracket = BracketOutcome::Changed),
        ),
    ]
}

#[test]
fn every_blocking_state_has_a_perturbation() {
    // **Compared against `BlockingState::ALL`, not against the table itself.**
    // The first version of this guard derived BOTH of its sets from
    // `perturbations()` -- the reached set by applying them, the labelled set by
    // reading them -- so a new branch in `blocking_states` that no mutation
    // activated appeared in neither, and both loops stayed green. It could only
    // confirm that existing labels described existing mutations, which is not
    // what its name claims. Found by a review, one round after the guard was
    // added in response to an earlier one.
    //
    // `ALL` is exhaustive by compiler: `described()` matches on every variant,
    // so adding one without listing it there fails to build.
    let mut reached: Vec<BlockingState> = Vec::new();
    for (_, mutate) in perturbations() {
        let mut observation = agreeing();
        mutate(&mut observation);
        for state in blocking_states(&observation) {
            if !reached.contains(&state) {
                reached.push(state);
            }
        }
    }

    let labelled: Vec<BlockingState> = perturbations()
        .into_iter()
        .map(|(state, _)| state)
        .collect();

    for state in BlockingState::ALL {
        assert!(
            labelled.contains(state),
            "`{state:?}` is a blocking state with no entry in the perturbation \
             table, so nothing shows that it fires or that `cross_check` \
             already forbids it"
        );
        assert!(
            reached.contains(state),
            "`{state:?}` has a table entry whose mutation does not actually \
             produce it, so the row for it tests nothing"
        );
    }

    for state in &reached {
        assert!(
            BlockingState::ALL.contains(state),
            "`{state:?}` is reported by `blocking_states` and missing from \
             `BlockingState::ALL`"
        );
    }
}

#[test]
fn every_blocking_state_forbids_an_agreeing_verdict() {
    for (state, mutate) in perturbations() {
        let mut observation = agreeing();
        mutate(&mut observation);

        assert!(
            blocking_states(&observation).contains(&state),
            "{state:?}: the observation is in this state and `blocking_states` \
             did not say so"
        );
        assert!(
            check(&observation, Verdict::Agree)
                .contains(&Violation::StateWithAgreeingVerdict { state }),
            "{state:?}: the state is present beside an agreeing verdict and the \
             invariant did not fire"
        );
    }
}

#[test]
fn every_blocking_state_is_one_the_real_cross_check_already_reports() {
    // **The invariants must agree with `cross_check`, or they are a second
    // opinion rather than a postcondition.** For each state, the verdict the
    // crate actually draws must already be something other than `agree` -- so
    // the invariant is pinning behaviour that exists rather than demanding
    // behaviour that does not.
    //
    // This is what makes the whole module a postcondition. If one of these
    // failed, the right response would be to fix `cross_check`, not to relax
    // the invariant.
    for (state, mutate) in perturbations() {
        let mut observation = agreeing();
        mutate(&mut observation);

        let cross_check = observation.cross_check();
        assert_ne!(
            cross_check.verdict(),
            Verdict::Agree,
            "{state:?}: the invariant forbids `agree` here, so `cross_check` must \
             already forbid it: {cross_check:?}"
        );
        assert_eq!(
            check(&observation, cross_check.verdict()),
            Vec::new(),
            "{state:?}: and against the real verdict there is nothing to report"
        );
    }
}

#[test]
fn a_blocking_state_is_silent_when_the_verdict_already_admits_it() {
    // The acceptance half of the whole module: these rules constrain the
    // AGREEING verdict and nothing else. A report that says it is incomplete is
    // free to be in any of these states -- that is what incomplete means.
    for verdict in [Verdict::Incomplete, Verdict::Disagree] {
        for (state, mutate) in perturbations() {
            let mut observation = agreeing();
            mutate(&mut observation);

            assert_eq!(
                check(&observation, verdict),
                Vec::new(),
                "{state:?}: {verdict:?} admits the doubt, so there is nothing to \
                 contradict"
            );
        }
    }
}

#[test]
fn the_partitioning_state_is_read_from_the_observation_not_the_list() {
    // **The distinction the module is built on, asserted rather than described.**
    // A rule reading `parse_incomplete` cannot catch a DELETED push site: the
    // deletion empties the list, so the rule sees nothing and the verdict is
    // `agree` legitimately. Reading the observation is what survives that.
    //
    // Expressed here as: the state is visible with no reference to the
    // cross-check at all.
    let mut observation = agreeing();
    observation.partitioning_cache_level = Some(9);

    assert!(matches!(
        observation.partitioning_cache(),
        PartitioningCache::SummaryMissing(9)
    ));
    assert!(
        blocking_states(&observation).contains(&BlockingState::PartitioningSummaryMissing),
        "read from the observation, with the cross-check never consulted"
    );
}

#[test]
fn an_agreeing_verdict_requires_the_counters_to_have_been_read() {
    // Zero is how both counters report failure, so `agree` beside a zero is the
    // verdict claiming a comparison that could not have happened.
    for (counter, mutate) in [
        (
            "GetActiveProcessorCount",
            Box::new(|o: &mut Observation| o.raw_active_processors = 0)
                as Box<dyn Fn(&mut Observation)>,
        ),
        (
            "GetActiveProcessorGroupCount",
            Box::new(|o: &mut Observation| o.raw_group_count = 0),
        ),
    ] {
        let mut observation = agreeing();
        mutate(&mut observation);

        assert_eq!(
            check(&observation, Verdict::Agree),
            vec![Violation::AgreedWithoutComparingCounter { counter }],
            "{counter}: a failed read cannot sit beside a verdict claiming \
             every check was made"
        );

        let cross_check = observation.cross_check();
        assert_ne!(
            cross_check.verdict(),
            Verdict::Agree,
            "{counter}: and the real cross-check already forbids it: \
             {cross_check:?}"
        );
    }
}

#[test]
fn an_agreeing_verdict_requires_the_counters_to_have_matched() {
    for (counter, parsed, read, mutate) in [
        (
            "GetActiveProcessorCount",
            4,
            8,
            Box::new(|o: &mut Observation| o.raw_active_processors = 8)
                as Box<dyn Fn(&mut Observation)>,
        ),
        (
            "GetActiveProcessorGroupCount",
            1,
            2,
            Box::new(|o: &mut Observation| o.raw_group_count = 2),
        ),
    ] {
        let mut observation = agreeing();
        mutate(&mut observation);

        assert_eq!(
            check(&observation, Verdict::Agree),
            vec![Violation::AgreedDespiteCounterMismatch {
                counter,
                parsed,
                read,
            }],
            "{counter}: the enumeration and the counter differ, so `agree` is \
             not available"
        );
    }
}

#[test]
fn the_numa_counter_is_deliberately_not_held_to_the_enumeration() {
    // `GetNumaHighestNodeNumber` reports the largest node NUMBER, which Windows
    // does not promise equals the node count -- nodes 0 and 2 are a valid
    // sparse topology. Holding it to `numa_domains` would manufacture a
    // violation on hardware reporting itself correctly, which is the same
    // over-claim `cross_check` was corrected to stop making.
    let mut observation = agreeing();
    observation.numa_domains = 2;
    observation.highest_numa_node = Some(2);
    observation.raw_highest_numa_node = Some(2);

    assert_eq!(observation.cross_check().verdict(), Verdict::Agree);
    assert_eq!(check(&observation, Verdict::Agree), Vec::new());
}
