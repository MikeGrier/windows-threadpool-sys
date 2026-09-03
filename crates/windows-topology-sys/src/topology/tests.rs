// Copyright (c) 2026 Mike Grier
use super::*;
use crate::domain::{Domain, DomainKind};
use crate::processor_set::ProcessorSet;
use crate::relation::CacheKind;

fn synthetic() -> MachineMemoryTopology {
    let group0 = ProcessorSet::from_group_mask(0, 0b11);
    MachineMemoryTopology {
        processors: vec![
            Processor {
                id: ProcessorId {
                    group: 0,
                    number: 0,
                },
                online: true,
                capacity: 0,
            },
            Processor {
                id: ProcessorId {
                    group: 0,
                    number: 1,
                },
                online: true,
                capacity: 10,
            },
        ],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: group0.clone(),
                observations: Vec::new(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: group0.clone(),
                observations: Vec::new(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: true,
                    efficiency_class: 10,
                },
                processors: group0.clone(),
                observations: Vec::new(),
            },
            Domain {
                kind: DomainKind::Cache {
                    level: 3,
                    associativity: 16,
                    line_size: 64,
                    size_bytes: 32 * 1024 * 1024,
                    cache_type: CacheKind::Unified,
                },
                processors: group0.clone(),
                observations: Vec::new(),
            },
            Domain {
                kind: DomainKind::Cache {
                    level: 2,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 512 * 1024,
                    cache_type: CacheKind::Unified,
                },
                processors: group0,
                observations: Vec::new(),
            },
            Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                processors: ProcessorSet::empty(),
                observations: Vec::new(),
            },
        ],
        cpu_sets: None,
        // Named rather than defaulted, so this fixture states what it is. The
        // helper is called `synthetic` and now says so in the value too.
        provenance: Provenance::Synthetic,
    }
}

#[test]
fn groups_returns_only_group_domains() {
    assert_eq!(synthetic().groups().count(), 1);
}

#[test]
fn packages_returns_only_package_domains() {
    assert_eq!(synthetic().packages().count(), 1);
}

#[test]
fn cores_returns_only_core_domains() {
    assert_eq!(synthetic().cores().count(), 1);
}

#[test]
fn caches_returns_every_cache_regardless_of_level() {
    assert_eq!(synthetic().caches().count(), 2);
}

#[test]
fn caches_at_level_filters_to_exactly_that_level() {
    let topo = synthetic();
    assert_eq!(topo.caches_at_level(3).count(), 1);
    assert_eq!(topo.caches_at_level(2).count(), 1);
    assert_eq!(topo.caches_at_level(1).count(), 0);
}

#[test]
fn memory_domains_includes_one_with_no_processors() {
    let topo = synthetic();
    let memory: Vec<_> = topo.memory_domains().collect();
    assert_eq!(memory.len(), 1);
    assert!(memory[0].processors.is_empty());
}

#[test]
fn processor_looks_up_by_id() {
    let topo = synthetic();
    let found = topo
        .processor(ProcessorId {
            group: 0,
            number: 1,
        })
        .expect("processor exists");
    assert_eq!(found.capacity, 10);
    assert!(
        topo.processor(ProcessorId {
            group: 9,
            number: 9
        })
        .is_none()
    );
}

#[test]
fn discover_succeeds_and_every_online_processor_is_in_some_group() {
    let topo = MachineMemoryTopology::discover().expect("discover");
    assert!(!topo.processors.is_empty());
    assert!(topo.groups().count() >= 1);

    let all_group_processors: ProcessorSet = topo
        .groups()
        .fold(ProcessorSet::empty(), |acc, g| acc.union(&g.processors));
    for processor in topo.processors.iter().filter(|p| p.online) {
        assert!(
            all_group_processors.contains(processor.id.group, processor.id.number),
            "online processor {:?} is not a member of any group's processor set",
            processor.id
        );
    }
}

#[test]
fn discover_reports_a_processor_entry_for_every_slot_up_to_each_groups_maximum() {
    let topo = MachineMemoryTopology::discover().expect("discover");
    let relations = crate::relation::discover().expect("discover relations");
    let expected: usize = relations
        .groups
        .iter()
        .map(|g| g.maximum_processor_count as usize)
        .sum();
    assert_eq!(topo.processors.len(), expected);
}

// --- serde (M3.3) ---

#[cfg(feature = "serde")]
mod serde_tests {
    use super::super::*;

    #[test]
    fn a_discovered_topology_round_trips_through_json_except_for_its_provenance() {
        // This test used to assert the round trip was *unchanged*. That is now
        // deliberately false, and the change is the point rather than a
        // regression: a discovered topology asserts "this is the machine you
        // are on", and once written to a file it can no longer assert that.
        // Reloading yields `Restored`.
        //
        // Two things are downgraded across the boundary, for one reason. The
        // provenance drops to `Restored`, and every relation's platform
        // observations are dropped -- because a file saying "the relationship
        // walk observed this" cannot establish that it did, and carrying the
        // claim would be exactly the forgery D-12 refuses.
        //
        // The assertion is deliberately not weakened to "the parts I still
        // expect to match". Everything else must survive verbatim, so this
        // compares against the original with only those two adjusted -- a
        // second corruption would still fail here.
        let topology = MachineMemoryTopology::discover().expect("discover");
        assert!(
            topology.provenance.is_measured(),
            "discover must claim the machine it read"
        );
        assert!(
            topology
                .domains
                .iter()
                .all(|domain| !domain.observations.is_empty()),
            "a discovered relation must record which source reported it"
        );

        let json = serde_json::to_string(&topology).expect("serialize");
        let back: MachineMemoryTopology = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.provenance, Provenance::Restored);
        assert!(
            back.domains
                .iter()
                .all(|domain| domain.observations.is_empty()),
            "a restored relation must not claim a platform source observed it"
        );

        let mut expected = topology;
        expected.provenance = Provenance::Restored;
        for domain in &mut expected.domains {
            domain.observations.clear();
        }
        assert_eq!(back, expected);
    }

    #[test]
    fn both_sources_are_folded_into_one_relation_where_they_agree() {
        // M3+.1.2: the walk and CPU Sets both describe cores and NUMA nodes. On
        // a machine where they agree -- measured to be the ordinary case
        // (D-15) -- that is ONE relation carrying TWO observations, not two
        // competing relations.
        //
        // This test replaced one asserting exactly one observation per
        // relation, which is what caught this change arriving rather than
        // letting it land unnoticed.
        let topology = MachineMemoryTopology::discover().expect("discover");

        let doubly_observed = topology
            .domains
            .iter()
            .filter(|domain| domain.observations.len() > 1)
            .count();
        assert!(
            doubly_observed > 0,
            "both Win32 sources report cores, so at least one relation must \
             carry two observations: {:?}",
            topology
                .domains
                .iter()
                .map(|d| (&d.kind, d.observations.len()))
                .collect::<Vec<_>>()
        );

        for domain in &topology.domains {
            assert!(
                !domain.observations.is_empty(),
                "every relation names who reported it: {domain:?}"
            );
            let mut sources: Vec<_> = domain.observations.iter().map(|o| o.source).collect();
            sources.sort_unstable();
            sources.dedup();
            assert_eq!(
                sources.len(),
                domain.observations.len(),
                "one observation per source, never two from the same one: {domain:?}"
            );
        }
    }

    #[test]
    fn a_doubly_observed_relation_keeps_both_labels() {
        // The measured disagreement D-15 is built on: the sources agree on the
        // core partition and label it differently. Both labels survive, which
        // they could not if the relation carried a single id.
        let topology = MachineMemoryTopology::discover().expect("discover");

        let both: Vec<_> = topology
            .domains
            .iter()
            .filter(|d| d.observations.len() > 1)
            .collect();
        assert!(!both.is_empty(), "nothing was unified");

        for domain in both {
            assert!(
                domain
                    .observations
                    .iter()
                    .any(|o| o.source == Source::RelationshipWalk),
                "{domain:?}"
            );
            assert!(
                domain
                    .observations
                    .iter()
                    .any(|o| o.source == Source::CpuSets),
                "{domain:?}"
            );
        }
    }

    #[test]
    fn the_last_level_cache_grouping_is_not_folded_into_a_cache_relation() {
        // D-14: CPU Sets' LastLevelCacheIndex answers a different question from
        // the derived cache partitioning -- one group against eight L2
        // partitions on the development host -- so folding it into `Cache`
        // would assert an agreement neither source made.
        let topology = MachineMemoryTopology::discover().expect("discover");

        for domain in topology.caches() {
            assert!(
                domain
                    .observations
                    .iter()
                    .all(|o| o.source == Source::RelationshipWalk),
                "no cache relation may carry a CPU-sets observation: {domain:?}"
            );
        }
    }
    /// A CPU-set record for one processor in group 0.
    fn cpu_set(index: u8, core: u8, node: u8, efficiency_class: u8) -> crate::cpu_set::CpuSet {
        crate::cpu_set::CpuSet {
            id: u32::from(index),
            group: 0,
            logical_processor_index: index,
            core_index: core,
            last_level_cache_index: 0,
            numa_node_index: node,
            efficiency_class,
            parked: false,
            allocated: true,
            allocated_to_target_process: true,
            real_time: false,
            scheduling_class: 0,
            allocation_tag: 0,
        }
    }

    fn core_domain(label: u32, members: &[u8], efficiency_class: u8) -> Domain {
        let mut processors = ProcessorSet::empty();
        for &m in members {
            processors.insert(0, m);
        }
        Domain {
            kind: DomainKind::Core {
                simultaneous_multithreading: members.len() > 1,
                efficiency_class,
            },
            processors,
            observations: vec![Observation::new(Source::RelationshipWalk, label)],
        }
    }

    // --- the fold, against shapes this host does not have ---
    //
    // Every other fold test runs `discover()`, which sees one machine whose two
    // sources agree exactly. That cannot distinguish matching on equal
    // membership from matching on a subset, because here the two coincide -- a
    // sabotage run proved it, passing all 169 tests. These build the disagreeing
    // shapes deliberately.

    #[test]
    fn folding_matches_on_equal_membership_not_on_containment() {
        // The walk reports one four-processor core; CPU Sets reports two
        // two-processor ones. Each CPU-sets membership is a strict SUBSET of the
        // walk's, so a containment match would attach both observations to the
        // walk's relation and record a false agreement.
        let mut topology = MachineMemoryTopology {
            processors: Vec::new(),
            domains: vec![core_domain(0, &[0, 1, 2, 3], 0)],
            cpu_sets: None,
            provenance: Provenance::Synthetic,
        };
        topology.fold_in_cpu_sets(&[
            cpu_set(0, 0, 0, 0),
            cpu_set(1, 0, 0, 0),
            cpu_set(2, 1, 0, 0),
            cpu_set(3, 1, 0, 0),
        ]);

        let walk_relation = topology
            .domains
            .iter()
            .find(|d| d.processors.len() == 4)
            .expect("the walk's relation survives");
        assert_eq!(
            walk_relation.observations.len(),
            1,
            "a relation nothing agreed with keeps its single observation"
        );

        let cpu_only: Vec<_> = topology
            .domains
            .iter()
            .filter(|d| {
                matches!(d.kind, DomainKind::Core { .. })
                    && d.observations.iter().all(|o| o.source == Source::CpuSets)
            })
            .collect();
        assert_eq!(
            cpu_only.len(),
            2,
            "each disagreeing CPU-sets membership becomes its own relation: {:?}",
            topology.domains
        );
    }

    #[test]
    fn a_core_only_cpu_sets_reports_takes_its_efficiency_class_from_the_record() {
        // Never fabricated. Defaulting to `0` would reinvent the
        // `Processor::capacity` sentinel, because `0` is a legitimate class.
        let mut topology = MachineMemoryTopology {
            processors: Vec::new(),
            domains: Vec::new(),
            cpu_sets: None,
            provenance: Provenance::Synthetic,
        };
        topology.fold_in_cpu_sets(&[cpu_set(0, 0, 0, 2), cpu_set(1, 0, 0, 2)]);

        let core = topology
            .domains
            .iter()
            .find(|d| matches!(d.kind, DomainKind::Core { .. }))
            .expect("a core relation");
        assert!(
            matches!(
                core.kind,
                DomainKind::Core {
                    efficiency_class: 2,
                    simultaneous_multithreading: true
                }
            ),
            "{:?}",
            core.kind
        );
        assert_eq!(
            core.observations,
            vec![Observation::new(Source::CpuSets, 0)]
        );
    }

    #[test]
    fn folding_agreeing_sources_yields_one_relation_with_both_labels() {
        let mut topology = MachineMemoryTopology {
            processors: Vec::new(),
            domains: vec![core_domain(7, &[0, 1], 0)],
            cpu_sets: None,
            provenance: Provenance::Synthetic,
        };
        topology.fold_in_cpu_sets(&[cpu_set(0, 3, 0, 0), cpu_set(1, 3, 0, 0)]);

        let cores: Vec<_> = topology
            .domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Core { .. }))
            .collect();
        assert_eq!(cores.len(), 1, "agreement is one relation: {cores:?}");
        assert_eq!(
            cores[0].observations,
            vec![
                Observation::new(Source::RelationshipWalk, 7),
                Observation::new(Source::CpuSets, 3),
            ],
            "both labels survive, which is the whole of D-15"
        );
    }

    #[test]
    fn folding_never_attaches_a_cpu_sets_observation_to_the_wrong_kind() {
        // A memory domain covering the same processors as a core must not
        // absorb the core's CPU-sets observation.
        let mut memory = core_domain(0, &[0, 1], 0);
        memory.kind = DomainKind::Memory { memory_bytes: None };
        let mut topology = MachineMemoryTopology {
            processors: Vec::new(),
            domains: vec![memory],
            cpu_sets: None,
            provenance: Provenance::Synthetic,
        };
        topology.fold_in_cpu_sets(&[cpu_set(0, 5, 0, 0), cpu_set(1, 5, 0, 0)]);

        let memory_domain = topology
            .domains
            .iter()
            .find(|d| matches!(d.kind, DomainKind::Memory { .. }))
            .expect("the memory domain");
        assert!(
            memory_domain
                .observations
                .iter()
                .any(|o| o.source == Source::CpuSets),
            "the NUMA membership does match this one, and should attach"
        );
        let core = topology
            .domains
            .iter()
            .find(|d| matches!(d.kind, DomainKind::Core { .. }))
            .expect("the core arrives as its own relation");
        assert_eq!(
            core.observations,
            vec![Observation::new(Source::CpuSets, 5)]
        );
    }
    // --- M3+.2: the two Provenance properties, re-derived at the relation level ---

    #[test]
    fn a_relation_nobody_reported_claims_no_source() {
        // "The default is the untrusted value", carried down a level. A
        // hand-built relation has an empty observation list, which says nobody
        // reported it -- rather than defaulting to a source and asserting
        // something no API said. The argument is STRONGER here than for the
        // object, because there are far more places to forget.
        let domain = core_domain(0, &[0, 1], 0);
        assert_eq!(domain.observations.len(), 1, "the helper states its source");

        let silent = Domain {
            observations: Vec::new(),
            ..core_domain(0, &[0, 1], 0)
        };
        assert!(
            silent.observations.is_empty(),
            "nothing fills this in on a caller's behalf"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn relation_level_trust_never_upgrades_across_a_file() {
        // "Trust never upgrades", carried down a level. A description asserting
        // the relationship walk observed something cannot establish that it
        // did, so the claim does not survive deserialization -- the same rule
        // `Provenance::downgraded_to` applies to the object (D-12).
        let json = r#"{
            "processors": [
                {"id": {"group": 0, "number": 0}, "online": true, "capacity": 0}
            ],
            "domains": [
                {"kind": "core", "id": 0, "processors": [{"group":0,"number":0}],
                 "simultaneous_multithreading": false, "efficiency_class": 0}
            ],
            "provenance": "measured"
        }"#;
        let topology: MachineMemoryTopology = serde_json::from_str(json).expect("parse");

        assert_eq!(
            topology.provenance,
            Provenance::Restored,
            "the object's claim is capped"
        );
        assert!(
            topology
                .domains
                .iter()
                .all(|domain| domain.observations.is_empty()),
            "and no relation claims a platform source either"
        );
    }
    #[test]
    fn a_hand_written_synthetic_topology_parses() {
        let json = r#"{
            "processors": [
                {"id": {"group": 0, "number": 0}, "online": true, "capacity": 10},
                {"id": {"group": 0, "number": 1}, "online": true, "capacity": 10}
            ],
            "domains": [
                {"kind": "group", "id": 0, "processors": [{"group":0,"number":0},{"group":0,"number":1}]},
                {"kind": "memory", "id": 0, "processors": [], "memory_bytes": 68719476736}
            ],
            "distances": null
        }"#;
        let topology: MachineMemoryTopology = serde_json::from_str(json).expect("parse");
        assert_eq!(topology.processors.len(), 2);
        assert_eq!(topology.groups().count(), 1);
        let memory: Vec<_> = topology.memory_domains().collect();
        assert_eq!(memory.len(), 1);
        assert!(memory[0].processors.is_empty());
    }

    /// A description shaped like what a Linux system would produce: a
    /// single processor group (Linux has no group concept), a memory-only
    /// node, and a populated scalar distance matrix.
    ///
    /// The matrix is **ignored** as of D-20 in `DESIGN-NOTES.md`: this crate does not go below
    /// the Win32 topology APIs, so inter-node distance is not a fact it
    /// states, and the field it used to be read into is gone. The test keeps
    /// the populated matrix rather than dropping it, because what needs
    /// proving is that such a description still *parses* -- nothing here sets
    /// `deny_unknown_fields`, so an existing Linux-shaped description does not
    /// become unreadable. It does not round-trip: the value is dropped on read
    /// and absent on write.
    #[test]
    fn a_linux_shaped_description_parses_and_its_distances_are_ignored() {
        let json = r#"{
            "processors": [
                {"id": {"group": 0, "number": 0}, "online": true, "capacity": 1024},
                {"id": {"group": 0, "number": 1}, "online": true, "capacity": 1024}
            ],
            "domains": [
                {"kind": "group", "id": 0, "processors": [{"group":0,"number":0},{"group":0,"number":1}]},
                {"kind": "memory", "id": 0, "processors": [{"group":0,"number":0},{"group":0,"number":1}],
                 "memory_bytes": 17179869184},
                {"kind": "memory", "id": 1, "processors": [], "memory_bytes": 549755813888}
            ],
            "distances": {"over": "memory", "matrix": [[10, 40], [40, 10]]}
        }"#;
        let topology: MachineMemoryTopology = serde_json::from_str(json).expect("parse");
        assert_eq!(topology.memory_domains().count(), 2);
        assert!(
            topology.memory_domains().any(|d| d.processors.is_empty()),
            "the CXL-shaped node must survive"
        );

        // The half that D-20 changed: re-serializing does not carry the matrix
        // back out, so the drop is silent and is asserted rather than assumed.
        let round_tripped = serde_json::to_string(&topology).expect("serialize");
        assert!(
            !round_tripped.contains("distances"),
            "distances must not reappear on write: {round_tripped}"
        );
    }

    /// The other half of D-10: a single "group" holding more than 64
    /// processors -- ordinary on Linux, where there is no group concept at
    /// all -- cannot be materialised into this crate's `ProcessorSet`, whose
    /// mask is one machine word because a real `GROUP_AFFINITY` is. The
    /// documented, sanctioned response is to reject rather than silently
    /// drop processors 64 and up.
    #[test]
    fn a_description_claiming_more_than_64_processors_in_one_group_is_rejected() {
        let json = r#"{
            "processors": [],
            "domains": [
                {"kind": "group", "id": 0, "processors": [{"group": 0, "number": 100}]}
            ],
            "distances": null
        }"#;
        let error = serde_json::from_str::<MachineMemoryTopology>(json)
            .expect_err("processor number 100 is out of range");
        assert!(
            error.to_string().contains("100"),
            "error should name the offending number: {error}"
        );
    }
}

#[test]
fn a_hand_built_topology_is_not_measured() {
    // The fixture above names `Synthetic` explicitly; this pins down that the
    // value survives to a reader, so a consumer asking "is this my machine"
    // gets the right answer from hand-built data.
    assert_eq!(synthetic().provenance, Provenance::Synthetic);
    assert!(!synthetic().provenance.is_measured());
}

#[test]
fn a_defaulted_topology_is_not_measured() {
    // `MachineMemoryTopology::default()` is the easiest way to obtain one and must be the
    // safe one. If this ever reports measured, every forgetful construction in
    // every dependent silently starts asserting it read the machine.
    let topology = MachineMemoryTopology::default();

    assert_eq!(topology.provenance, Provenance::Synthetic);
    assert!(!topology.provenance.is_measured());
}

#[test]
fn struct_update_syntax_from_default_stays_untrusted() {
    // `..Default::default()` is how a caller builds a topology while naming
    // only the fields they care about, and provenance is exactly the field
    // nobody thinks to name.
    let topology = MachineMemoryTopology {
        cpu_sets: None,
        ..Default::default()
    };

    assert!(!topology.provenance.is_measured());
}

#[cfg(feature = "serde")]
mod serde_provenance {
    use super::*;

    fn load(provenance_field: &str) -> MachineMemoryTopology {
        let json =
            format!(r#"{{"processors": [], "domains": [], "distances": null{provenance_field}}}"#);
        serde_json::from_str(&json).expect("the description must parse")
    }

    #[test]
    fn a_description_claiming_measured_is_downgraded_to_restored() {
        // The core of the rule. A file cannot establish that it is the machine
        // you are running on, however sincerely it asserts it -- and a
        // hand-edited description is the obvious way someone would try.
        let topology = load(r#", "provenance": "measured""#);

        assert_eq!(topology.provenance, Provenance::Restored);
        assert!(!topology.provenance.is_measured());
    }

    #[test]
    fn a_description_claiming_restored_stays_restored() {
        assert_eq!(
            load(r#", "provenance": "restored""#).provenance,
            Provenance::Restored
        );
    }

    #[test]
    fn a_description_claiming_synthetic_is_not_promoted() {
        // The ceiling is a maximum, not an assignment: passing through a loader
        // must not launder fabricated data into merely-restored data.
        assert_eq!(
            load(r#", "provenance": "synthetic""#).provenance,
            Provenance::Synthetic
        );
    }

    #[test]
    fn a_description_without_the_field_loads_as_synthetic() {
        // Every description written before this field existed takes this path,
        // so the default has to be the safe one here too.
        assert_eq!(load("").provenance, Provenance::Synthetic);
    }

    #[test]
    fn a_measured_topology_does_not_survive_a_round_trip_as_measured() {
        // The property that makes persistence honest, stated end to end: you
        // may archive a real topology, and what you reload is explicitly a
        // description of a machine rather than a claim about this one.
        let mut measured = synthetic();
        measured.provenance = Provenance::Measured;

        let json = serde_json::to_string(&measured).expect("must serialize");
        assert!(
            json.contains("measured"),
            "the marker is not visible in the persisted form: {json}"
        );

        let reloaded: MachineMemoryTopology = serde_json::from_str(&json).expect("must parse");
        assert_eq!(reloaded.provenance, Provenance::Restored);
        assert!(!reloaded.provenance.is_measured());
    }

    #[test]
    fn everything_but_the_provenance_round_trips_unchanged() {
        // The downgrade must be the *only* thing a round trip changes,
        // otherwise this would be trading one silent corruption for another.
        let mut measured = synthetic();
        measured.provenance = Provenance::Measured;

        let json = serde_json::to_string(&measured).expect("must serialize");
        let reloaded: MachineMemoryTopology = serde_json::from_str(&json).expect("must parse");

        assert_eq!(reloaded.processors, measured.processors);
        assert_eq!(reloaded.domains, measured.domains);
        assert_eq!(reloaded.cpu_sets, measured.cpu_sets);
    }
}

// --- capacity is the class of the processor's OWN core (mutation-testing gap) ---
//
// A `cargo mutants` run replaced the match guard in `processors_from` -- the
// test that a core domain actually contains the processor being described --
// with both `true` and `false`, and the whole suite passed either way.
//
// Both mutants are real defects. With `true`, every processor takes the first
// core domain's efficiency class, so on a heterogeneous machine the performance
// cores would be reported with the efficiency cores' class. With `false`, no
// domain ever matches and every processor reports capacity 0.
//
// Nothing caught them because the only test reaching `processors_from` asserted
// the *count* of processors, and the serde round-trip compares a discovered
// topology against itself -- identically wrong on both sides of the comparison.
//
// These use a synthetic two-core, two-class fixture rather than the real
// machine, so they assert the mapping on every host rather than only on a
// heterogeneous one.

/// Two processors on two cores of different efficiency classes.
fn heterogeneous_relations() -> (crate::relation::Relations, Vec<Domain>) {
    use crate::relation::{CoreRelation, GroupRelation, Relations};

    let cpu0 = ProcessorSet::from_group_mask(0, 0b01);
    let cpu1 = ProcessorSet::from_group_mask(0, 0b10);

    let relations = Relations {
        cores: vec![
            CoreRelation {
                simultaneous_multithreading: false,
                efficiency_class: 0,
                processors: cpu0.clone(),
            },
            CoreRelation {
                simultaneous_multithreading: false,
                efficiency_class: 1,
                processors: cpu1.clone(),
            },
        ],
        packages: Vec::new(),
        dies: Vec::new(),
        modules: Vec::new(),
        caches: Vec::new(),
        numa_nodes: Vec::new(),
        groups: vec![GroupRelation {
            group: 0,
            maximum_processor_count: 2,
            active_processor_count: 2,
            active_processors: ProcessorSet::from_group_mask(0, 0b11),
        }],
    };

    let domains = vec![
        Domain {
            kind: DomainKind::Core {
                simultaneous_multithreading: false,
                efficiency_class: 0,
            },
            processors: cpu0,
            observations: Vec::new(),
        },
        Domain {
            kind: DomainKind::Core {
                simultaneous_multithreading: false,
                efficiency_class: 1,
            },
            processors: cpu1,
            observations: Vec::new(),
        },
    ];

    (relations, domains)
}

#[test]
fn each_processor_takes_the_efficiency_class_of_its_own_core() {
    let (relations, domains) = heterogeneous_relations();
    let processors = MachineMemoryTopology::processors_from(&relations, &domains);

    assert_eq!(processors.len(), 2);
    assert_eq!(
        processors[0].capacity, 0,
        "processor 0 belongs to the class-0 core"
    );
    assert_eq!(
        processors[1].capacity, 1,
        "processor 1 belongs to the class-1 core, and must not inherit the \
         first core domain's class"
    );
}

#[test]
fn a_processor_with_no_matching_core_domain_reports_no_capacity() {
    // The other side of the same guard: a domain list that does not describe
    // this processor must yield 0 rather than borrowing some other core's
    // class. Windows reports relations only for active processors, so this is
    // the inactive-slot path.
    let (relations, _) = heterogeneous_relations();
    let processors = MachineMemoryTopology::processors_from(&relations, &[]);

    assert_eq!(processors.len(), 2);
    for processor in &processors {
        assert_eq!(
            processor.capacity, 0,
            "with no core domains there is no class to report"
        );
    }
}

#[test]
fn an_offline_processor_reports_no_capacity_even_when_a_core_claims_it() {
    use crate::relation::{CoreRelation, GroupRelation, Relations};

    // A slot that exists but is not active. The core domain still names it, so
    // only the `online` check keeps its capacity at 0 -- which makes this the
    // test for that check rather than for the guard above.
    let both = ProcessorSet::from_group_mask(0, 0b11);
    let relations = Relations {
        cores: vec![CoreRelation {
            simultaneous_multithreading: false,
            efficiency_class: 7,
            processors: both.clone(),
        }],
        packages: Vec::new(),
        dies: Vec::new(),
        modules: Vec::new(),
        caches: Vec::new(),
        numa_nodes: Vec::new(),
        groups: vec![GroupRelation {
            group: 0,
            maximum_processor_count: 2,
            active_processor_count: 1,
            // Only processor 0 is online.
            active_processors: ProcessorSet::from_group_mask(0, 0b01),
        }],
    };
    let domains = vec![Domain {
        kind: DomainKind::Core {
            simultaneous_multithreading: false,
            efficiency_class: 7,
        },
        processors: both,
        observations: Vec::new(),
    }];

    let processors = MachineMemoryTopology::processors_from(&relations, &domains);

    assert!(processors[0].online);
    assert_eq!(processors[0].capacity, 7);
    assert!(!processors[1].online);
    assert_eq!(
        processors[1].capacity, 0,
        "an offline slot's capacity is not invented from a domain that names it"
    );
}

/// A machine of `cores` two-processor cores, each with the split L1 that real
/// firmware reports, plus a shared last-level cache.
///
/// The split L1 is the point: Windows reports one relationship per *cache*, so
/// a core contributes an L1 `data` domain **and** an L1 `instruction` domain
/// covering exactly the same two processors.
fn split_l1_machine(cores: u32, last_level: u8) -> MachineMemoryTopology {
    let mut domains = Vec::new();
    let mut all = 0usize;
    let mut id = 0u32;
    for core in 0..cores {
        let mask = 0b11usize << (core * 2);
        all |= mask;
        let processors = ProcessorSet::from_group_mask(0, mask);
        for cache_type in [CacheKind::Data, CacheKind::Instruction] {
            domains.push(Domain {
                kind: DomainKind::Cache {
                    level: 1,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 32 * 1024,
                    cache_type,
                },
                processors: processors.clone(),
                // The fixture stands in for a discovered machine, so its
                // relations say who reported them and carry the walk's own
                // numbering -- which is what the partitioning rule reads back.
                observations: vec![Observation::new(Source::RelationshipWalk, id)],
            });
            id += 1;
        }
    }
    domains.push(Domain {
        kind: DomainKind::Cache {
            level: last_level,
            associativity: 16,
            line_size: 64,
            size_bytes: 32 * 1024 * 1024,
            cache_type: CacheKind::Unified,
        },
        processors: ProcessorSet::from_group_mask(0, all),
        observations: Vec::new(),
    });

    MachineMemoryTopology {
        processors: Vec::new(),
        domains,
        cpu_sets: None,
        provenance: Provenance::Synthetic,
    }
}

#[test]
fn cache_levels_are_ascending_and_without_repeats() {
    // Sixteen L1 relationships, one L3, but only two distinct levels.
    assert_eq!(split_l1_machine(8, 3).cache_levels(), vec![1, 3]);
}

#[test]
fn cache_levels_are_empty_when_no_cache_is_reported() {
    let topo = MachineMemoryTopology {
        processors: Vec::new(),
        domains: Vec::new(),
        cpu_sets: None,
        provenance: Provenance::Synthetic,
    };
    assert!(topo.cache_levels().is_empty());
}

#[test]
fn a_split_instruction_and_data_cache_is_one_partition_not_two() {
    // The measured shape of the development host: eight cores, so sixteen L1
    // relationships over eight distinct processor pairs. Counting
    // relationships reports twice as many partitions as the machine has.
    let topo = split_l1_machine(8, 3);
    assert_eq!(topo.caches_at_level(1).count(), 16);
    assert_eq!(topo.cache_partitions_at_level(1).len(), 8);
}

#[test]
fn cache_partitions_keep_the_first_domain_for_each_processor_set() {
    let topo = split_l1_machine(2, 3);
    let ids: Vec<u32> = topo
        .cache_partitions_at_level(1)
        .iter()
        .map(|domain| {
            domain
                .label_from(Source::RelationshipWalk)
                .expect("the fixture records a walk observation")
        })
        .collect();
    // 0 and 2 are the `data` domains; 1 and 3 are the `instruction` domains
    // covering the same processors, and are the ones dropped.
    assert_eq!(ids, vec![0, 2]);
}

#[test]
fn the_outermost_partitioning_cache_skips_a_level_that_covers_everything() {
    // L3 spans the machine and so divides nothing, however far out it sits.
    let topo = split_l1_machine(4, 3);
    let (level, partitions) = topo.outermost_partitioning_cache().expect("L1 divides");
    assert_eq!(level, 1);
    assert_eq!(partitions.len(), 4);
}

#[test]
fn a_partitioning_cache_above_level_four_is_found() {
    // `level` is a `u8`. A consumer sweeping a hard-coded `1..=4` reports this
    // machine as having no partitioning cache at all.
    let mut topo = split_l1_machine(1, 5);
    // Replace the shared last level with two L5 partitions, so the dividing
    // level is one a fixed `1..=4` ceiling cannot reach.
    topo.domains.pop();
    for (_id, mask) in [(100u32, 0b01usize), (101, 0b10)] {
        topo.domains.push(Domain {
            kind: DomainKind::Cache {
                level: 5,
                associativity: 16,
                line_size: 64,
                size_bytes: 64 * 1024 * 1024,
                cache_type: CacheKind::Unified,
            },
            processors: ProcessorSet::from_group_mask(0, mask),
            observations: Vec::new(),
        });
    }
    let (level, partitions) = topo.outermost_partitioning_cache().expect("L5 divides");
    assert_eq!(level, 5);
    assert_eq!(partitions.len(), 2);
}

#[test]
fn a_single_core_split_l1_partitions_nothing() {
    // The false positive deduplication removes: two cache relationships over
    // one processor set is one partition, so no level divides this machine and
    // a caller must not be told L1 does.
    let topo = split_l1_machine(1, 3);
    assert_eq!(topo.caches_at_level(1).count(), 2);
    assert_eq!(topo.cache_partitions_at_level(1).len(), 1);
    assert!(topo.outermost_partitioning_cache().is_none());
}

#[test]
fn a_machine_with_no_cache_at_all_has_no_partitioning_cache() {
    assert!(synthetic().outermost_partitioning_cache().is_none());
}

#[test]
fn a_level_whose_domains_overlap_is_not_a_partition() {
    // `MachineMemoryTopology` is deliberately constructible by hand and by deserialization,
    // so `outermost_partitioning_cache` cannot assume hardware produced its
    // input. Two L2 domains that share processor 1 are distinct sets, so
    // deduplication keeps both -- and a caller told they are partitions places
    // work on processor 1 twice and overwrites its domain assignment.
    let mut topo = split_l1_machine(1, 3);
    topo.domains.pop(); // the shared last level, which divides nothing
    for (_id, mask) in [(200u32, 0b011usize), (201, 0b110)] {
        topo.domains.push(Domain {
            kind: DomainKind::Cache {
                level: 2,
                associativity: 8,
                line_size: 64,
                size_bytes: 1024 * 1024,
                cache_type: CacheKind::Unified,
            },
            processors: ProcessorSet::from_group_mask(0, mask),
            observations: Vec::new(),
        });
    }

    // Both survive deduplication, which only removes *equal* sets...
    assert_eq!(topo.cache_partitions_at_level(2).len(), 2);
    // ...but overlapping domains are not a partition, so no level qualifies.
    assert!(
        topo.outermost_partitioning_cache().is_none(),
        "overlapping cache domains must not be reported as partitions"
    );
}

#[test]
fn a_level_whose_domains_are_disjoint_but_incomplete_still_partitions() {
    // The deliberate limit of the disjointness rule. A processor with no cache
    // reported at this level is a gap in what the firmware said; the domains
    // that *were* reported still divide the processors they cover, so
    // discarding the level over the gap would throw away a true boundary.
    let mut topo = split_l1_machine(1, 3);
    topo.domains.pop();
    for (_id, mask) in [(300u32, 0b0001usize), (301, 0b0010)] {
        topo.domains.push(Domain {
            kind: DomainKind::Cache {
                level: 2,
                associativity: 8,
                line_size: 64,
                size_bytes: 1024 * 1024,
                cache_type: CacheKind::Unified,
            },
            processors: ProcessorSet::from_group_mask(0, mask),
            observations: Vec::new(),
        });
    }

    let (level, partitions) = topo
        .outermost_partitioning_cache()
        .expect("two disjoint L2 domains divide what they cover");
    assert_eq!(level, 2);
    assert_eq!(partitions.len(), 2);
}

#[test]
fn a_domain_covering_nothing_is_not_a_partition() {
    // The other end of the same threat model as the overlap test above. An
    // empty processor set is *disjoint from everything*, vacuously, so it
    // passes the pairwise check; and it is not equal to any non-empty set, so
    // deduplication keeps it. A level with one real cache plus one empty domain
    // therefore counts two "partitions" and is reported as dividing a machine
    // it does not divide -- a caller sharding across the result gets a shard
    // covering no processors at all.
    //
    // `Domain` is publicly constructible and `ProcessorSet` has `empty()`, so
    // this is reachable by hand and by deserialization, which is precisely the
    // input this method promises not to trust.
    let mut topo = split_l1_machine(1, 3);
    topo.domains.pop();
    for (_id, processors) in [
        (400u32, ProcessorSet::from_group_mask(0, 0b111)),
        (401, ProcessorSet::empty()),
    ] {
        topo.domains.push(Domain {
            kind: DomainKind::Cache {
                level: 2,
                associativity: 8,
                line_size: 64,
                size_bytes: 1024 * 1024,
                cache_type: CacheKind::Unified,
            },
            processors,
            observations: Vec::new(),
        });
    }

    assert!(
        topo.outermost_partitioning_cache().is_none(),
        "a level whose only second domain covers nothing does not divide the machine"
    );
}
