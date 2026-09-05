// Copyright (c) 2026 Mike Grier
//! Tests for the granularity order.

use super::Granularity;
use crate::domain::{Domain, DomainKind, Processor, ProcessorId};
use crate::observed::Observed;
use crate::processor_set::ProcessorSet;
use crate::provenance::Provenance;
use crate::relation::CacheKind;
use crate::topology::Coherence;
use crate::topology::MachineMemoryTopology;

/// `count` processors in group 0, all online.
fn processors(count: u8) -> Vec<Processor> {
    (0..count)
        .map(|number| Processor {
            id: ProcessorId { group: 0, number },
            online: true,
            capacity: 0,
        })
        .collect()
}

fn set(numbers: &[u8]) -> ProcessorSet {
    let mut s = ProcessorSet::empty();
    for &n in numbers {
        s.insert(0, n);
    }
    s
}

fn cache(level: u8, numbers: &[u8], cache_type: CacheKind) -> Domain {
    Domain {
        kind: DomainKind::Cache {
            level,
            associativity: 8,
            line_size: 64,
            size_bytes: 32 * 1024,
            cache_type,
        },
        processors: set(numbers),
        observations: Vec::new(),
    }
}

fn core(numbers: &[u8]) -> Domain {
    Domain {
        kind: DomainKind::Core {
            simultaneous_multithreading: numbers.len() > 1,
            efficiency_class: 0,
        },
        processors: set(numbers),
        observations: Vec::new(),
    }
}

fn memory(numbers: &[u8]) -> Domain {
    Domain {
        kind: DomainKind::Memory {
            memory_bytes: Observed::NotObserved,
        },
        processors: set(numbers),
        observations: Vec::new(),
    }
}

fn topology(processor_count: u8, domains: Vec<Domain>) -> MachineMemoryTopology {
    MachineMemoryTopology {
        processors: processors(processor_count),
        domains,
        cpu_sets: None,
        provenance: Provenance::Synthetic,
        coherence: Coherence::NotCollected,
        enumeration_anomalies: Vec::new(),
        processor_attributes: Vec::new(),
    }
}

// --- the machine, and totality (M2+.3) ---

#[test]
fn machine_processors_names_every_processor_the_topology_knows() {
    let t = topology(4, Vec::new());
    assert_eq!(t.machine_processors(), set(&[0, 1, 2, 3]));
}

#[test]
fn machine_processors_includes_offline_slots() {
    // A slot that exists is part of the machine. Excluding it would make a
    // query naming it answer "nothing shared" rather than "the machine".
    let mut t = topology(4, Vec::new());
    t.processors[3].online = false;
    assert_eq!(t.machine_processors(), set(&[0, 1, 2, 3]));
}

#[test]
fn a_pair_no_relation_covers_answers_the_machine_rather_than_nothing() {
    // Two NUMA nodes, nothing spanning them: exactly the cross-node case the
    // top exists for.
    let t = topology(4, vec![memory(&[0, 1]), memory(&[2, 3])]);
    assert_eq!(t.minimal_shared(&set(&[0, 3])), vec![Granularity::Machine]);
}

#[test]
fn the_machine_never_appears_beside_an_observed_relation() {
    // It is the fallback that makes the query total, not an element competing
    // with what the platform reported.
    let t = topology(4, vec![memory(&[0, 1, 2, 3])]);
    let answer = t.minimal_shared(&set(&[0, 3]));
    assert_eq!(answer.len(), 1);
    assert!(!answer[0].is_machine());
}

#[test]
fn a_topology_with_no_domains_at_all_still_answers() {
    let t = topology(2, Vec::new());
    assert_eq!(t.minimal_shared(&set(&[0, 1])), vec![Granularity::Machine]);
}

// --- inclusion, not level number (M2+.2) ---

#[test]
fn the_tightest_covering_relation_wins_regardless_of_level_number() {
    let t = topology(
        4,
        vec![
            cache(3, &[0, 1, 2, 3], CacheKind::Unified),
            cache(2, &[0, 1], CacheKind::Unified),
            cache(2, &[2, 3], CacheKind::Unified),
        ],
    );
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 1);
    let domain = answer[0].relation().expect("a relation, not the machine");
    assert_eq!(domain.processors, set(&[0, 1]));
}

#[test]
fn a_lower_level_number_does_not_win_when_it_covers_more() {
    // The inversion firmware numbering would get wrong: an L1 that (on this
    // synthetic machine) spans everything while an L2 splits it. Inclusion
    // answers with the L2 because it is smaller; a level-number sort would
    // have answered with the L1.
    let t = topology(
        4,
        vec![
            cache(1, &[0, 1, 2, 3], CacheKind::Unified),
            cache(2, &[0, 1], CacheKind::Unified),
        ],
    );
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 1);
    let domain = answer[0].relation().expect("a relation");
    assert!(
        matches!(domain.kind, DomainKind::Cache { level: 2, .. }),
        "inclusion must pick the smaller set, not the lower level number"
    );
}

#[test]
fn kinds_that_share_no_numbering_are_still_ordered() {
    // A core against a memory domain: no level number relates them, and
    // inclusion does.
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 1);
    assert!(matches!(
        answer[0].relation().expect("a relation").kind,
        DomainKind::Core { .. }
    ));
}

// --- incomparability and ties (M2+.4) ---

#[test]
fn two_relations_over_the_same_processors_both_survive() {
    // Measured shape: L1 arrives as a data cache and an instruction cache over
    // the very same processors. Picking one would be arbitrary.
    let t = topology(
        4,
        vec![
            cache(1, &[0, 1], CacheKind::Data),
            cache(1, &[0, 1], CacheKind::Instruction),
            cache(3, &[0, 1, 2, 3], CacheKind::Unified),
        ],
    );
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 2, "a tie is reported, not broken: {answer:?}");
    assert!(
        answer
            .iter()
            .all(|g| g.relation().expect("a relation").processors == set(&[0, 1]))
    );
}

#[test]
fn genuinely_incomparable_relations_both_survive() {
    // Neither contains the other, and both contain the query. This is the case
    // that makes the answer a set by construction rather than by accident.
    let t = topology(
        4,
        vec![memory(&[0, 1, 2]), cache(2, &[0, 1, 3], CacheKind::Unified)],
    );
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 2, "{answer:?}");
}

#[test]
fn a_relation_strictly_inside_another_excludes_it() {
    let t = topology(
        4,
        vec![
            memory(&[0, 1, 2, 3]),
            core(&[0, 1]),
            cache(2, &[0, 1, 2], CacheKind::Unified),
        ],
    );
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 1);
    assert_eq!(
        answer[0].relation().expect("a relation").processors,
        set(&[0, 1])
    );
}

// --- edges ---

#[test]
fn a_processor_the_topology_does_not_know_answers_empty_not_the_machine() {
    // Claiming the machine contains a processor it has never heard of would be
    // an invention. Totality holds over what the topology knows.
    let t = topology(2, vec![memory(&[0, 1])]);
    assert!(t.minimal_shared(&set(&[0, 7])).is_empty());
}

#[test]
fn the_order_holds_across_processor_groups() {
    // Every other test here uses group 0 alone, which cannot exercise the
    // multi-group path in `ProcessorSet::is_subset` -- and that path is where
    // "this group is not covered" differs from "this group is absent".
    let mut t = topology(2, Vec::new());
    t.processors.push(Processor {
        id: ProcessorId {
            group: 1,
            number: 0,
        },
        online: true,
        capacity: 0,
    });

    let mut spanning = set(&[0, 1]);
    spanning.insert(1, 0);
    t.domains.push(Domain {
        kind: DomainKind::Memory {
            memory_bytes: Observed::NotObserved,
        },
        processors: spanning,
        observations: Vec::new(),
    });
    t.domains.push(core(&[0, 1]));

    // Within group 0 the core is tighter than the spanning domain.
    let within = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(within.len(), 1);
    assert!(matches!(
        within[0].relation().expect("a relation").kind,
        DomainKind::Core { .. }
    ));

    // A pair straddling the group boundary is covered only by the spanning
    // domain -- the core must not qualify.
    let mut across = set(&[0]);
    across.insert(1, 0);
    let answer = t.minimal_shared(&across);
    assert_eq!(answer.len(), 1);
    assert!(matches!(
        answer[0].relation().expect("a relation").kind,
        DomainKind::Memory { .. }
    ));
}

#[test]
fn a_single_processor_answers_its_tightest_relation() {
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    let answer = t.minimal_shared(&set(&[0]));
    assert_eq!(answer.len(), 1);
    assert_eq!(
        answer[0].relation().expect("a relation").processors,
        set(&[0, 1])
    );
}

#[test]
fn the_empty_set_is_covered_by_everything_so_the_smallest_wins() {
    // Not a case a caller has reason to ask, but it must not panic or invent.
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    let answer = t.minimal_shared(&ProcessorSet::empty());
    assert_eq!(answer.len(), 1);
    assert_eq!(
        answer[0].relation().expect("a relation").processors,
        set(&[0, 1])
    );
}

#[test]
fn a_memory_only_domain_never_covers_anything_but_does_not_break_the_order() {
    // D-5's CXL-shaped node has no processors, so it covers no non-empty
    // query. It must not become a spurious minimum.
    let t = topology(2, vec![memory(&[]), core(&[0, 1])]);
    let answer = t.minimal_shared(&set(&[0, 1]));
    assert_eq!(answer.len(), 1);
    assert!(matches!(
        answer[0].relation().expect("a relation").kind,
        DomainKind::Core { .. }
    ));
}

// --- the comparison itself ---

#[test]
fn is_finer_than_is_strict() {
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    let core_g = Granularity::Relation(&t.domains[0]);
    let memory_g = Granularity::Relation(&t.domains[1]);

    assert!(t.is_finer_than(core_g, memory_g));
    assert!(!t.is_finer_than(memory_g, core_g));
    assert!(
        !t.is_finer_than(core_g, core_g),
        "strict: nothing is finer than itself"
    );
}

#[test]
fn every_relation_is_finer_than_the_machine_unless_it_spans_it() {
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    assert!(t.is_finer_than(Granularity::Relation(&t.domains[0]), Granularity::Machine));
    assert!(
        !t.is_finer_than(Granularity::Relation(&t.domains[1]), Granularity::Machine),
        "a relation covering every processor ties with the machine rather than being finer"
    );
}

#[test]
fn incomparable_granularities_are_finer_in_neither_direction() {
    let t = topology(
        4,
        vec![memory(&[0, 1, 2]), cache(2, &[0, 1, 3], CacheKind::Unified)],
    );
    let left = Granularity::Relation(&t.domains[0]);
    let right = Granularity::Relation(&t.domains[1]);
    assert!(!t.is_finer_than(left, right));
    assert!(!t.is_finer_than(right, left));
}

#[test]
fn the_machine_covers_every_processor_as_a_granularity() {
    let t = topology(3, vec![core(&[0, 1])]);
    assert_eq!(Granularity::Machine.processors(&t), set(&[0, 1, 2]));
    assert_eq!(
        Granularity::Relation(&t.domains[0]).processors(&t),
        set(&[0, 1])
    );
}

#[test]
fn relation_and_is_machine_agree() {
    let t = topology(2, vec![core(&[0, 1])]);
    let relation = Granularity::Relation(&t.domains[0]);
    assert!(!relation.is_machine());
    assert!(relation.relation().is_some());
    assert!(Granularity::Machine.is_machine());
    assert!(Granularity::Machine.relation().is_none());
}

// --- M4+.1: the pairwise helper and its upper-bound flag ---

fn id(number: u8) -> ProcessorId {
    ProcessorId { group: 0, number }
}

#[test]
fn proximity_is_the_pairwise_face_of_minimal_shared() {
    // The helper must not be a second implementation of the grouping -- that
    // is the SH-16.9 defect one level up -- so it must agree with the
    // collection exactly.
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    assert_eq!(
        t.proximity(&[id(0), id(1)]).shared,
        t.minimal_shared(&set(&[0, 1]))
    );
    assert_eq!(
        t.proximity(&[id(0), id(3)]).shared,
        t.minimal_shared(&set(&[0, 3]))
    );
}

#[test]
fn proximity_generalises_past_two_processors() {
    // Nothing in the query is specific to a pair; the same call sizes a fan-in
    // over a whole block.
    let t = topology(4, vec![core(&[0, 1]), memory(&[0, 1, 2, 3])]);
    let over_three = t.proximity(&[id(0), id(1), id(2)]);
    assert_eq!(over_three.shared.len(), 1);
    assert!(matches!(
        over_three.shared[0].relation().expect("a relation").kind,
        DomainKind::Memory { .. }
    ));
}

#[test]
fn a_pair_no_relation_covers_is_the_machine_and_the_query_stays_total() {
    let t = topology(4, vec![memory(&[0, 1]), memory(&[2, 3])]);
    assert_eq!(
        t.proximity(&[id(0), id(3)]).shared,
        vec![Granularity::Machine]
    );
}

#[test]
fn a_complete_machine_reports_no_unobserved_finer_granularity() {
    // Every processor is covered by every kind this machine reports, so the
    // answer is the answer -- not an upper bound.
    let t = topology(
        4,
        vec![
            core(&[0, 1]),
            core(&[2, 3]),
            cache(2, &[0, 1], CacheKind::Unified),
            cache(2, &[2, 3], CacheKind::Unified),
        ],
    );
    assert!(!t.proximity(&[id(0), id(1)]).finer_unobserved);
}

#[test]
fn a_processor_in_no_relation_of_a_reported_kind_makes_the_answer_an_upper_bound() {
    // Processor 3 is in no core, while cores exist. The platform has said
    // nothing about whether 0 and 3 share one, so "the tightest shared thing is
    // the machine" is at most true -- and a caller told otherwise would pick a
    // slower channel than the hardware can support and never learn why.
    let t = topology(4, vec![core(&[0, 1]), core(&[2])]);
    let answer = t.proximity(&[id(0), id(3)]);
    assert!(
        answer.finer_unobserved,
        "partial coverage at a reported kind is a gap, not a fact: {answer:?}"
    );
}

#[test]
fn a_machine_that_reports_no_caches_is_complete_not_incomplete() {
    // The conflation D-13 exists to remove, at the query level: absence of a
    // KIND is a complete description of a machine without it, whereas absence
    // of a processor FROM a kind that exists is a gap. Counting the first as a
    // gap would make every answer on such a machine an upper bound.
    let t = topology(2, vec![core(&[0, 1])]);
    assert!(!t.proximity(&[id(0), id(1)]).finer_unobserved);
}

#[test]
fn a_memory_gap_is_not_an_unobserved_finer_granularity() {
    // A processor in no memory domain is M4+.3's *unplaced* case, which has its
    // own answer. Treating it as evidence of unreported finer sharing would
    // conflate two different absences.
    let t = topology(4, vec![core(&[0, 1]), core(&[2, 3]), memory(&[0, 1])]);
    assert!(!t.proximity(&[id(0), id(2)]).finer_unobserved);
}

#[test]
fn only_reports_a_tie_rather_than_hiding_it() {
    let t = topology(
        4,
        vec![
            cache(1, &[0, 1], CacheKind::Data),
            cache(1, &[0, 1], CacheKind::Instruction),
        ],
    );
    let answer = t.proximity(&[id(0), id(1)]);
    assert_eq!(answer.shared.len(), 2);
    assert_eq!(answer.only(), None, "a caller must not silently take one");

    let single = topology(2, vec![core(&[0, 1])]);
    assert!(single.proximity(&[id(0), id(1)]).only().is_some());
}

#[test]
fn an_unknown_processor_yields_an_empty_answer_not_the_machine() {
    let t = topology(2, vec![core(&[0, 1])]);
    assert!(t.proximity(&[id(0), id(7)]).shared.is_empty());
}

#[test]
fn a_processor_number_too_wide_for_this_platform_is_reported_not_panicked() {
    // `ProcessorId`'s fields are public and `number` is a `u8`, so any value up
    // to 255 is constructible; a `ProcessorSet` holds one bit per processor in
    // a `usize`, and `insert` asserts on anything wider. `proximity` used to
    // pass the id straight in, so a public query aborted the process over its
    // own argument. Raised in the PR #56 review.
    //
    // The ceiling is `usize::BITS`, so this test's id is out of range on every
    // supported target: 64 on x86-64, 32 on `i686-pc-windows-msvc`, and 255 is
    // beyond both. That target-dependence is the sharp part -- a `number` of 40
    // is an ordinary query on one and an abort on the other.
    let topology = topology(2, Vec::new());
    let impossible = ProcessorId {
        group: 0,
        number: 255,
    };

    let proximity = topology.proximity(&[impossible]);

    assert_eq!(
        proximity.unrepresentable,
        vec![impossible],
        "an id this platform cannot express must be named, not silently dropped"
    );
}

#[test]
fn an_unrepresentable_processor_does_not_corrupt_the_answer_for_the_others() {
    // The complement: the representable processors must still be answered
    // about. Skipping the bad id and saying nothing would answer a question
    // about fewer processors than were asked, and a caller comparing two
    // processors would read "nothing shared" from an argument it got wrong.
    let topology = topology(2, Vec::new());
    let real = ProcessorId {
        group: 0,
        number: 0,
    };
    let impossible = ProcessorId {
        group: 0,
        number: 255,
    };

    let alone = topology.proximity(&[real]);
    let mixed = topology.proximity(&[real, impossible]);

    assert!(
        mixed.unrepresentable.contains(&impossible),
        "the bad id must be reported"
    );
    assert_eq!(
        mixed.shared.len(),
        alone.shared.len(),
        "the representable processor's answer must be unchanged by its company"
    );
}
