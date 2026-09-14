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

/// A named mutation of an observation, for the tests that build a corpus.
type Perturb = Box<dyn Fn(&mut Observation)>;
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
type Perturbation = (BlockingState, Perturb);

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
                o.coherence = windows_topology_sys::Coherence::Disagreed {
                    attempts: 2,
                    walk_only: Vec::new(),
                    cpu_sets_only: Vec::new(),
                };
            }),
        ),
        (
            // Kept distinct from the `Disagreed` row above, because the row
            // publishes a different code for each and a shared perturbation
            // would leave one of the two codes unexercised -- which is exactly
            // how the states came to be conflated.
            BlockingState::CoherenceNotCollected,
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
    // **`ALL` is exhaustive by construction, not by the `described()` match.**
    // This comment used to claim the latter, and it was false: the match forces
    // a new variant to acquire an ARM, never an entry in a separate array.
    // Measured -- a variant absent from a hand-written `ALL` compiled and left
    // all ten tests here green. `BlockingState` is now declared by a macro from
    // one list, so the variants and `ALL` are the same list.
    //
    // The reverse loop below is NOT a substitute for that. It catches a state
    // `blocking_states` produces and `ALL` omits, but only once some
    // perturbation reaches it -- and a state with no perturbation entry is
    // precisely the case this test exists to catch, so relying on it would be
    // circular in exactly the case that matters.
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
    //
    // A sparse topology is the whole point of this case: `numa_domains` is 2
    // while the highest node number is also 2, so the two differ by one and a
    // count-shaped rule would fire. The highest-against-highest rule below does
    // not, which is what makes them different rules rather than one rule that
    // was left out.
    let mut observation = agreeing();
    observation.numa_domains = 2;
    observation.highest_numa_node = Some(2);
    observation.raw_highest_numa_node = Some(2);

    assert_eq!(observation.cross_check().verdict(), Verdict::Agree);
    assert_eq!(check(&observation, Verdict::Agree), Vec::new());
}

#[test]
fn the_processor_guard_agrees_with_the_one_cross_check_applies() {
    // **Two copies of one boundary, deliberately, and nothing held them to each
    // other.** `blocking_states` recomputes `online_processors > 0 && packages
    // == 0` rather than asking `cross_check`, and it MUST: a rule that reads
    // `cross_check`'s output restates `verdict()` and is blind to a deleted push
    // site, which is the whole reason this module exists. Independence is the
    // design; agreement is the property, and the property was untested.
    //
    // Found by a mutation sweep, not by review. Relaxing either `>` to `>=` here
    // survived five review rounds across four models, because the test that
    // names this boundary --
    // `a_topology_with_no_processors_at_all_is_not_accused_of_hiding_packages`
    // -- asserts on `cross_check` and so covers only the OTHER copy.
    //
    // Written as a correspondence over a corpus that spans the boundary rather
    // than as a single case, so a future guard whose condition drifts on either
    // side is caught wherever it drifts.
    let shapes: [(&str, Perturb); 4] = [
        (
            "no processors, and nothing else reported either",
            Box::new(|o: &mut Observation| {
                o.online_processors = 0;
                o.raw_active_processors = 0;
                o.packages = 0;
                o.cores = Vec::new();
            }),
        ),
        (
            "no processors, but packages and cores reported",
            Box::new(|o: &mut Observation| {
                o.online_processors = 0;
                o.raw_active_processors = 0;
            }),
        ),
        (
            "processors reported, packages absent",
            Box::new(|o: &mut Observation| o.packages = 0),
        ),
        (
            "processors reported, cores absent",
            Box::new(|o: &mut Observation| o.cores = Vec::new()),
        ),
    ];

    for (shape, mutate) in shapes {
        let mut observation = agreeing();
        mutate(&mut observation);

        let states = blocking_states(&observation);
        let parse = observation.cross_check().parse_incomplete;

        assert_eq!(
            states.contains(&BlockingState::NoPackages),
            parse.contains(&crate::topology::ParseIncomplete::NoPackages),
            "{shape}: this module and `cross_check` disagree about whether \
             absent packages are a finding -- states {states:?}, parse {parse:?}"
        );
        assert_eq!(
            states.contains(&BlockingState::NoCores),
            parse.contains(&crate::topology::ParseIncomplete::NoCores),
            "{shape}: this module and `cross_check` disagree about whether \
             absent cores are a finding -- states {states:?}, parse {parse:?}"
        );
    }
}

#[test]
fn a_corpus_spanning_the_processor_guard_reaches_both_of_its_answers() {
    // The correspondence above compares two computations, so it passes
    // vacuously if every shape lands on the same side of the boundary. Assert
    // that the corpus reaches BOTH answers, or the test is a comparison of two
    // constants.
    let mut absent = agreeing();
    absent.packages = 0;
    absent.cores = Vec::new();
    assert!(
        blocking_states(&absent).contains(&BlockingState::NoPackages),
        "a machine with processors and no packages must be in the state"
    );

    let mut unmeasured = agreeing();
    unmeasured.online_processors = 0;
    unmeasured.raw_active_processors = 0;
    unmeasured.packages = 0;
    unmeasured.cores = Vec::new();
    assert!(
        !blocking_states(&unmeasured).contains(&BlockingState::NoPackages),
        "a topology that reported no processors is not accused of hiding \
         packages -- this is the `> 0` the sweep found unguarded"
    );
    assert!(
        !blocking_states(&unmeasured).contains(&BlockingState::NoCores),
        "nor of hiding cores"
    );
}

#[test]
fn an_agreeing_verdict_requires_the_numa_counter_to_have_been_read() {
    // The branch `cross_check` takes when `GetNumaHighestNodeNumber` fails: it
    // files `HighestNumaNodeFailed` and returns, so `agree` is unreachable.
    // Delete that push and `agree` becomes reachable beside an unread counter,
    // which is the defect this rule exists to name.
    let mut observation = agreeing();
    observation.raw_highest_numa_node = None;

    assert_eq!(
        check(&observation, Verdict::Agree),
        vec![Violation::AgreedWithoutComparingCounter {
            counter: "GetNumaHighestNodeNumber"
        }]
    );
}

#[test]
fn an_agreeing_verdict_requires_the_numa_counter_to_have_matched() {
    // Both directions of the mismatch, because the parse's side is an `Option`
    // and the absent case renders differently -- a rule whose message says
    // "carries None" where a reader expected a number is a rule that will be
    // misread in the one situation it fires.
    let mut mismatched = agreeing();
    mismatched.highest_numa_node = Some(1);
    mismatched.raw_highest_numa_node = Some(3);

    assert_eq!(
        check(&mismatched, Verdict::Agree),
        vec![Violation::AgreedDespiteNumaMismatch {
            parsed: Some(1),
            counter: 3
        }]
    );

    let mut unparsed = agreeing();
    unparsed.highest_numa_node = None;
    unparsed.raw_highest_numa_node = Some(3);

    assert_eq!(
        check(&unparsed, Verdict::Agree),
        vec![Violation::AgreedDespiteNumaMismatch {
            parsed: None,
            counter: 3
        }]
    );
    assert!(
        Violation::AgreedDespiteNumaMismatch {
            parsed: None,
            counter: 3
        }
        .to_string()
        .contains("no NUMA node at all"),
        "the absent case must not render as a number"
    );
}

#[test]
fn every_numa_counter_branch_in_the_real_cross_check_is_one_this_module_forbids() {
    // **The claim in this module's header, checked rather than asserted**: a
    // push site deleted from `cross_check` fires a rule here. For each NUMA
    // COUNTER branch, the verdict the real `cross_check` draws must already be
    // something other than `agree`, AND this module must forbid `agree` for the
    // same observation -- so the rule pins behaviour that exists rather than
    // demanding behaviour that does not.
    //
    // **Named for the counter on purpose.** This was
    // `every_numa_branch_...`, which was false: `cross_check` also pushes
    // `NumaDomainsOnlyInCpuSets`, `NumaDomainsUnreported` and
    // `NumaDomainsWithConflictingLabels`, none of which has a `BlockingState`,
    // so deleting one of those push sites is not caught. A review found the
    // name claiming the whole family while the body covered the two branches
    // that read `raw_highest_numa_node`. The absentees are queued as M4.1; the
    // name now says which half is guarded.
    //
    // This is the pairing the other states get from
    // `every_blocking_state_is_one_the_real_cross_check_already_reports`; NUMA
    // had neither half until a review found the header's claim was false for
    // exactly these two branches.
    for (what, mutate) in [
        (
            "counter unreadable",
            Box::new(|o: &mut Observation| o.raw_highest_numa_node = None)
                as Box<dyn Fn(&mut Observation)>,
        ),
        (
            "counter disagrees with the parse",
            Box::new(|o: &mut Observation| {
                o.highest_numa_node = Some(1);
                o.raw_highest_numa_node = Some(3);
            }),
        ),
    ] {
        let mut observation = agreeing();
        mutate(&mut observation);

        let cross_check = observation.cross_check();
        assert_ne!(
            cross_check.verdict(),
            Verdict::Agree,
            "{what}: `cross_check` must already forbid `agree`: {cross_check:?}"
        );
        assert_ne!(
            check(&observation, Verdict::Agree),
            Vec::new(),
            "{what}: and this module must forbid it too, or a deleted push site \
             here fires nothing"
        );
        assert_eq!(
            check(&observation, cross_check.verdict()),
            Vec::new(),
            "{what}: the verdict the crate actually draws must hold every rule"
        );
    }
}
